//! Test-only lookup of public fixtures and verified model caches (never downloads).
//!
//! Resolution per relative path: `$VT_TEST_FIXTURES/<rel>` when the variable is set;
//! otherwise the legacy `/private/tmp/<rel>` if it exists; otherwise
//! `DEFAULT_ROOT/<rel>`. A missing fixture still resolves to a path, so callers'
//! existing "must exist" assertions fail exactly as before.
use std::path::PathBuf;

const LEGACY_ROOT: &str = "/private/tmp";
const DEFAULT_ROOT: &str = "/Volumes/dev/vantranscribe-eval/test-fixtures";

pub(crate) fn path(rel: &str) -> PathBuf {
    if let Some(root) = std::env::var_os("VT_TEST_FIXTURES") {
        return PathBuf::from(root).join(rel);
    }
    let legacy = PathBuf::from(LEGACY_ROOT).join(rel);
    if legacy.exists() {
        legacy
    } else {
        PathBuf::from(DEFAULT_ROOT).join(rel)
    }
}

/// Hard-links `source` to `destination` (as before); falls back to a copy when the
/// fixture root is on another volume, where hard links are impossible.
pub(crate) fn link_or_copy(source: &std::path::Path, destination: &std::path::Path) {
    if std::fs::hard_link(source, destination).is_err() {
        std::fs::copy(source, destination).unwrap();
    }
}
