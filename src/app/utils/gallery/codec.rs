use mp4;
use openh264::{decoder::Decoder, formats::YUVSource};
use std::path::Path;

pub fn extract_frame(path: &Path, sec: f64) -> Result<(u32, u32, Vec<u8>), String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut mp4 = mp4::read_mp4(file).map_err(|e| e.to_string())?;

    let track = mp4
        .tracks()
        .values()
        .find(|t| t.track_type().unwrap() == mp4::TrackType::Video)
        .ok_or("No video track")?;

    let track_id = track.track_id();
    let timescale = track.timescale() as f64;
    let target_tick = (sec * timescale) as u64;

    let stbl = &track.trak.mdia.minf.stbl;

    // --- ターゲットサンプルの特定 ---
    let mut current_tick = 0;
    let mut target_sample_id = 1;
    for entry in &stbl.stts.entries {
        for _ in 0..entry.sample_count {
            if current_tick + entry.sample_delta as u64 > target_tick {
                break;
            }
            current_tick += entry.sample_delta as u64;
            target_sample_id += 1;
        }
    }

    // --- キーフレームの特定 (シーク) ---
    let mut start_sample_id = 1;
    if let Some(stss) = &stbl.stss {
        for &sync_id in &stss.entries {
            if sync_id <= target_sample_id {
                start_sample_id = sync_id;
            } else {
                break;
            }
        }
    }

    // --- デコーダの初期化 ---
    let mut decoder = Decoder::new().map_err(|e| e.to_string())?;

    // --- SPS / PPS の投入 (重要！) ---
    // MP4 の avcC ボックスから SPS/PPS を取り出し、Annex-B 形式でデコーダに教える
    if let Some(avc1) = &stbl.stsd.avc1 {
        let mut config_payload = Vec::new();
        for sps in &avc1.avcc.sequence_parameter_sets {
            config_payload.extend_from_slice(&[0, 0, 0, 1]);
            config_payload.extend_from_slice(&sps.bytes);
        }
        for pps in &avc1.avcc.picture_parameter_sets {
            config_payload.extend_from_slice(&[0, 0, 0, 1]);
            config_payload.extend_from_slice(&pps.bytes);
        }
        let _ = decoder.decode(&config_payload);
    }

    let mut width = 0;
    let mut height = 0;
    let mut rgba_data = Vec::new();

    // --- デコードループ ---
    for i in start_sample_id..=target_sample_id {
        let sample = mp4
            .read_sample(track_id, i)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Sample not found")?;

        // AVCC (Length Prefixed) を Annex-B (Start Code) に変換
        let annex_b_sample = avcc_to_annex_b(&sample.bytes);

        match decoder.decode(&annex_b_sample) {
            Ok(Some(frame)) => {
                let (w, h) = frame.dimensions();
                width = w as u32;
                height = h as u32;
                let mut rgba = vec![0u8; (width * height * 4) as usize];
                frame.write_rgba8(&mut rgba);
                rgba_data = rgba;
            }
            _ => continue,
        }
    }

    if rgba_data.is_empty() {
        return Err("Failed to decode frame".to_string());
    }
    Ok((width, height, rgba_data))
}

/// AVCC形式のサンプルデータをAnnex-B形式に変換するヘルパー
fn avcc_to_annex_b(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos + 4 <= data.len() {
        // 先頭4バイトからNALユニットの長さを取得
        let len =
            u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        pos += 4;

        if pos + len <= data.len() {
            // Annex-B の開始コードを付与
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(&data[pos..pos + len]);
            pos += len;
        } else {
            break;
        }
    }
    out
}
