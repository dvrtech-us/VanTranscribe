//! Explicit developer tool for generated synthetic inputs only. It never opens
//! an audio device or model and refuses to replace an existing output file.
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    path::PathBuf,
};
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut input = None;
    let mut output = None;
    while let Some(flag) = args.next() {
        let value = args.next().ok_or("missing option value")?;
        let slot = match flag.as_str() {
            "--input" => &mut input,
            "--output" => &mut output,
            _ => return Err("only --input and --output are accepted".into()),
        };
        if slot.replace(PathBuf::from(value)).is_some() {
            return Err("duplicate option".into());
        }
    }
    let input = input.ok_or("--input is required (synthetic f32le mono 16 kHz)")?;
    let output = output.ok_or("--output is required (new binary file)")?;
    if output.exists() {
        return Err("output must be a new file".into());
    }
    let mut bytes = Vec::new();
    File::open(input)?
        .take((vt_speaker_frontend::MAX_SAMPLES * 4 + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > vt_speaker_frontend::MAX_SAMPLES * 4 || !bytes.len().is_multiple_of(4) {
        return Err("input exceeds five seconds or has invalid f32le geometry".into());
    }
    let samples = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| f32::from_le_bytes(*v))
        .collect::<Vec<_>>();
    let matrix = vt_speaker_frontend::reference_features(&samples)?;
    let mut data = Vec::with_capacity(matrix.values.len() * 4);
    for value in &matrix.values {
        data.extend_from_slice(&value.to_le_bytes());
    }
    let header = serde_json::json!({"schema_version":1,"research_only":true,"synthetic_fixture_only":true,"sample_rate":16000,"input_samples":samples.len(),"input_sha256":format!("{:x}",Sha256::digest(&bytes)),"frames":matrix.frames,"bins":matrix.bins,"format":"f32le_row_major_after_json_newline","values_bytes":data.len(),"values_sha256":format!("{:x}",Sha256::digest(&data)),"stage":"raw_log_mel_before_cmn","frontend":{"window":"hamming","snip_edges":true,"feature_dim":80,"low_freq":20,"high_freq":0,"dither":0,"normalize_samples":false,"frame_length_ms":25,"frame_shift_ms":10,"remove_dc_offset":true,"use_energy":false,"preemph_coeff":0.97,"round_to_power_of_two":true}});
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(temporary.as_file_mut(), &header)?;
    temporary.write_all(b"\n")?;
    temporary.write_all(&data)?;
    temporary.as_file().sync_all()?;
    temporary.persist_noclobber(&output)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Synthetic feature probe failed: {error}");
            std::process::ExitCode::from(2)
        }
    }
}
