use crate::{MemoryError, Result};
use sha2::{Digest, Sha256};
use std::path::Path;
use zeroize::Zeroizing;

pub type SecretKey = Zeroizing<[u8; 32]>;

/// Implementations must distinguish missing from unavailable/locked, and create
/// without replacing an existing key. No implementation may log key material.
pub trait KeyProvider: Send + Sync {
    fn load(&self) -> Result<Option<SecretKey>>;
    fn create(&self) -> Result<SecretKey>;
}

/// Separate from all provider credentials and future database/audio keys. The
/// canonical data root scopes the account, preventing test and user stores from
/// accidentally sharing a biometric key. Constructing this never accesses OS secrets.
pub struct KeychainProvider {
    account: String,
}
impl KeychainProvider {
    pub fn new(root: &Path) -> Result<Self> {
        let root = root
            .canonicalize()
            .map_err(|_| MemoryError::KeyUnavailable)?;
        #[cfg(unix)]
        let bytes = {
            use std::os::unix::ffi::OsStrExt;
            root.as_os_str().as_bytes()
        };
        #[cfg(not(unix))]
        let bytes = root.to_str().ok_or(MemoryError::KeyUnavailable)?.as_bytes();
        Ok(Self {
            account: format!("root-sha256:{:x}", Sha256::digest(bytes)),
        })
    }
}
#[cfg(target_os = "macos")]
const SERVICE: &str = "com.vantranscribe.voice-memory.column-key.v1";
impl KeyProvider for KeychainProvider {
    fn load(&self) -> Result<Option<SecretKey>> {
        #[cfg(target_os = "macos")]
        {
            match security_framework::passwords::get_generic_password(SERVICE, &self.account) {
                Ok(bytes) => {
                    let bytes = Zeroizing::new(bytes);
                    if bytes.len() != 32 {
                        return Err(MemoryError::KeyUnavailable);
                    }
                    let mut key = Zeroizing::new([0; 32]);
                    key.copy_from_slice(&bytes);
                    Ok(Some(key))
                }
                Err(error) if error.code() == -25300 => Ok(None),
                Err(_) => Err(MemoryError::KeyUnavailable),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = &self.account;
            Err(MemoryError::KeyUnavailable)
        }
    }
    fn create(&self) -> Result<SecretKey> {
        #[cfg(target_os = "macos")]
        {
            let mut key = Zeroizing::new([0; 32]);
            getrandom::fill(key.as_mut()).map_err(|_| MemoryError::Random)?;
            use core_foundation::data::CFData;
            use security_framework::item::{ItemAddOptions, ItemAddValue, ItemClass};
            // Both read and write use SecItem's generic-password namespace.
            // SecItemAdd is add-only: unlike set_generic_password it never
            // updates a racing existing entry, including an unreadable one.
            let mut options = ItemAddOptions::new(ItemAddValue::Data {
                class: ItemClass::generic_password(),
                data: CFData::from_buffer(key.as_ref()),
            });
            options.set_service(SERVICE).set_account_name(&self.account);
            match options.add() {
                Ok(()) => Ok(key),
                Err(error) if error.code() == -25299 => {
                    self.load()?.ok_or(MemoryError::KeyUnavailable)
                }
                Err(_) => Err(MemoryError::KeyUnavailable),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(MemoryError::KeyUnavailable)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_accounts_are_canonical_root_scoped_without_os_access() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let one = KeychainProvider::new(a.path()).unwrap();
        let two = KeychainProvider::new(&a.path().join(".")).unwrap();
        assert_eq!(one.account, two.account);
        assert_ne!(
            one.account,
            KeychainProvider::new(b.path()).unwrap().account
        );
        assert!(!one.account.contains(a.path().to_str().unwrap()));
    }

    /// Explicit native evidence for the SecItem namespace used by the adapter.
    /// This uses a private file Keychain and never changes the user's default,
    /// search list, global interaction flag or any existing credential.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "explicit opt-in: creates and destroys a private temporary macOS Keychain"]
    fn private_native_keychain_item_roundtrip_is_add_only_and_scoped() {
        assert_eq!(
            std::env::var("VANTRANSCRIBE_TEST_PRIVATE_KEYCHAIN").as_deref(),
            Ok("1"),
            "The native private-Keychain probe requires explicit opt-in."
        );
        use core_foundation::data::CFData;
        use security_framework::{
            item::{
                ItemAddOptions, ItemAddValue, ItemClass, ItemSearchOptions, Location, SearchResult,
            },
            os::macos::keychain::CreateOptions,
        };
        use std::os::unix::fs::PermissionsExt;

        struct OwnedItem {
            query: ItemSearchOptions,
            owned: bool,
        }
        impl Drop for OwnedItem {
            fn drop(&mut self) {
                if self.owned
                    && let Err(error) = self.query.delete()
                {
                    eprintln!(
                        "temporary_keychain_item_cleanup_failed_code={}",
                        error.code()
                    );
                }
            }
        }
        fn query(
            keychain: &security_framework::os::macos::keychain::SecKeychain,
            account: &str,
            data: bool,
        ) -> ItemSearchOptions {
            let mut query = ItemSearchOptions::new();
            query
                .class(ItemClass::generic_password())
                .service(SERVICE)
                .account(account)
                .keychains(std::slice::from_ref(keychain))
                .load_data(data)
                .skip_authenticated_items(true);
            query
        }
        fn add(
            keychain: &security_framework::os::macos::keychain::SecKeychain,
            account: &str,
            bytes: &[u8],
        ) -> security_framework::base::Result<()> {
            let mut options = ItemAddOptions::new(ItemAddValue::Data {
                class: ItemClass::generic_password(),
                data: CFData::from_buffer(bytes),
            });
            options
                .set_service(SERVICE)
                .set_account_name(account)
                .set_location(Location::FileKeychain(keychain.clone()));
            options.add()
        }
        fn read(
            keychain: &security_framework::os::macos::keychain::SecKeychain,
            account: &str,
        ) -> Zeroizing<Vec<u8>> {
            let mut results = query(keychain, account, true)
                .search()
                .expect("private-Keychain read failed");
            assert_eq!(results.len(), 1, "private-Keychain read must be exact");
            match results.remove(0) {
                SearchResult::Data(bytes) => Zeroizing::new(bytes),
                _ => panic!("private-Keychain query returned a non-data item"),
            }
        }

        let folder = tempfile::Builder::new()
            .prefix("vantranscribe-private-keychain-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in("/private/tmp")
            .unwrap();
        let data_root = folder.path().join("data");
        std::fs::create_dir(&data_root).unwrap();
        let first = KeychainProvider::new(&data_root).unwrap();
        let canonical = KeychainProvider::new(&data_root.join(".")).unwrap();
        assert_eq!(first.account, canonical.account);
        let unrelated_root = folder.path().join("other-data");
        std::fs::create_dir(&unrelated_root).unwrap();
        let unrelated = KeychainProvider::new(&unrelated_root).unwrap();
        assert_ne!(first.account, unrelated.account);
        // Keep only an opaque account ID and owned temporary keychain location
        // in the interruption ledger. Never write or print the random key/pass.
        let keychain_path = folder.path().join("voice-memory-probe.keychain");
        assert!(!keychain_path.exists());
        std::fs::write(
            folder.path().join("probe-ownership.txt"),
            format!(
                "service={SERVICE}\naccount={}\nprivate_keychain={}\n",
                first.account,
                keychain_path.display()
            ),
        )
        .unwrap();
        let password = Zeroizing::new(uuid::Uuid::new_v4().simple().to_string());
        let mut keychain = CreateOptions::new()
            .password(&password)
            .prompt_user(false)
            .create(&keychain_path)
            .expect("private-Keychain creation failed");
        // This unlock affects only the just-created private file and uses its
        // random supplied password; no user credential or UI is involved.
        keychain
            .unlock(Some(&password))
            .expect("private-Keychain unlock failed");
        let mut guard = OwnedItem {
            query: query(&keychain, &first.account, false),
            owned: false,
        };
        match guard.query.search() {
            Err(error) if error.code() == -25300 => {}
            _ => panic!("probe item must be absent before any write"),
        }
        let mut key = Zeroizing::new([0; 32]);
        getrandom::fill(key.as_mut()).expect("random probe key failed");
        add(&keychain, &first.account, key.as_ref()).expect("private-Keychain add failed");
        guard.owned = true;
        let stored = read(&keychain, &canonical.account);
        assert!(
            stored.as_slice() == key.as_ref(),
            "private-Keychain roundtrip changed the key"
        );
        let mut replacement = Zeroizing::new([0; 32]);
        getrandom::fill(replacement.as_mut()).expect("random duplicate probe failed");
        match add(&keychain, &first.account, replacement.as_ref()) {
            Err(error) if error.code() == -25299 => {}
            _ => panic!("duplicate SecItemAdd must refuse replacement"),
        }
        assert!(
            read(&keychain, &canonical.account).as_slice() == key.as_ref(),
            "duplicate write replaced the original key"
        );
        match query(&keychain, &unrelated.account, false).search() {
            Err(error) if error.code() == -25300 => {}
            _ => panic!("a different data root must not find this key"),
        }
        guard
            .query
            .delete()
            .expect("owned private-Keychain item deletion failed");
        guard.owned = false;
        match guard.query.search() {
            Err(error) if error.code() == -25300 => {}
            _ => panic!("owned probe item must be absent after deletion"),
        }
        drop(guard);
        drop(keychain);
        folder
            .close()
            .expect("private-Keychain file cleanup failed");
    }

    #[cfg(target_os = "macos")]
    mod native_default_probe {
        use super::*;
        use std::{
            os::unix::fs::{MetadataExt, PermissionsExt},
            path::{Path, PathBuf},
        };

        const PREFIX: &str = "vantranscribe-default-keychain-probe-";
        const LEDGER: &str = "probe-ledger.json";
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Ledger {
            service: String,
            account: String,
            canonical_data_root: PathBuf,
            phase: String,
        }
        fn record(path: &Path, ledger: &Ledger) {
            let pending = path.with_extension("pending");
            std::fs::write(
                &pending,
                serde_json::to_vec(ledger).expect("probe ledger serialization"),
            )
            .expect("probe ledger write");
            std::fs::set_permissions(&pending, std::fs::Permissions::from_mode(0o600))
                .expect("probe ledger permissions");
            std::fs::rename(pending, path).expect("atomic probe ledger publication");
        }
        fn delete_owned(account: &str) -> std::result::Result<(), security_framework::base::Error> {
            match security_framework::passwords::delete_generic_password(SERVICE, account) {
                Ok(()) => Ok(()),
                Err(error) if error.code() == -25300 => Ok(()),
                Err(error) => Err(error),
            }
        }
        struct OwnedItem {
            account: String,
            owned: bool,
        }
        impl Drop for OwnedItem {
            fn drop(&mut self) {
                if self.owned
                    && let Err(error) = delete_owned(&self.account)
                {
                    eprintln!(
                        "temporary_production_item_cleanup_failed_code={}",
                        error.code()
                    );
                }
            }
        }
        fn validated_ledger(path: &Path) -> (PathBuf, Ledger, KeychainProvider) {
            let path = path.canonicalize().expect("probe ledger must exist");
            assert_eq!(path.file_name().and_then(|n| n.to_str()), Some(LEDGER));
            let folder = path.parent().expect("probe ledger folder");
            assert_eq!(folder.parent(), Some(Path::new("/private/tmp")));
            assert!(
                folder
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(PREFIX))
            );
            let metadata = std::fs::symlink_metadata(folder).expect("probe folder metadata");
            assert!(
                metadata.is_dir() && metadata.mode() & 0o077 == 0,
                "probe folder must be owner-private"
            );
            assert!(std::fs::metadata(&path).unwrap().len() <= 4096);
            let ledger: Ledger = serde_json::from_slice(&std::fs::read(&path).unwrap())
                .expect("valid scoped probe ledger");
            assert_eq!(ledger.service, SERVICE);
            assert!(
                matches!(
                    ledger.phase.as_str(),
                    "preflight_absent" | "created" | "cleaned"
                ),
                "cleanup requires a completed absent-item preflight"
            );
            let data_root = folder
                .join("data")
                .canonicalize()
                .expect("owned probe data root");
            assert_eq!(
                data_root,
                folder.join("data"),
                "the probe data root must be its own regular child directory"
            );
            assert!(std::fs::symlink_metadata(&data_root).unwrap().is_dir());
            assert_eq!(ledger.canonical_data_root, data_root);
            let provider = KeychainProvider::new(&data_root).unwrap();
            assert_eq!(
                ledger.account, provider.account,
                "cleanup account must derive from this exact owned root"
            );
            (path, ledger, provider)
        }

        #[test]
        fn recovery_ledger_validation_rejects_unowned_or_escaped_scope_without_os_access() {
            let folder = tempfile::Builder::new()
                .prefix(PREFIX)
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir_in("/private/tmp")
                .unwrap();
            let data_root = folder.path().join("data");
            std::fs::create_dir(&data_root).unwrap();
            let provider = KeychainProvider::new(&data_root).unwrap();
            let path = folder.path().join(LEDGER);
            let mut ledger = Ledger {
                service: SERVICE.into(),
                account: provider.account.clone(),
                canonical_data_root: data_root.canonicalize().unwrap(),
                phase: "preflight_absent".into(),
            };
            record(&path, &ledger);
            let _ = validated_ledger(&path);
            ledger.phase = "prepared".into();
            record(&path, &ledger);
            assert!(std::panic::catch_unwind(|| validated_ledger(&path)).is_err());
            ledger.phase = "created".into();
            ledger.service = "wrong-service".into();
            record(&path, &ledger);
            assert!(std::panic::catch_unwind(|| validated_ledger(&path)).is_err());
            ledger.service = SERVICE.into();
            ledger.account = "wrong-account".into();
            record(&path, &ledger);
            assert!(std::panic::catch_unwind(|| validated_ledger(&path)).is_err());
            ledger.account = provider.account.clone();
            let moved = folder.path().join("other-data");
            std::fs::rename(&data_root, &moved).unwrap();
            std::os::unix::fs::symlink(&moved, &data_root).unwrap();
            // Even a self-consistent forged account/root cannot turn recovery
            // into a delete for a different directory reached through a link.
            ledger.canonical_data_root = moved.canonicalize().unwrap();
            ledger.account = KeychainProvider::new(&moved).unwrap().account;
            record(&path, &ledger);
            assert!(std::panic::catch_unwind(|| validated_ledger(&path)).is_err());
        }

        /// This is deliberately distinct from the private-Keychain primitive
        /// test: it exercises the unchanged production default-scope provider.
        /// Run only in a bounded child after compilation and inspect its ledger
        /// if a parent timeout interrupts OS access. It never changes UI policy.
        #[test]
        #[ignore = "explicit opt-in: creates and deletes one unique production-namespace test key"]
        fn production_native_keychain_provider_roundtrip_is_add_only_and_scoped() {
            assert_eq!(
                std::env::var("VANTRANSCRIBE_TEST_PRODUCTION_KEYCHAIN").as_deref(),
                Ok("1")
            );
            let folder = tempfile::Builder::new()
                .prefix(PREFIX)
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir_in("/private/tmp")
                .unwrap();
            // Keep the folder until verified cleanup; a forced interruption must
            // leave a durable exact-account ledger for the same binary to use.
            let folder = folder.keep();
            let data_root = folder.join("data");
            std::fs::create_dir(&data_root).unwrap();
            let provider = KeychainProvider::new(&data_root).unwrap();
            let path = folder.join(LEDGER);
            let mut ledger = Ledger {
                service: SERVICE.into(),
                account: provider.account.clone(),
                canonical_data_root: data_root.canonicalize().unwrap(),
                phase: "prepared".into(),
            };
            record(&path, &ledger);
            eprintln!("production_keychain_probe_ledger={}", path.display());
            let mut guard = OwnedItem {
                account: provider.account.clone(),
                owned: false,
            };
            assert!(
                provider
                    .load()
                    .expect("unique-account preflight failed")
                    .is_none(),
                "a preexisting item must never be modified or owned by this test"
            );
            ledger.phase = "preflight_absent".into();
            record(&path, &ledger);
            let created = provider
                .create()
                .expect("production Keychain test-key creation failed");
            guard.owned = true;
            ledger.phase = "created".into();
            record(&path, &ledger);
            let loaded = provider
                .load()
                .expect("production Keychain read failed")
                .expect("created test key missing");
            assert!(
                created.as_ref() == loaded.as_ref(),
                "production roundtrip changed test key"
            );
            let repeated = provider.create().expect("duplicate create failed");
            assert!(
                created.as_ref() == repeated.as_ref(),
                "duplicate create replaced the existing test key"
            );
            let canonical = KeychainProvider::new(&data_root.join(".")).unwrap();
            let from_second = canonical
                .load()
                .expect("second provider read failed")
                .expect("second provider cannot find test key");
            assert!(
                created.as_ref() == from_second.as_ref(),
                "canonical second provider changed test key"
            );
            let restarted = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--ignored", "--exact", "keys::tests::native_default_probe::read_owned_production_probe_from_second_process"])
                .env("VANTRANSCRIBE_TEST_PRODUCTION_KEYCHAIN", "1")
                .env("VANTRANSCRIBE_KEYCHAIN_PROBE_LEDGER", &path)
                .output()
                .expect("second-process probe did not start");
            assert!(
                restarted.status.success(),
                "second-process production Keychain read failed"
            );
            let child_report = String::from_utf8_lossy(&restarted.stdout);
            assert!(
                child_report.contains("read_owned_production_probe_from_second_process ... ok")
                    && child_report.contains("1 passed"),
                "second-process probe must execute its exact test"
            );
            let unrelated_root = folder.join("other-data");
            std::fs::create_dir(&unrelated_root).unwrap();
            let unrelated = KeychainProvider::new(&unrelated_root).unwrap();
            assert!(
                unrelated
                    .load()
                    .expect("unrelated root lookup failed")
                    .is_none(),
                "unrelated root must not find the test key"
            );
            delete_owned(&provider.account).expect("exact owned test-key deletion failed");
            guard.owned = false;
            assert!(
                provider
                    .load()
                    .expect("post-delete lookup failed")
                    .is_none(),
                "test key must be absent after cleanup"
            );
            ledger.phase = "cleaned".into();
            record(&path, &ledger);
            drop(guard);
            std::fs::remove_dir_all(folder).expect("owned probe folder cleanup failed");
        }

        #[test]
        #[ignore = "child-only scoped read of this probe's newly created temporary key"]
        fn read_owned_production_probe_from_second_process() {
            assert_eq!(
                std::env::var("VANTRANSCRIBE_TEST_PRODUCTION_KEYCHAIN").as_deref(),
                Ok("1")
            );
            let input = std::env::var_os("VANTRANSCRIBE_KEYCHAIN_PROBE_LEDGER")
                .expect("provide the exact owned probe ledger");
            let (_, ledger, provider) = validated_ledger(Path::new(&input));
            assert_eq!(ledger.phase, "created");
            let key = provider
                .load()
                .expect("second-process scoped read failed")
                .expect("owned test key missing");
            assert_eq!(key.len(), 32);
        }

        /// Exact-scope recovery for the same compiled/signed binary if its
        /// bounded parent interrupted the production probe. No key bytes read.
        #[test]
        #[ignore = "explicit recovery: deletes only the test item identified by an owned probe ledger"]
        fn cleanup_interrupted_production_keychain_probe() {
            assert_eq!(
                std::env::var("VANTRANSCRIBE_TEST_PRODUCTION_KEYCHAIN").as_deref(),
                Ok("1")
            );
            let input = std::env::var_os("VANTRANSCRIBE_KEYCHAIN_PROBE_LEDGER")
                .expect("provide the exact owned probe ledger");
            let (path, mut ledger, provider) = validated_ledger(Path::new(&input));
            // No secret read is needed for deletion or absence proof.
            use security_framework::item::{ItemClass, ItemSearchOptions};
            let mut query = ItemSearchOptions::new();
            query
                .class(ItemClass::generic_password())
                .service(SERVICE)
                .account(&provider.account)
                .skip_authenticated_items(true);
            delete_owned(&provider.account).expect("exact interrupted test-key deletion failed");
            match query.search() {
                Err(error) if error.code() == -25300 => {}
                _ => panic!("interrupted test item remains or cannot be checked"),
            }
            ledger.phase = "cleaned".into();
            record(&path, &ledger);
            std::fs::remove_dir_all(path.parent().unwrap())
                .expect("owned recovery folder cleanup failed");
        }
    }

    #[cfg(target_os = "macos")]
    mod private_locked_probe {
        use super::*;
        use core_foundation::data::CFData;
        use security_framework::{
            item::{
                ItemAddOptions, ItemAddValue, ItemClass, ItemSearchOptions, Location, SearchResult,
            },
            os::macos::keychain::{CreateOptions, SecKeychain},
        };
        use std::os::unix::fs::PermissionsExt;

        fn query(keychain: &SecKeychain, account: &str, data: bool) -> ItemSearchOptions {
            let mut query = ItemSearchOptions::new();
            query
                .class(ItemClass::generic_password())
                .service(SERVICE)
                .account(account)
                .keychains(std::slice::from_ref(keychain))
                .load_data(data);
            // Deliberately do not skip authenticated items: the locked read
            // must expose the actual non-missing OS failure, not hide the item.
            query
        }
        fn add(
            keychain: &SecKeychain,
            account: &str,
            bytes: &[u8],
        ) -> security_framework::base::Result<()> {
            let mut options = ItemAddOptions::new(ItemAddValue::Data {
                class: ItemClass::generic_password(),
                data: CFData::from_buffer(bytes),
            });
            options
                .set_service(SERVICE)
                .set_account_name(account)
                .set_location(Location::FileKeychain(keychain.clone()));
            options.add()
        }
        fn read(keychain: &SecKeychain, account: &str) -> Zeroizing<Vec<u8>> {
            let mut results = query(keychain, account, true)
                .search()
                .expect("owned private-Keychain read failed");
            assert_eq!(results.len(), 1, "private-Keychain read must be exact");
            match results.remove(0) {
                SearchResult::Data(bytes) => Zeroizing::new(bytes),
                _ => panic!("private-Keychain query returned a non-data item"),
            }
        }
        struct OwnedPrivateItem {
            keychain: SecKeychain,
            password: Zeroizing<String>,
            account: String,
            owned: bool,
        }
        impl Drop for OwnedPrivateItem {
            fn drop(&mut self) {
                if self.owned {
                    // On a test panic, unlock only this newly created private
                    // file with its own random password before exact cleanup.
                    if let Err(error) = self.keychain.unlock(Some(&self.password)) {
                        eprintln!(
                            "private_locked_probe_cleanup_unlock_failed_code={}",
                            error.code()
                        );
                    }
                    if let Err(error) = query(&self.keychain, &self.account, false).delete() {
                        eprintln!(
                            "private_locked_probe_cleanup_delete_failed_code={}",
                            error.code()
                        );
                    }
                }
            }
        }

        /// Run only as an isolated exact-test child with a hard parent deadline.
        /// The user interaction flag is a process-library boolean (Apple OSS
        /// Globals.h/SecKeychain.cpp), not a persisted user Keychain policy.
        #[test]
        #[ignore = "explicit opt-in: locks only a new private temporary Keychain and restores it"]
        fn physical_private_keychain_lock_fails_closed_and_preserves_key() {
            assert_eq!(
                std::env::var("VANTRANSCRIBE_TEST_PRIVATE_KEYCHAIN_LOCKED").as_deref(),
                Ok("1")
            );
            let prior_interaction =
                SecKeychain::user_interaction_allowed().expect("read process-local UI policy");
            // The binding's RAII guard restores true, so create it ONLY if the
            // original value was true. An original false stays false untouched.
            let noninteractive = if prior_interaction {
                Some(
                    SecKeychain::disable_user_interaction()
                        .expect("disable probe-process Keychain UI"),
                )
            } else {
                None
            };
            assert!(!SecKeychain::user_interaction_allowed().unwrap());
            let folder = tempfile::Builder::new()
                .prefix("vantranscribe-locked-private-keychain-")
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir_in("/private/tmp")
                .unwrap();
            let data_root = folder.path().join("data");
            std::fs::create_dir(&data_root).unwrap();
            let account = KeychainProvider::new(&data_root).unwrap().account;
            let path = folder.path().join("voice-memory-lock-probe.keychain");
            assert!(!path.exists());
            std::fs::write(
                folder.path().join("probe-ownership.txt"),
                format!(
                    "service={SERVICE}\naccount={account}\nprivate_keychain={}\n",
                    path.display()
                ),
            )
            .unwrap();
            let password = Zeroizing::new(uuid::Uuid::new_v4().simple().to_string());
            let mut keychain = CreateOptions::new()
                .password(&password)
                .prompt_user(false)
                .create(&path)
                .expect("new private-Keychain creation");
            keychain
                .unlock(Some(&password))
                .expect("new private-Keychain unlock");
            let mut owned = OwnedPrivateItem {
                keychain,
                password,
                account,
                owned: false,
            };
            match query(&owned.keychain, &owned.account, false).search() {
                Err(error) if error.code() == -25300 => {}
                _ => panic!("private lock-probe item must start absent"),
            }
            let mut key = Zeroizing::new([0; 32]);
            getrandom::fill(key.as_mut()).expect("random lock-probe key");
            add(&owned.keychain, &owned.account, key.as_ref()).expect("private lock-probe add");
            owned.owned = true;
            assert!(
                read(&owned.keychain, &owned.account).as_slice() == key.as_ref(),
                "unlocked private key did not roundtrip"
            );
            let locked = std::process::Command::new("/usr/bin/security")
                .arg("lock-keychain")
                .arg(&path)
                .output()
                .expect("explicit private-Keychain lock command");
            assert!(
                locked.status.success(),
                "locking the exact owned private file failed"
            );
            let failure = match query(&owned.keychain, &owned.account, true).search() {
                Err(error) => error.code(),
                Ok(_) => panic!("a physically locked private-Keychain secret must not be returned"),
            };
            assert!(
                matches!(failure, -25308 | -25315 | -25293),
                "locked private key must report a non-missing interaction/authentication error"
            );
            eprintln!("private_locked_probe_native_error_code={failure}");
            let mut replacement = Zeroizing::new([0; 32]);
            getrandom::fill(replacement.as_mut()).expect("random replacement candidate");
            let refused = match add(&owned.keychain, &owned.account, replacement.as_ref()) {
                Err(error) => error.code(),
                Ok(()) => panic!("locked duplicate add must never overwrite the private key"),
            };
            assert!(
                matches!(refused, -25299 | -25308 | -25315 | -25293),
                "locked duplicate add must fail closed"
            );
            eprintln!("private_locked_probe_duplicate_error_code={refused}");
            owned
                .keychain
                .unlock(Some(&owned.password))
                .expect("unlock only the owned private file");
            assert!(
                read(&owned.keychain, &owned.account).as_slice() == key.as_ref(),
                "locked failure or duplicate add changed the original key"
            );
            query(&owned.keychain, &owned.account, false)
                .delete()
                .expect("delete exact owned private lock-probe key");
            owned.owned = false;
            match query(&owned.keychain, &owned.account, false).search() {
                Err(error) if error.code() == -25300 => {}
                _ => panic!("private lock-probe item must be absent after deletion"),
            }
            drop(owned);
            folder.close().expect("private lock-probe file cleanup");
            drop(noninteractive);
            assert_eq!(
                SecKeychain::user_interaction_allowed().unwrap(),
                prior_interaction,
                "restore the exact original probe-process UI policy"
            );
            eprintln!("private_locked_probe_process_interaction_restored=true");
        }
    }
}
