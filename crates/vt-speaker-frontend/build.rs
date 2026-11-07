use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    env, fs,
    path::{Component, Path, PathBuf},
};
const SDK_SHA256: &str = "1f56629332d0e299e6ae800b8fce008b0b1ed67889df059ba035a457b5d479cb";
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn local(root: &Path, value: &str) -> PathBuf {
    let relative = Path::new(value);
    assert!(
        !relative.is_absolute()
            && relative
                .components()
                .all(|c| matches!(c, Component::Normal(_))),
        "invalid pinned SDK relative path"
    );
    let path = root.join(relative);
    assert!(
        path.canonicalize()
            .expect("missing pinned SDK input")
            .starts_with(root),
        "SDK path escapes vendor root"
    );
    path
}
fn files(root: &Path, path: &Path, out: &mut BTreeSet<PathBuf>) {
    for entry in fs::read_dir(path).expect("cannot enumerate SDK") {
        let entry = entry.expect("invalid SDK entry");
        let kind = entry.file_type().expect("invalid SDK entry type");
        assert!(!kind.is_symlink(), "SDK symlinks are not accepted");
        if kind.is_dir() {
            files(root, &entry.path(), out);
        } else {
            assert!(kind.is_file(), "SDK entry must be a regular file");
            out.insert(entry.path().strip_prefix(root).unwrap().into());
        }
    }
}
fn main() {
    println!("cargo:rustc-check-cfg=cfg(vt_reference_native)");
    println!("cargo:rerun-if-changed=native/bridge.cc");
    println!("cargo:rerun-if-changed=native/bridge.h");
    println!("cargo:rerun-if-changed=vendor/provenance.json");
    if env::var("TARGET").as_deref() != Ok("aarch64-apple-darwin")
        || env::var_os("DOCS_RS").is_some()
    {
        return;
    }
    let root = Path::new("vendor")
        .canonicalize()
        .expect("pinned reference SDK is required");
    let raw = fs::read(root.join("provenance.json")).expect("pinned SDK provenance is required");
    assert_eq!(
        sha(&raw),
        SDK_SHA256,
        "SDK provenance does not match the reviewed pin"
    );
    let provenance: Value = serde_json::from_slice(&raw).expect("SDK provenance JSON invalid");
    assert_eq!(provenance["schema_version"], 1);
    assert_eq!(provenance["target"], "aarch64-apple-darwin");
    assert_eq!(provenance["native_identity"]["sherpa_version"], "2.0.1");
    assert_eq!(
        provenance["native_identity"]["sherpa_commit"],
        "af2d8d81dbfa61c4cea4c568da78f3982edca005"
    );
    assert_eq!(
        provenance["native_identity"]["onnxruntime_version"],
        "1.28.2"
    );
    let mut declared = BTreeSet::from([PathBuf::from("provenance.json")]);
    for item in provenance["files"]
        .as_array()
        .expect("SDK file pins missing")
    {
        let name = item["path"].as_str().expect("SDK path missing");
        let path = local(&root, name);
        let meta = fs::symlink_metadata(&path).expect("SDK file missing");
        assert!(
            meta.is_file() && !meta.is_symlink(),
            "SDK must use original regular files"
        );
        assert_eq!(Some(meta.len()), item["size"].as_u64(), "SDK size mismatch");
        assert_eq!(
            sha(&fs::read(&path).expect("SDK file unreadable")),
            item["sha256"].as_str().unwrap(),
            "SDK SHA-256 mismatch"
        );
        assert!(declared.insert(PathBuf::from(name)), "duplicate SDK file");
        println!("cargo:rerun-if-changed={}", path.display());
    }
    let mut actual = BTreeSet::new();
    files(&root, &root, &mut actual);
    assert_eq!(declared, actual, "unmanifested or missing SDK files");
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .file("native/bridge.cc")
        .warnings(true)
        .extra_warnings(false)
        .warnings_into_errors(true);
    for item in provenance["include_roots"]
        .as_array()
        .expect("SDK include roots missing")
    {
        build
            .flag("-isystem")
            .flag(local(&root, item.as_str().unwrap()).as_os_str());
    }
    // sherpa-onnx-sys is the sole native-library locator/link owner. No guessed
    // target path, alternate ONNX runtime, source build or downloader is added.
    build.compile("vt-speaker-reference-bridge");
    println!("cargo:rustc-cfg=vt_reference_native");
}
