use super::*;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct FakeKeys {
    key: Mutex<Option<[u8; 32]>>,
    unavailable: bool,
    creates: AtomicUsize,
}
impl FakeKeys {
    fn new() -> Self {
        Self {
            key: Mutex::new(None),
            unavailable: false,
            creates: AtomicUsize::new(0),
        }
    }
    fn present(byte: u8) -> Self {
        Self {
            key: Mutex::new(Some([byte; 32])),
            unavailable: false,
            creates: AtomicUsize::new(0),
        }
    }
}
impl KeyProvider for FakeKeys {
    fn load(&self) -> Result<Option<SecretKey>> {
        if self.unavailable {
            return Err(MemoryError::KeyUnavailable);
        }
        Ok(self.key.lock().unwrap().map(Zeroizing::new))
    }
    fn create(&self) -> Result<SecretKey> {
        self.creates.fetch_add(1, Ordering::SeqCst);
        if self.unavailable {
            return Err(MemoryError::KeyUnavailable);
        }
        let mut key = self.key.lock().unwrap();
        Ok(Zeroizing::new(*key.get_or_insert([7; 32])))
    }
}
fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-01-31T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}
fn database() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    initialize(&db).unwrap();
    db
}
fn enabled(db: &Connection) {
    update_settings(
        db,
        &MemorySettings {
            enabled: true,
            retention_months: 12,
        },
        true,
        now(),
    )
    .unwrap()
}
fn request() -> Enrollment {
    Enrollment {
        display_name: "Fixture Person".into(),
        person_id: None,
        is_self: false,
        written_consent: true,
        consent_method: "Fixture written release".into(),
        attested_by: "Fixture operator".into(),
        channel_type: ChannelType::RemoteWideband,
        channel_provenance: "user_declared".into(),
        embedding_version: "a".repeat(64),
        clean_speech_ms: 20_000,
        shared_room: false,
        embedding: vec![0.2, 0.3, 0.4, 0.5],
    }
}
fn counts(db: &Connection) -> (i64, i64, i64) {
    (
        db.query_row("SELECT COUNT(*) FROM app_people", [], |r| r.get(0))
            .unwrap(),
        db.query_row("SELECT COUNT(*) FROM app_biometric_consents", [], |r| {
            r.get(0)
        })
        .unwrap(),
        db.query_row("SELECT COUNT(*) FROM app_voiceprints", [], |r| r.get(0))
            .unwrap(),
    )
}
fn blob(db: &Connection, id: &str) -> Vec<u8> {
    db.query_row(
        "SELECT exemplars FROM app_voiceprints WHERE person_id=?1",
        [id],
        |r| r.get(0),
    )
    .unwrap()
}
fn decrypt_print(db: &Connection, keys: &FakeKeys, person: &str) -> Vec<Exemplar> {
    let (id,channel,provenance,version,blob,n):(String,String,String,String,Vec<u8>,u32)=db.query_row("SELECT id,channel_type,channel_provenance,embedding_version,exemplars,n_exemplars FROM app_voiceprints WHERE person_id=?1",[person],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).unwrap();
    let plain = crypto::decrypt(
        &keys.load().unwrap().unwrap(),
        &blob,
        &aad(&id, person, &channel, &provenance, &version),
    )
    .unwrap();
    decode(&plain, n).unwrap()
}
#[test]
fn migration_and_metadata_are_default_off_and_never_create_keys() {
    let db = database();
    initialize(&db).unwrap();
    assert_eq!(settings(&db).unwrap(), MemorySettings::default());
    let before: Option<Vec<u8>> = db
        .query_row("SELECT key_check FROM app_voice_memory_settings", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(before.is_none());
    let state = status(&db, None).unwrap();
    assert!(!state.enabled);
    assert!(!state.matching_available);
    assert!(state.people.is_empty());
    assert_eq!(counts(&db), (0, 0, 0));
    assert!(
        update_settings(
            &db,
            &MemorySettings {
                enabled: true,
                retention_months: 12
            },
            false,
            now()
        )
        .is_err()
    );
    assert!(!settings(&db).unwrap().enabled);
    for retention in [0, 25, u32::MAX] {
        assert!(
            update_settings(
                &db,
                &MemorySettings {
                    enabled: false,
                    retention_months: retention
                },
                false,
                now()
            )
            .is_err()
        );
    }
}
#[test]
fn consent_clean_speech_shared_room_and_hash_are_required_before_key_access() {
    let db = database();
    let keys = FakeKeys::new();
    assert!(matches!(
        enroll(&db, &keys, &request(), now()),
        Err(MemoryError::Disabled)
    ));
    enabled(&db);
    let mut value = request();
    value.written_consent = false;
    assert!(enroll(&db, &keys, &value, now()).is_err());
    value.written_consent = true;
    value.clean_speech_ms = 19_999;
    assert!(enroll(&db, &keys, &value, now()).is_err());
    value.clean_speech_ms = 20_000;
    value.shared_room = true;
    assert!(enroll(&db, &keys, &value, now()).is_err());
    value.shared_room = false;
    value.consent_method = " ".into();
    assert!(enroll(&db, &keys, &value, now()).is_err());
    value.consent_method = "Fixture release".into();
    value.embedding_version = "legacy-model".into();
    assert!(enroll(&db, &keys, &value, now()).is_err());
    value.embedding_version = "a".repeat(64);
    value.channel_provenance = "guessed".into();
    assert!(enroll(&db, &keys, &value, now()).is_err());
    assert_eq!(keys.creates.load(Ordering::SeqCst), 0);
    assert_eq!(counts(&db), (0, 0, 0));
}
#[test]
fn encrypted_restart_roundtrip_retains_consent_without_exposing_embeddings() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("memory.db");
    let keys = FakeKeys::new();
    let db = Connection::open(&path).unwrap();
    initialize(&db).unwrap();
    enabled(&db);
    let value = request();
    let person = enroll(&db, &keys, &value, now()).unwrap();
    let samples = decrypt_print(&db, &keys, &person.id);
    assert_eq!(samples.len(), 1);
    let norm = samples[0]
        .vector
        .iter()
        .map(|v| f64::from(*v).powi(2))
        .sum::<f64>();
    assert!((norm - 1.).abs() < 0.00001);
    let serialized =
        serde_json::to_string(&status(&db, Some(&value.embedding_version)).unwrap()).unwrap();
    assert!(!serialized.contains("\"exemplars\":"));
    assert!(!serialized.contains("embedding\":"));
    assert!(!person.voiceprints[0].stale);
    assert_eq!(person.consent.attested_by, "Fixture operator");
    let plain = encode(&samples);
    let cipher = blob(&db, &person.id);
    assert!(!cipher.windows(plain.len()).any(|v| v == plain.as_slice()));
    drop(db);
    let file = std::fs::read(&path).unwrap();
    assert!(!file.windows(plain.len()).any(|v| v == plain.as_slice()));
    let reopened = Connection::open(path).unwrap();
    initialize(&reopened).unwrap();
    assert_eq!(
        status(&reopened, Some(&value.embedding_version))
            .unwrap()
            .people[0],
        person
    );
    assert_eq!(decrypt_print(&reopened, &keys, &person.id).len(), 1);
}
#[test]
fn missing_locked_and_wrong_keys_never_recreate_or_modify_existing_data() {
    let db = database();
    enabled(&db);
    let keys = FakeKeys::new();
    let person = enroll(&db, &keys, &request(), now()).unwrap();
    let before = blob(&db, &person.id);
    let absent = FakeKeys::new();
    let mut value = request();
    value.person_id = Some(person.id.clone());
    assert!(matches!(
        enroll(&db, &absent, &value, now()),
        Err(MemoryError::KeyUnavailable)
    ));
    assert_eq!(absent.creates.load(Ordering::SeqCst), 0);
    let locked = FakeKeys {
        unavailable: true,
        ..FakeKeys::new()
    };
    assert!(matches!(
        enroll(&db, &locked, &value, now()),
        Err(MemoryError::KeyUnavailable)
    ));
    assert_eq!(locked.creates.load(Ordering::SeqCst), 0);
    let wrong = FakeKeys::present(9);
    assert!(matches!(
        enroll(&db, &wrong, &value, now()),
        Err(MemoryError::Authentication)
    ));
    value.person_id = None;
    assert!(matches!(
        enroll(&db, &wrong, &value, now()),
        Err(MemoryError::Authentication)
    ));
    assert_eq!(blob(&db, &person.id), before);
    assert_eq!(counts(&db), (1, 1, 1));
    // Privacy deletion still works when the biometric key is missing or locked.
    assert!(forget_person(&db, &person.id).unwrap());
    assert_eq!(counts(&db), (0, 0, 0));
}
#[test]
fn blob_tampering_and_row_swaps_are_authenticated() {
    let db = database();
    enabled(&db);
    let keys = FakeKeys::new();
    let first = enroll(&db, &keys, &request(), now()).unwrap();
    let second = enroll(&db, &keys, &request(), now()).unwrap();
    let original = blob(&db, &first.id);
    let other = blob(&db, &second.id);
    assert_ne!(original, other);
    db.execute(
        "UPDATE app_voiceprints SET exemplars=?1 WHERE person_id=?2",
        params![other, first.id],
    )
    .unwrap();
    let mut value = request();
    value.person_id = Some(first.id.clone());
    assert!(matches!(
        enroll(&db, &keys, &value, now()),
        Err(MemoryError::Authentication)
    ));
    assert_eq!(counts(&db), (2, 2, 2));
    let mut changed = original.clone();
    let i = changed.len() - 1;
    changed[i] ^= 1;
    db.execute(
        "UPDATE app_voiceprints SET exemplars=?1 WHERE person_id=?2",
        params![changed, first.id],
    )
    .unwrap();
    assert!(matches!(
        enroll(&db, &keys, &value, now()),
        Err(MemoryError::Authentication)
    ));
    let key = keys.load().unwrap().unwrap();
    let print = &first.voiceprints[0];
    for wrong in [
        aad(
            "different-row",
            &first.id,
            &print.channel_type,
            &print.channel_provenance,
            &print.embedding_version,
        ),
        aad(
            &print.id,
            "different-person",
            &print.channel_type,
            &print.channel_provenance,
            &print.embedding_version,
        ),
        aad(
            &print.id,
            &first.id,
            "in_person",
            &print.channel_provenance,
            &print.embedding_version,
        ),
        aad(
            &print.id,
            &first.id,
            &print.channel_type,
            "meeting_mode",
            &print.embedding_version,
        ),
        aad(
            &print.id,
            &first.id,
            &print.channel_type,
            &print.channel_provenance,
            &"b".repeat(64),
        ),
    ] {
        assert!(matches!(
            crypto::decrypt(&key, &original, &wrong),
            Err(MemoryError::Authentication)
        ));
    }
}
#[test]
fn oldest_first_cap_and_confirmed_learning_refresh_expiry() {
    let db = database();
    enabled(&db);
    let keys = FakeKeys::new();
    let person = enroll(&db, &keys, &request(), now()).unwrap();
    let initial = blob(&db, &person.id);
    let mut value = request();
    value.person_id = Some(person.id.clone());
    for day in 1..=11 {
        enroll(&db, &keys, &value, now() + chrono::Duration::days(day)).unwrap();
    }
    let samples = decrypt_print(&db, &keys, &person.id);
    assert_eq!(samples.len(), 10);
    assert_eq!(
        samples[0].created,
        (now() + chrono::Duration::days(2)).timestamp_millis()
    );
    assert_eq!(
        samples[9].created,
        (now() + chrono::Duration::days(11)).timestamp_millis()
    );
    assert_ne!(initial, blob(&db, &person.id));
    let state = status(&db, Some(&value.embedding_version)).unwrap();
    assert_eq!(state.people[0].voiceprints[0].n_exemplars, 10);
    assert_eq!(
        state.people[0].voiceprints[0].expires_at,
        expiry(now() + chrono::Duration::days(11), 12).unwrap()
    );
}
#[test]
fn versions_and_channel_types_are_isolated_and_stale_is_explicit() {
    let db = database();
    enabled(&db);
    let keys = FakeKeys::new();
    let person = enroll(&db, &keys, &request(), now()).unwrap();
    let mut value = request();
    value.person_id = Some(person.id.clone());
    value.embedding_version = "b".repeat(64);
    enroll(&db, &keys, &value, now()).unwrap();
    value.channel_type = ChannelType::InPerson;
    value.channel_provenance = "meeting_mode".into();
    enroll(&db, &keys, &value, now()).unwrap();
    let state = status(&db, Some(&"b".repeat(64))).unwrap();
    assert_eq!(state.people[0].voiceprints.len(), 3);
    assert_eq!(
        state.people[0]
            .voiceprints
            .iter()
            .filter(|v| v.stale)
            .count(),
        1
    );
    assert!(!state.matching_available);
    let mut bad = request();
    bad.embedding = vec![f32::NAN, 0.];
    assert!(enroll(&db, &keys, &bad, now()).is_err());
    bad.embedding = vec![0., 0.];
    assert!(enroll(&db, &keys, &bad, now()).is_err());
    assert_eq!(counts(&db), (1, 3, 3));
}
#[test]
fn expiry_uses_calendar_months_and_setting_changes_cannot_resurrect_expired_blobs() {
    let db = database();
    enabled(&db);
    let keys = FakeKeys::new();
    enroll(&db, &keys, &request(), now()).unwrap();
    update_settings(
        &db,
        &MemorySettings {
            enabled: false,
            retention_months: 1,
        },
        false,
        now(),
    )
    .unwrap();
    let end = DateTime::parse_from_rfc3339("2026-02-28T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    assert_eq!(
        status(&db, None).unwrap().people[0].voiceprints[0].expires_at,
        timestamp(end)
    );
    assert_eq!(
        expire(&db, end - chrono::Duration::milliseconds(1)).unwrap(),
        0
    );
    assert_eq!(expire(&db, end).unwrap(), 1);
    assert_eq!(counts(&db), (1, 1, 0));
    update_settings(
        &db,
        &MemorySettings {
            enabled: false,
            retention_months: 24,
        },
        false,
        end,
    )
    .unwrap();
    assert!(status(&db, None).unwrap().people[0].voiceprints.is_empty());
    assert_eq!(delete_all(&db).unwrap(), 1);
    assert_eq!(counts(&db), (0, 0, 0));
    assert_eq!(delete_all(&db).unwrap(), 0);
}
#[test]
fn failed_publication_rolls_back_people_consent_keycheck_and_exemplars() {
    let db = database();
    enabled(&db);
    let keys = FakeKeys::new();
    db.execute_batch("CREATE TRIGGER fail_voiceprint BEFORE INSERT ON app_voiceprints BEGIN SELECT RAISE(ABORT,'fixture'); END").unwrap();
    assert!(enroll(&db, &keys, &request(), now()).is_err());
    assert_eq!(counts(&db), (0, 0, 0));
    let check: Option<Vec<u8>> = db
        .query_row("SELECT key_check FROM app_voice_memory_settings", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(check.is_none());
    db.execute_batch("DROP TRIGGER fail_voiceprint").unwrap();
    let person = enroll(&db, &keys, &request(), now()).unwrap();
    db.execute_batch("CREATE TRIGGER fail_update BEFORE UPDATE ON app_voiceprints BEGIN SELECT RAISE(ABORT,'fixture'); END").unwrap();
    let mut value = request();
    value.person_id = Some(person.id.clone());
    value.display_name = "Changed fixture".into();
    assert!(enroll(&db, &keys, &value, now()).is_err());
    assert_eq!(counts(&db), (1, 1, 1));
    assert_eq!(
        status(&db, None).unwrap().people[0].display_name,
        person.display_name
    );
}
#[test]
fn caller_transaction_rolls_back_successful_enrollment_and_person_deletion() {
    let mut db = database();
    enabled(&db);
    let keys = FakeKeys::new();
    {
        let tx = db.transaction().unwrap();
        enroll(&tx, &keys, &request(), now()).unwrap();
        assert_eq!(counts(&tx), (1, 1, 1));
    }
    assert_eq!(counts(&db), (0, 0, 0));
    let person = enroll(&db, &keys, &request(), now()).unwrap();
    {
        let tx = db.transaction().unwrap();
        assert!(forget_person(&tx, &person.id).unwrap());
        assert_eq!(counts(&tx), (0, 0, 0));
    }
    assert_eq!(counts(&db), (1, 1, 1));
}
#[test]
fn explicit_self_consent_cannot_change_existing_person_kind_or_duplicate_self() {
    let db = database();
    enabled(&db);
    let keys = FakeKeys::new();
    let mut value = request();
    value.is_self = true;
    let person = enroll(&db, &keys, &value, now()).unwrap();
    assert!(person.is_self);
    assert!(enroll(&db, &keys, &value, now()).is_err());
    value.person_id = Some(person.id.clone());
    value.is_self = false;
    assert!(enroll(&db, &keys, &value, now()).is_err());
    value.person_id = Some("missing".into());
    assert!(matches!(
        enroll(&db, &keys, &value, now()),
        Err(MemoryError::MissingPerson)
    ));
    assert_eq!(counts(&db), (1, 1, 1));
}

#[test]
fn explicit_delete_all_recovers_a_genuinely_lost_key_only_after_destroying_all_prints() {
    let db = database();
    enabled(&db);
    let original = FakeKeys::new();
    enroll(&db, &original, &request(), now()).unwrap();
    let replacement = FakeKeys::new();
    assert!(matches!(
        enroll(&db, &replacement, &request(), now()),
        Err(MemoryError::KeyUnavailable)
    ));
    assert_eq!(replacement.creates.load(Ordering::SeqCst), 0);
    assert_eq!(delete_all(&db).unwrap(), 1);
    assert_eq!(counts(&db), (0, 0, 0));
    let person = enroll(&db, &replacement, &request(), now()).unwrap();
    assert_eq!(replacement.creates.load(Ordering::SeqCst), 1);
    assert_eq!(decrypt_print(&db, &replacement, &person.id).len(), 1);
}
