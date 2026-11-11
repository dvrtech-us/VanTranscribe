# Voice-memory baseline

## Invariants

- Speaker memory is off by default; enabling it never enrolls a person.
- Each off→on transition requires explicit acknowledgment of the biometric explanation.
- Enrollment requires a named person, explicit per-person written or self consent, a method, an attesting operator and an app-recorded date.
- Only trusted refined-source extraction supplies embeddings; WebView requests cannot supply them.
- At least 20,000 ms of clean non-overlapped speech is required. Shared-room speakers never create or update a voiceprint.
- Embedding versions are complete lowercase SHA-256 identities. Person/channel/version sets are isolated; channel provenance is retained.
- Embeddings are finite, nonzero, normalized and bounded to 2–1,024 dimensions. A version cannot silently change dimensions.
- Every exemplar blob is authenticated encryption with a fresh random nonce and row/person/channel/provenance/version associated data.
- The separate root-scoped key exists only in the OS secret store. It is never written to SQLite/configuration, returned by IPC or logged.
- A persisted verifier prevents a missing or wrong key from creating a new gallery alongside unreadable old voiceprints.
- Metadata listing, settings, expiry and explicit deletion do not access Keychain.
- Unnamed embeddings stay in processing memory and are never persisted.
- Automatic labels and suggestions remain unavailable without validated version/channel/gallery-specific calibration. A self-declared artifact field cannot authorize a profile; no release pin is currently shipped.
- Raw embedding contract and semantic calibration version derive a separate full comparison identity without circular artifact hashing. Profile changes never silently reuse old prints.
- Matching compares one normalized centroid per distinct person, exact version/channel only; stale/shared-room candidates are excluded and callers remove expired/unconsented data before scoring.
- Automatic policy requires score, positive margin over a distinct runner-up and at least fifteen seconds of clean query speech. Singleton galleries produce no invented margin and can only suggest.
- Curve coverage is bounded and never extrapolated. Above fifty eligible people the automatic threshold exceeds its explicit fifty-person boundary.
- Research ranking never enables matching, labels a stored transcript, learns a sample or refreshes expiry. Fixed-identity evaluation counts wrong known and unknown labels and includes abstentions in recognition denominators.
- A user-confirmed sample refreshes the schedule and appends one exemplar; each person/channel/version set retains at most ten, oldest first.
- The default expiry is twelve calendar months without a confirmed enrollment/match; configuration permits one through twenty-four. Expired blobs cannot be resurrected by extending retention.
- Expiry destroys voiceprint rows and retains people/consent metadata. Explicit Forget/Delete all removes the corresponding people, consents and links.
- Past plain-text transcript names remain unless the user requests name removal. Deletion cannot retract exports, prior provider disclosures or external backups.
- A database mutation either commits enrollment/consent/link/name together or leaves them unchanged. OS Keychain key creation is separate and may survive a rolled-back first enrollment without storing biometric material.

## Flags and failure rules

| Condition | Severity | Blocks enrollment? |
|---|---|---|
| Memory off or explanation not acknowledged | Action required | Yes |
| Missing per-person consent/method/operator | Action required | Yes |
| Insufficient clean refined speech or shared room | Ineligible | Yes |
| Stale/wrong source run, missing audio or invalid channel proof | Ineligible | Yes |
| Missing, locked, unsupported or malformed key | Unavailable | Yes; no plaintext fallback |
| Ciphertext/verifier/metadata authentication failure | Integrity error | Yes; existing records preserved |
| Version lacks verified calibration | Experimental limitation | Recognition/suggestions unavailable |
| Expired voiceprint | Scheduled destruction | Deleted without key access |

## Side effects in order

1. Validate explicit settings/consent and trusted source eligibility.
2. Verify/extract the bounded mean into memory; no audio or unnamed embedding is written.
3. Load or add the separate OS key (`prepare_key`) outside runtime/detection locks and every database `read`/`write` closure; reads and add-only writes both use the SecItem generic-password namespace.
4. Under the runtime/controller guards and one transaction, revalidate current authority/source and authenticate the stored verifier.
5. Decrypt and validate only a matching existing exemplar set, append the confirmed sample, encrypt with a fresh nonce, persist person/consent/voiceprint/link/name atomically.
6. On failure, roll back database changes and zeroize owned sensitive buffers.

## Defaults and access control

Local desktop only; no HTTP or cloud capability. Default `enabled=false`, `retention_months=12`, `matching_available=false`, no people and no key. Reading status does not ask the OS to unlock a key. Voiceprint payloads and keys are never serialized for the interface. Explicit Delete all can reset a lost-key verifier only after all corresponding ciphertext has been removed. Whole-database/audio encryption remains a separate feature.

- An enrollment’s own runtime processing reservation does not present the submitted form as a changed speaker. Its disabled form stays in progress until the server returns; fresh eligibility and generation checks remain authoritative on the server.

- Forget/Delete all intentionally require no page generation: they reduce stored biometric data and cannot enable memory or enroll a voice.
- Flagging an already enrolled speaker as a shared room blocks further enrollment but does not retroactively delete a saved print or name; Forget in People removes those records.
- Retained annotations with actual Refined context remain eligible on the same verified source even when a later run reuses them; Refine run kind alone, copied Live context and absent context are insufficient.
