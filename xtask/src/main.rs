use std::{path::PathBuf, process::ExitCode};
fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let command = args
        .next()
        .ok_or("usage: cargo xtask eval --manifest PATH --output PATH")?;
    if command == "--help" || command == "help" {
        println!(
            "cargo xtask eval --manifest PATH --output PATH\nWith --features inference: infer|probe|voice-eval --manifest PATH --output NEW_DIRECTORY\nvoice-eval also accepts --calibration-only; voice-version [--profile ID] prints the frozen embedding contract\nLocal reference-backed metrics/inference; no downloads or capture."
        );
        return Ok(());
    }
    if command == "voice-version" {
        let profile = match (args.next(), args.next(), args.next()) {
            (None, None, None) => None,
            (Some(flag), Some(value), None) if flag == "--profile" => Some(value),
            _ => return Err("voice-version accepts no arguments except --profile ID".into()),
        };
        #[cfg(feature = "inference")]
        {
            let profile = profile
                .as_deref()
                .unwrap_or("eres2net-v1")
                .parse::<vt_diarize::VoiceEmbeddingProfile>()
                .map_err(|error| error.to_string())?;
            println!(
                "{}",
                xtask::speaker_calibration::version_for_profile(profile)
            );
            return Ok(());
        }
        #[cfg(not(feature = "inference"))]
        {
            let _ = profile;
            return Err("voice-version requires an explicit --features inference build".into());
        }
    }
    if command != "eval" && command != "infer" && command != "probe" && command != "voice-eval" {
        return Err(format!("unknown xtask command: {command}"));
    }
    let mut manifest = None;
    let mut output = None;
    let mut calibration_only = false;
    while let Some(flag) = args.next() {
        if flag == "--calibration-only" {
            if command != "voice-eval" {
                return Err("--calibration-only is supported only by voice-eval".into());
            }
            if calibration_only {
                return Err("duplicate option: --calibration-only".into());
            }
            calibration_only = true;
            continue;
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        if value.starts_with("--") {
            return Err(format!("missing value for {flag}"));
        }
        let slot = match flag.as_str() {
            "--manifest" => &mut manifest,
            "--output" => &mut output,
            _ => return Err(format!("unknown option: {flag}")),
        };
        if slot.replace(PathBuf::from(value)).is_some() {
            return Err(format!("duplicate option: {flag}"));
        }
    }
    let manifest = manifest.ok_or("--manifest is required")?;
    let output = output.ok_or("--output is required")?;
    if command == "infer" || command == "probe" || command == "voice-eval" {
        #[cfg(feature = "inference")]
        {
            if command == "infer" {
                xtask::inference::run(&manifest, &output)?;
            } else if command == "voice-eval" {
                xtask::speaker_calibration::run_in_mode(
                    &manifest,
                    &output,
                    if calibration_only {
                        xtask::speaker_calibration::EvaluationMode::CalibrationOnly
                    } else {
                        xtask::speaker_calibration::EvaluationMode::Full
                    },
                )?;
            } else {
                xtask::inference::experiments::run(&manifest, &output)?;
            }
            println!(
                "Public inference evidence written to {}; acceptance is not claimed.",
                output.display()
            );
            return Ok(());
        }
        #[cfg(not(feature = "inference"))]
        return Err(format!(
            "{command} requires an explicit --features inference build"
        ));
    }
    let report = xtask::evaluate_manifest(&manifest)?;
    xtask::write_report(&report, &output)?;
    println!(
        "Evaluation report written to {}; acceptance is not claimed.",
        output.display()
    );
    Ok(())
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Evaluation failed: {error}");
            ExitCode::from(2)
        }
    }
}
