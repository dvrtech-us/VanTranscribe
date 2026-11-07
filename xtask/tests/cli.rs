use std::{fs, path::Path, process::Command};
#[test]
fn real_cli_writes_reference_backed_compact_json_and_rejects_bad_options() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("evaluation.json");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/synthetic/manifest.json");
    let result = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["eval", "--manifest"])
        .arg(&fixture)
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let raw = fs::read_to_string(output).unwrap();
    let report: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(report["corpus"]["kind"], "synthetic");
    assert_eq!(report["definition"]["collar_half_width_seconds"], 0.125);
    assert_eq!(report["acceptance"], "not_claimed");
    assert_eq!(raw.lines().count(), 1);
    let invalid = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["eval", "--unknown", "x"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
}

#[test]
fn voice_commands_reject_extra_arguments_and_bad_manifests_without_completed_evidence() {
    let extra = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["voice-version", "--manifest", "anything"])
        .output()
        .unwrap();
    assert!(!extra.status.success());
    assert!(String::from_utf8_lossy(&extra.stderr).contains("accepts no arguments"));
    let version = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("voice-version")
        .output()
        .unwrap();
    #[cfg(feature = "inference")]
    {
        assert!(version.status.success());
        let info: serde_json::Value = serde_json::from_slice(&version.stdout).unwrap();
        assert_eq!(info["matching_available"], false);
        assert_eq!(info["raw_contract_sha256"].as_str().unwrap().len(), 64);
        assert_ne!(
            info["raw_contract_sha256"],
            info["legacy_unavailable_embedding_version"]
        );
        let t = tempfile::tempdir().unwrap();
        let input = t.path().join("bad.json");
        fs::write(&input, b"{}").unwrap();
        let output = t.path().join("research");
        let result = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .arg("voice-eval")
            .arg("--manifest")
            .arg(input)
            .arg("--output")
            .arg(&output)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(output.join("INCOMPLETE").exists());
        assert!(output.join("error.json").exists());
        assert!(!output.join("COMPLETE").exists());
    }
    #[cfg(not(feature = "inference"))]
    {
        assert!(!version.status.success());
        assert!(String::from_utf8_lossy(&version.stderr).contains("--features inference"));
    }
}
#[test]
fn calibration_only_flag_is_explicit_scoped_and_cannot_be_duplicated() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("--calibration-only"), b"{}").unwrap();
    let output = directory.path().join("must-not-start");
    let missing_path = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .current_dir(directory.path())
        .args(["voice-eval", "--manifest", "--calibration-only", "--output"])
        .arg(&output)
        .output()
        .unwrap();
    assert!(!missing_path.status.success());
    assert!(String::from_utf8_lossy(&missing_path.stderr).contains("missing value for --manifest"));
    assert!(!output.exists());
    let duplicate = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["voice-eval", "--calibration-only", "--calibration-only"])
        .output()
        .unwrap();
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("duplicate option"));
    let wrong_command = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["eval", "--calibration-only"])
        .output()
        .unwrap();
    assert!(!wrong_command.status.success());
    assert!(
        String::from_utf8_lossy(&wrong_command.stderr).contains("supported only by voice-eval")
    );
    #[cfg(feature = "inference")]
    {
        let version = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(["voice-version", "--profile", "wespeaker-resnet34-v1"])
            .output()
            .unwrap();
        assert!(version.status.success());
        let info: serde_json::Value = serde_json::from_slice(&version.stdout).unwrap();
        assert_eq!(info["embedding_profile"], "wespeaker-resnet34-v1");
        assert_eq!(info["embedding_dimensions"], 256);
        assert_eq!(info["matching_available"], false);
        assert_ne!(
            info["selected_profile_unavailable_embedding_version"],
            info["legacy_unavailable_embedding_version"]
        );
        let bad = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(["voice-version", "--profile", "arbitrary-user-model"])
            .output()
            .unwrap();
        assert!(!bad.status.success());
    }
}
