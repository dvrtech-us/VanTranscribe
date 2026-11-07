# Consented encrypted speaker memory

This feature implements opt-in local enrollment and People management from design §§7.4 and 11. Automatic recognition is unavailable until version-specific, gallery-aware calibration is validated; there are no placeholder matching thresholds.

## User flow

1. Enable speaker memory in Settings after reading its separate biometric explanation. Default is off. Enabling does not enroll a person or create/read a Keychain item.
2. On an eligible saved refined speaker, select Remember voice, choose/name a person and record that person's written-consent attestation or the user's own explicit consent. A method and attesting operator are required; the app dates the attestation.
3. Enrollment verifies source eligibility before extracting a mean embedding. It requires at least twenty seconds of clean, non-overlapped speech. Shared-room speakers cannot create/update voiceprints. Remote channel type is explicitly declared; in-person provenance comes from meeting mode.
4. SQLite receives only encrypted exemplar material and public metadata. Unnamed embeddings never become persistent records. The mean is normalized, finite and bounded; confirmed samples are capped at ten per person/channel/version, oldest first.
5. Settings → People lists names, consent metadata, channel type/version and expiry. Forget removes a person and their biometric records/links; Delete all removes all people, consents, voiceprints and links. Past transcript names are preserved unless the user also requests their removal.

## Technical flow

The `vt-session` boundary verifies meeting/run/speaker/source conditions, obtains a fresh trusted embedding and uses one SQLite transaction for enrollment, consent, the person link and the explicit speaker name. `vt-speakers` accepts a caller `Connection` (including a dereferenced `Transaction`) and also uses nested savepoints to roll back partial mutations on error. It accepts no WebView-provided embedding.

`initialize` idempotently adds the tables without accessing OS secrets. `update_settings` validates one through twenty-four calendar months, requires acknowledgment on each off→on transition, applies retention changes and never accesses keys. `status` reads metadata only: `protected` means an authenticated key verifier exists, not that Keychain is unlocked. `unavailable_platform` is explicit outside macOS. The `matching_available` field is always false. An absent/unverified current model version conservatively marks saved versions stale.

`enroll` validates opt-in and per-person consent, source duration, shared-room exclusion, SHA-256 version identity, channel provenance and embedding geometry. It then obtains the separately scoped key, verifies the store's encrypted key-check sentinel, decrypts only an existing matching person/channel/version exemplar set, appends the confirmed sample and encrypts the bounded result. XChaCha20Poly1305 uses a fresh 24-byte random nonce per write. Associated data binds the format domain, voiceprint row ID, person ID, channel type, channel provenance and embedding version. Blob structure and exemplar count/dimension/norm are revalidated on decrypt. Embedding buffers, serialized plaintext and keys use zeroizing wrappers where held by this crate.

The macOS `KeychainProvider` uses a dedicated service and SHA-256 of the canonical data-root bytes as its account scope. Reads and writes use the SecItem generic-password API family. Creation uses `SecItemAdd` through the pinned safe binding, without an update operation; a racing existing key is read, never overwritten. Missing or unavailable keys and authentication failures reject writes without fallback. An orphan Keychain key may remain if a subsequent SQLite transaction rolls back; it contains no biometric material and can be reused safely. No key is stored in SQLite, configuration, logs or IPC.

`expire` deletes expired voiceprint rows without decrypting. Startup and periodic idle maintenance invoke it even when speaker memory is off. Names/consent history survive expiry until explicitly forgotten. Explicit Delete all clears the authenticated key-check/store ID only after removing all biometric blobs, allowing recovery from a genuinely lost key without overwriting any remaining ciphertext. Forgetting one person does not clear the store-wide verifier.

## Pure matching and research evaluation

`crates/vt-speakers/src/matching.rs` supplies an open-set matcher without enabling app recognition. No release calibration pin is shipped and `MemoryStatus.matching_available` remains false. `Calibration::bind` accepts exact artifact bytes only against a non-deserializable pin constructed from reviewed release-code constants. The SHA-256 pin authenticates those bytes and their exact channel, embedding contract, semantic calibration version and protocol; fields such as `verified: true` cannot authorize a profile. Hash agreement does not establish scientific adequacy: independent held-out evidence must be reviewed before release code acquires a pin.

The raw model/preprocessing/normalization contract digest is separate from its semantic calibration version. `identification_version(raw_contract_sha256, calibration_version)` derives the full comparison identity without hashing the artifact that contains it. Changing either input changes that identity. Research uses `vt_diarize::voice_embedding_contract_sha256()` when available; the legacy/current calibration-unavailable version must not be passed off as the raw contract. Existing saved prints are never silently relabeled as belonging to a new calibrated version.

Only exact version/channel, non-stale, non-shared-room candidates are compared. The caller additionally supplies only consented, unexpired gallery material. At most 1,024 people, ten exemplars per person and 1,024 dimensions are accepted. Finite vectors must already have L2 norm within 0.001 of one; comparison renormalizes in f64. Each person contributes one normalized centroid over their exemplars. Duplicate eligible person IDs, mismatched dimensions, invalid norms and a cancelling zero centroid fail closed. The runner-up is a distinct person, never the second exemplar of the winning person.

Auto-labeling requires the pinned curve's score, positive second-person margin and at least 15,000 ms of clean query speech (a profile may require more). High scores with too little speech or insufficient margin can only suggest. A singleton has no measured runner-up and therefore only suggests; no runner-up score or margin is invented. Scores are cosine similarities, not probabilities. Shared-room or zero-speech queries cannot match.

Curves use conservative gallery-size buckets. Coverage, thresholds and margins cannot decrease, a profile extending above fifty people must include an explicit fifty-person boundary, and every bucket above fifty must raise the automatic threshold above that boundary. Gallery counts outside the profile's tested coverage return unavailable; there is no extrapolation. Missing calibration and uncovered version/channel return no identity or score.

`rank_for_evaluation` exposes the same distinct-person centroid scores without making a recognition/suggestion decision or granting authority to install calibration. It writes no vectors or store data. `evaluate_identification` requires fixed unique gallery IDs and fixed query truth labels; it cannot rename or permute identities after scoring. Recognition is correct automatic labels divided by **all known trials**, including abstentions. Wrong automatic labels on both known and unknown trials count toward overall false identification; unknown false identification is also reported separately. Suggestions, abstentions and unavailable cases have explicit counts. Reports record whether the fixed gallery contains exactly fifty people and issue no acceptance flag or confidence bound: corpus independence and correlated trials require separate evidence.

## Key classes and integration points

| Type/function | File | Responsibility |
|---|---|---|
| `MemorySettings`, `MemoryStatus`, `PersonInfo`, `VoiceprintInfo` | `crates/vt-speakers/src/lib.rs` | Metadata-only contracts; no embedding/key serialization |
| `Enrollment` | `crates/vt-speakers/src/lib.rs` | Trusted session-only input; embedding zeroized on drop |
| `CalibrationPin`, `Calibration`, `identify`, `rank_for_evaluation`, `evaluate_identification` | `crates/vt-speakers/src/matching.rs` | Exact pinned-profile binding, pure open-set policy and fixed-label research metrics; no runtime enablement |
| `KeyProvider`, `KeychainProvider` | `crates/vt-speakers/src/keys.rs` | Injectable secret boundary; add-only macOS implementation |
| `encrypt`, `decrypt` | `crates/vt-speakers/src/crypto.rs` | Versioned authenticated encryption and bounded parsing |
| `initialize`, `settings`, `update_settings`, `status` | `crates/vt-speakers/src/store.rs` | Migration, opt-in, metadata and retention policy |
| `enroll`, `forget_person`, `delete_all`, `expire` | `crates/vt-speakers/src/store.rs` | Atomic consented mutation and destruction |

Integration uses the existing session database and desktop command boundary. Person links and optional transcript-name cleanup belong to the session layer and share the outer transaction. The store has no networking, recording, notification or provider integration. `KeyProvider` injection allows tests to prove missing/locked/wrong-key failures without accessing the real Keychain.

## Routes and access control

This is a local desktop capability; there is no HTTP route or remote authentication surface. The desktop commands require explicit settings acknowledgment or per-person consent before an enrollment. Caller-side source/run checks and storage-side invariants both apply. Metadata reads, expiry and explicit deletion do not require a secret. Unsupported secret backends fail closed for enrollment.

## Database schema

| Table | Columns and purpose |
|---|---|
| `app_voice_memory_settings` | Singleton `enabled` (false), `retention_months` (12, range 1–24), `explanation_acknowledged_at`, random `store_id`, encrypted `key_check` verifier |
| `app_people` | `id`, `display_name`, `is_self`, `created_at`; at most one self entry |
| `app_biometric_consents` | `id`, `person_id`, `attested_at`, `method`, `attested_by`, `is_self`, optional `revoked_at`; active consent history |
| `app_voiceprints` | `id`, `person_id`, `channel_type`, `channel_provenance`, `embedding_version`, encrypted `exemplars`, `n_exemplars` (1–10), `last_matched_at`, `expires_at`, `updated_at`; unique person/channel/version |

SQL artifact: [001-voice-memory.sql](sql/001-voice-memory.sql). Explicit delete operations remove dependent rows even when a caller has not enabled SQLite foreign keys. Key and plaintext embeddings are never database columns. Person names and consent metadata remain unencrypted; this feature is not whole-database or recording encryption.

## Verification and limits

Store regression tests use fake keys and fictional names. They cover opt-in, source/consent validation, restart roundtrip, ciphertext-only persistence, key loss/lock/mismatch, tamper/row-swap rejection, isolated channels/versions, capped learning, calendar expiry, transaction failure rollback and key-loss recovery after explicit Delete all. Session and browser checks additionally verify the actual app flows. An explicit signed native probe verifies the production provider’s default-scope create/read, duplicate refusal, same-binary second-process access, isolation and exact cleanup. Installed-app upgrade ACL prompts, physically locked-Keychain behavior, calibrated recognition, a fifty-person gallery and in-house speaker quality remain separate acceptance evidence. See [PRIVACY.md](../../../../PRIVACY.md) for the published schedule and backup/export limits.

Primary crypto implementation reference: [RustCrypto XChaCha20Poly1305 documentation](https://docs.rs/chacha20poly1305/0.10.1/chacha20poly1305/). The pinned package source and its safe macOS Keychain binding are the implementation references.

During an enrollment the form stays disabled with a saving indication while its own backend processing reservation blocks other edits. A source/context rejection is shown after the server returns; the UI does not mistake that expected busy reservation for a speaker change.

Refined evidence may be retained from an earlier actual refined pass over the same verified source; the current run kind does not prove that diarization reran. Copied Live or absent context remains ineligible. Marking a previously enrolled speaker as a shared room blocks future samples but does not delete an existing saved print; use Forget in People for that deletion. Forget/Delete all are intentionally generation-free protective operations.

Runtime probe evidence and its strict temporary-account scope are recorded in the [Keychain checkpoint](../../../planning/Architecture/2026-11-07-private-keychain-validation.md). The research/calibration work is recorded in the [matching checkpoint](../../../planning/Architecture/2026-11-07-speaker-matching-calibration-validation.md); no research report activates application matching.

The [first native public calibration](../../../planning/Architecture/2026-11-07-speaker-matching-calibration-validation.md#actual-native-result) missed its recognition gate (113/150 known = 75.33%, one wrong-known, 36 abstentions, 0/42 unknown false IDs). Held-out inference was skipped; no calibration pin or automatic-recognition availability was enabled. Enrollment and People controls retain their previous behavior.

A later [English extractor comparison](../../../planning/Architecture/2026-11-07-english-speaker-calibration-comparison.md#corrected-native-result) found missing feature normalization and improved calibration recognition to 130/150 (86.67%) after a separately versioned correction. It still misses 90%; no held-out identity score, release calibration, app-model switch or automatic matching was enabled. Existing enrollment/People behavior and stored-print identities remain unchanged.
