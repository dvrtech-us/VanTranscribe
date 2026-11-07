use std::{env, fs, path::PathBuf, process::Command};

fn run(command: &mut Command) {
    let status = command
        .status()
        .expect("could not start native helper compiler");
    assert!(status.success(), "native helper compilation failed");
}

fn main() {
    println!("cargo:rerun-if-changed=../native/macos");
    println!("cargo:rerun-if-changed=../crates/vt-capture/resources/meeting-apps.def");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let sources = PathBuf::from("../native/macos");
        let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
        let object = out.join("AudioRing.o");
        let resources = PathBuf::from("resources");
        fs::create_dir_all(&resources).expect("capture resources directory");
        run(Command::new("clang")
            .args(["-std=c11", "-O2", "-mmacosx-version-min=14.4", "-c"])
            .arg(sources.join("AudioRing.c"))
            .arg("-o")
            .arg(&object));
        let target = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
            Ok("aarch64") => "arm64-apple-macosx14.4",
            _ => "x86_64-apple-macosx14.4",
        };
        run(Command::new("swiftc")
            .args([
                "-O",
                "-swift-version",
                "5",
                "-target",
                target,
                "-module-cache-path",
            ])
            .arg(out.join("swift-module-cache"))
            .arg("-import-objc-header")
            .arg(sources.join("AudioRing.h"))
            .arg(sources.join("CaptureHelper.swift"))
            .arg(&object)
            .arg("-o")
            .arg(resources.join("vt-capture-helper"))
            .args([
                "-framework",
                "CoreAudio",
                "-framework",
                "AVFoundation",
                "-framework",
                "Foundation",
                "-framework",
                "AppKit",
                "-framework",
                "IOKit",
            ]));
        run(Command::new("xcrun")
            .args([
                "swiftc",
                "-disable-sandbox",
                "-parse-as-library",
                "-O",
                "-target",
                target,
                "-module-cache-path",
            ])
            .arg(out.join("swift-summary-module-cache"))
            .args([
                "-Xlinker",
                "-weak_framework",
                "-Xlinker",
                "FoundationModels",
            ])
            .arg(sources.join("SummaryHelper.swift"))
            .arg("-o")
            .arg(resources.join("vt-summary-helper")));
    }
    tauri_build::build();
}
