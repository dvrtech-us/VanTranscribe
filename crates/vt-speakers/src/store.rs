use crate::{
    ChannelType, ConsentInfo, Enrollment, KeyProvider, MemoryError, MemorySettings, MemoryStatus,
    PersonInfo, Result, SecretKey, VoiceprintInfo, crypto,
};
use chrono::{DateTime, Months, SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use zeroize::Zeroizing;

const SCHEMA: &str =
    include_str!("../../../documentation/features/speakers/voice-memory/sql/001-voice-memory.sql");
const CHECK: &[u8] = b"VanTranscribe voice memory key check v1";

fn timestamp(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}
fn expiry(now: DateTime<Utc>, months: u32) -> Result<String> {
    Ok(timestamp(
        now.checked_add_months(Months::new(months))
            .ok_or(MemoryError::Invalid("The expiry date is out of range."))?,
    ))
}
fn atomic<T>(db: &Connection, f: impl FnOnce() -> Result<T>) -> Result<T> {
    db.execute_batch("SAVEPOINT vt_voice_memory_mutation")?;
    match f() {
        Ok(value) => {
            db.execute_batch("RELEASE vt_voice_memory_mutation")?;
            Ok(value)
        }
        Err(error) => {
            let _ = db.execute_batch(
                "ROLLBACK TO vt_voice_memory_mutation; RELEASE vt_voice_memory_mutation",
            );
            Err(error)
        }
    }
}
/// Schema initialization does not access Keychain or create a key.
pub fn initialize(db: &Connection) -> Result<()> {
    atomic(db, || {
        db.execute_batch(SCHEMA)?;
        db.execute(
            "INSERT OR IGNORE INTO app_voice_memory_settings(singleton,store_id) VALUES(1,?1)",
            [uuid::Uuid::new_v4().to_string()],
        )?;
        Ok(())
    })
}
pub fn settings(db: &Connection) -> Result<MemorySettings> {
    Ok(db.query_row(
        "SELECT enabled,retention_months FROM app_voice_memory_settings WHERE singleton=1",
        [],
        |row| {
            Ok(MemorySettings {
                enabled: row.get(0)?,
                retention_months: row.get(1)?,
            })
        },
    )?)
}
/// Enabling is an explicit explanation acknowledgment, independent from any
/// participant's consent. Setting changes never access OS keys.
pub fn update_settings(
    db: &Connection,
    value: &MemorySettings,
    explanation_acknowledged: bool,
    now: DateTime<Utc>,
) -> Result<()> {
    if !(1..=24).contains(&value.retention_months) {
        return Err(MemoryError::Invalid(
            "Voiceprint retention must be between 1 and 24 months.",
        ));
    }
    atomic(db, || {
        let previous = settings(db)?;
        if value.enabled && !previous.enabled && !explanation_acknowledged {
            return Err(MemoryError::Invalid(
                "Acknowledge the biometric explanation before enabling speaker memory.",
            ));
        }
        expire(db, now)?;
        db.execute("UPDATE app_voice_memory_settings SET enabled=?1,retention_months=?2,explanation_acknowledged_at=CASE WHEN ?3 THEN ?4 ELSE explanation_acknowledged_at END WHERE singleton=1", params![value.enabled,value.retention_months,value.enabled && explanation_acknowledged,timestamp(now)])?;
        if value.retention_months != previous.retention_months {
            let rows = db
                .prepare("SELECT id,last_matched_at FROM app_voiceprints")?
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for (id, last) in rows {
                let last = DateTime::parse_from_rfc3339(&last)
                    .map_err(|_| MemoryError::Authentication)?
                    .with_timezone(&Utc);
                db.execute(
                    "UPDATE app_voiceprints SET expires_at=?1 WHERE id=?2",
                    params![expiry(last, value.retention_months)?, id],
                )?;
            }
            expire(db, now)?;
        }
        Ok(())
    })
}
/// Deletes only expired biometric blobs. Named person records and their consent
/// history remain until Forget/Delete all. No key access is required.
pub fn expire(db: &Connection, now: DateTime<Utc>) -> Result<usize> {
    Ok(db.execute(
        "DELETE FROM app_voiceprints WHERE expires_at<=?1",
        [timestamp(now)],
    )?)
}
/// Explicit deletion does not need a readable/unlocked biometric key. Session
/// callers clear person links in the same outer transaction.
pub fn forget_person(db: &Connection, id: &str) -> Result<bool> {
    atomic(db, || {
        db.execute("DELETE FROM app_voiceprints WHERE person_id=?1", [id])?;
        db.execute(
            "DELETE FROM app_biometric_consents WHERE person_id=?1",
            [id],
        )?;
        Ok(db.execute("DELETE FROM app_people WHERE id=?1", [id])? > 0)
    })
}
pub fn delete_all(db: &Connection) -> Result<usize> {
    atomic(db, || {
        db.execute("DELETE FROM app_voiceprints", [])?;
        db.execute("DELETE FROM app_biometric_consents", [])?;
        let count = db.execute("DELETE FROM app_people", [])?;
        // All biometric ciphertext is now gone, so an explicit Delete all can
        // recover from a genuinely lost key without risking existing prints.
        db.execute(
            "UPDATE app_voice_memory_settings SET key_check=NULL,store_id=?1 WHERE singleton=1",
            [uuid::Uuid::new_v4().to_string()],
        )?;
        Ok(count)
    })
}
fn people(db: &Connection, current: Option<&str>) -> Result<Vec<PersonInfo>> {
    let persons = db
        .prepare("SELECT id,display_name,is_self FROM app_people ORDER BY created_at,id")?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    persons.into_iter().map(|(id,display_name,is_self)| {
        let consent = db.query_row("SELECT attested_at,method,attested_by FROM app_biometric_consents WHERE person_id=?1 AND revoked_at IS NULL ORDER BY attested_at DESC,id DESC LIMIT 1",[&id],|row| Ok(ConsentInfo { attested_at:row.get(0)?,method:row.get(1)?,attested_by:row.get(2)? }))?;
        let voiceprints = db.prepare("SELECT id,channel_type,channel_provenance,embedding_version,n_exemplars,last_matched_at,expires_at FROM app_voiceprints WHERE person_id=?1 ORDER BY channel_type,embedding_version")?.query_map([&id], |row| {
            let version:String=row.get(3)?;
            Ok(VoiceprintInfo { id:row.get(0)?,channel_type:row.get(1)?,channel_provenance:row.get(2)?,stale:current != Some(version.as_str()),embedding_version:version,n_exemplars:row.get(4)?,last_matched_at:row.get(5)?,expires_at:row.get(6)? })
        })?.collect::<std::result::Result<Vec<_>,_>>()?;
        Ok(PersonInfo { id,display_name,is_self,voiceprints,consent })
    }).collect()
}
/// Metadata-only. `protected` means a verifier exists, not that the OS secret is
/// currently readable. Automatic recognition is unavailable without calibration.
pub fn status(db: &Connection, current_embedding_version: Option<&str>) -> Result<MemoryStatus> {
    let value = settings(db)?;
    let protected: bool = db.query_row(
        "SELECT key_check IS NOT NULL FROM app_voice_memory_settings WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    Ok(MemoryStatus {
        enabled: value.enabled,
        retention_months: value.retention_months,
        matching_available: false,
        key_status: if cfg!(target_os = "macos") {
            if protected {
                "protected"
            } else {
                "not_created"
            }
        } else {
            "unavailable_platform"
        }
        .into(),
        people: people(db, current_embedding_version)?,
    })
}

fn check_aad(store_id: &str) -> Vec<u8> {
    format!("vt-speakers/key-check/v1/{store_id}").into_bytes()
}
fn key(db: &Connection, provider: &dyn KeyProvider) -> Result<SecretKey> {
    let (store_id, check): (String, Option<Vec<u8>>) = db.query_row(
        "SELECT store_id,key_check FROM app_voice_memory_settings WHERE singleton=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let loaded = provider.load()?;
    if let Some(check) = check {
        let key = loaded.ok_or(MemoryError::KeyUnavailable)?;
        if crypto::decrypt(&key, &check, &check_aad(&store_id))?.as_slice() != CHECK {
            return Err(MemoryError::Authentication);
        }
        return Ok(key);
    }
    let count: i64 = db.query_row("SELECT COUNT(*) FROM app_voiceprints", [], |row| row.get(0))?;
    if count != 0 {
        return Err(MemoryError::Authentication);
    }
    let key = match loaded {
        Some(key) => key,
        None => provider.create()?,
    };
    let check = crypto::encrypt(&key, CHECK, &check_aad(&store_id))?;
    db.execute(
        "UPDATE app_voice_memory_settings SET key_check=?1 WHERE singleton=1",
        [check],
    )?;
    Ok(key)
}
fn aad(id: &str, person: &str, channel: &str, provenance: &str, version: &str) -> Vec<u8> {
    // Length-delimited JSON strings prevent concatenation ambiguity.
    serde_json::to_vec(&(
        "vt-speakers/exemplars/v1",
        id,
        person,
        channel,
        provenance,
        version,
    ))
    .expect("serializing string tuple cannot fail")
}
fn normalized(value: &[f32]) -> Result<Zeroizing<Vec<f32>>> {
    if !(2..=1024).contains(&value.len()) || value.iter().any(|v| !v.is_finite()) {
        return Err(MemoryError::Invalid(
            "A valid bounded embedding is required.",
        ));
    }
    let norm = value
        .iter()
        .map(|v| f64::from(*v).powi(2))
        .sum::<f64>()
        .sqrt();
    if !norm.is_finite() || norm < 1e-12 {
        return Err(MemoryError::Invalid("A nonzero embedding is required."));
    }
    Ok(Zeroizing::new(
        value
            .iter()
            .map(|v| (f64::from(*v) / norm) as f32)
            .collect(),
    ))
}
struct Exemplar {
    created: i64,
    vector: Zeroizing<Vec<f32>>,
}
fn encode(exemplars: &[Exemplar]) -> Zeroizing<Vec<u8>> {
    let mut bytes = Zeroizing::new(Vec::new());
    bytes.push(1);
    bytes.extend_from_slice(&(exemplars[0].vector.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(exemplars.len() as u32).to_le_bytes());
    for exemplar in exemplars {
        bytes.extend_from_slice(&exemplar.created.to_le_bytes());
        for v in exemplar.vector.iter() {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
    }
    bytes
}
fn decode(bytes: &[u8], count: u32) -> Result<Vec<Exemplar>> {
    if bytes.len() < 9 || bytes[0] != 1 {
        return Err(MemoryError::Authentication);
    }
    let dimension = u32::from_le_bytes(
        bytes[1..5]
            .try_into()
            .map_err(|_| MemoryError::Authentication)?,
    ) as usize;
    let n = u32::from_le_bytes(
        bytes[5..9]
            .try_into()
            .map_err(|_| MemoryError::Authentication)?,
    ) as usize;
    if !(2..=1024).contains(&dimension)
        || !(1..=10).contains(&n)
        || n != count as usize
        || bytes.len() != 9 + n * (8 + dimension * 4)
    {
        return Err(MemoryError::Authentication);
    }
    let mut result = Vec::with_capacity(n);
    for record in bytes[9..].chunks_exact(8 + dimension * 4) {
        let created = i64::from_le_bytes(
            record[..8]
                .try_into()
                .map_err(|_| MemoryError::Authentication)?,
        );
        let vector = Zeroizing::new(
            record[8..]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|v| f32::from_le_bytes(*v))
                .collect::<Vec<_>>(),
        );
        let norm = vector.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>();
        if vector.iter().any(|v| !v.is_finite()) || (norm - 1.0).abs() > 0.001 {
            return Err(MemoryError::Authentication);
        }
        result.push(Exemplar { created, vector });
    }
    Ok(result)
}
fn text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
fn validate(value: &Enrollment) -> Result<()> {
    if !text(&value.display_name, 200) {
        return Err(MemoryError::Invalid(
            "Enter a person name of at most 200 bytes.",
        ));
    }
    if !value.written_consent {
        return Err(MemoryError::Invalid(
            "Confirm this person's written consent, or your own explicit consent, before saving their voice.",
        ));
    }
    if !text(&value.consent_method, 1000) || !text(&value.attested_by, 200) {
        return Err(MemoryError::Invalid(
            "Enter the consent method and the person making the attestation.",
        ));
    }
    if value.shared_room {
        return Err(MemoryError::Invalid(
            "Shared-room speakers cannot create or update voiceprints.",
        ));
    }
    if value.clean_speech_ms < 20_000 {
        return Err(MemoryError::Invalid(
            "At least 20 seconds of clean, non-overlapped refined speech is required.",
        ));
    }
    if value.embedding_version.len() != 64
        || !value
            .embedding_version
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(MemoryError::Invalid(
            "A complete embedding version hash is required.",
        ));
    }
    let expected = if value.channel_type == ChannelType::InPerson {
        "meeting_mode"
    } else {
        "user_declared"
    };
    if value.channel_provenance != expected {
        return Err(MemoryError::Invalid(
            "The voice channel provenance is unavailable.",
        ));
    }
    Ok(())
}
/// Creates/appends only after trusted source validation. All SQLite changes are
/// rolled back together on error, even if caller did not begin a transaction.
/// A key newly added to OS Keychain may remain after DB rollback; it contains no
/// biometric data and is safely reused on the next explicit enrollment.
pub fn enroll(
    db: &Connection,
    provider: &dyn KeyProvider,
    value: &Enrollment,
    now: DateTime<Utc>,
) -> Result<PersonInfo> {
    validate(value)?;
    let vector = normalized(&value.embedding)?;
    atomic(db, || {
        let prefs = settings(db)?;
        if !prefs.enabled {
            return Err(MemoryError::Disabled);
        }
        let acknowledged:bool=db.query_row("SELECT explanation_acknowledged_at IS NOT NULL FROM app_voice_memory_settings WHERE singleton=1",[],|row|row.get(0))?;
        if !acknowledged {
            return Err(MemoryError::Disabled);
        }
        expire(db, now)?;
        let person = if let Some(id) = &value.person_id {
            let is_self: Option<bool> = db
                .query_row("SELECT is_self FROM app_people WHERE id=?1", [id], |row| {
                    row.get(0)
                })
                .optional()?;
            if is_self != Some(value.is_self) {
                return Err(if is_self.is_none() {
                    MemoryError::MissingPerson
                } else {
                    MemoryError::Invalid(
                        "An existing person's self identity cannot be changed by voice enrollment.",
                    )
                });
            }
            id.clone()
        } else {
            if value.is_self
                && db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM app_people WHERE is_self=1)",
                    [],
                    |row| row.get::<_, bool>(0),
                )?
            {
                return Err(MemoryError::Invalid(
                    "Your own person entry already exists. Select it to add another voice sample.",
                ));
            }
            uuid::Uuid::new_v4().to_string()
        };
        let key = key(db, provider)?;
        let saved:Option<(String,String,Vec<u8>,u32)>=db.query_row("SELECT id,channel_provenance,exemplars,n_exemplars FROM app_voiceprints WHERE person_id=?1 AND channel_type=?2 AND embedding_version=?3",params![person,value.channel_type.as_str(),value.embedding_version],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?;
        let (id, mut exemplars) = if let Some((id, provenance, blob, count)) = saved {
            if provenance != value.channel_provenance {
                return Err(MemoryError::Authentication);
            }
            let plain = crypto::decrypt(
                &key,
                &blob,
                &aad(
                    &id,
                    &person,
                    value.channel_type.as_str(),
                    &provenance,
                    &value.embedding_version,
                ),
            )?;
            let exemplars = decode(&plain, count)?;
            if exemplars[0].vector.len() != vector.len() {
                return Err(MemoryError::Invalid(
                    "The embedding dimension does not match this version.",
                ));
            }
            (id, exemplars)
        } else {
            (uuid::Uuid::new_v4().to_string(), Vec::new())
        };
        // A user-confirmed enrollment refreshes the exemplar. No unconfirmed or
        // automatic match ever calls this path. Stable ordering evicts oldest.
        exemplars.push(Exemplar {
            created: now.timestamp_millis(),
            vector,
        });
        exemplars.sort_by_key(|v| v.created);
        while exemplars.len() > 10 {
            exemplars.remove(0);
        }
        let blob = crypto::encrypt(
            &key,
            &encode(&exemplars),
            &aad(
                &id,
                &person,
                value.channel_type.as_str(),
                &value.channel_provenance,
                &value.embedding_version,
            ),
        )?;
        let when = timestamp(now);
        db.execute("INSERT INTO app_people(id,display_name,is_self,created_at) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET display_name=excluded.display_name",params![person,value.display_name.trim(),value.is_self,when])?;
        db.execute("INSERT INTO app_biometric_consents(id,person_id,attested_at,method,attested_by,is_self) VALUES(?1,?2,?3,?4,?5,?6)",params![uuid::Uuid::new_v4().to_string(),person,when,value.consent_method.trim(),value.attested_by.trim(),value.is_self])?;
        db.execute("INSERT INTO app_voiceprints(id,person_id,channel_type,channel_provenance,embedding_version,exemplars,n_exemplars,last_matched_at,expires_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?8) ON CONFLICT(person_id,channel_type,embedding_version) DO UPDATE SET exemplars=excluded.exemplars,n_exemplars=excluded.n_exemplars,last_matched_at=excluded.last_matched_at,expires_at=excluded.expires_at,updated_at=excluded.updated_at",params![id,person,value.channel_type.as_str(),value.channel_provenance,value.embedding_version,blob,exemplars.len() as u32,when,expiry(now,prefs.retention_months)?])?;
        people(db, Some(&value.embedding_version))?
            .into_iter()
            .find(|p| p.id == person)
            .ok_or(MemoryError::MissingPerson)
    })
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
