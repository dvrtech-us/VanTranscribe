# Consented enrollment before calibrated identification

- Date: 2026-11-07
- Feature: voice-memory
- Related code: `crates/vt-speakers`, `crates/vt-session`, `crates/vt-diarize`, desktop command and People interfaces

## Context

The accepted design requires optional cross-meeting speaker memory with independent participant consent, channel/version isolation, encrypted biometric storage and a published destruction schedule. Existing speaker labels are experimental; uncalibrated cosine similarity cannot establish reliable identity. The prior Anthropic and Codex design reviews remain accepted.

## Decisions

Implement explicit enrollment and People management without inventing automatic-recognition thresholds. A named, consented refined speaker supplies at least twenty seconds of trusted clean speech to an in-memory mean extractor. The session revalidates source authority and commits person, consent, voiceprint, person link and explicit name together. Shared rooms and unknown/ineligible sources fail closed. Remote channel declarations are explicit and recorded; in-person provenance comes from meeting mode.

A separate `vt-speakers` boundary encrypts bounded exemplar sets using XChaCha20Poly1305 with random nonces and associated metadata. A dedicated canonical-root-scoped macOS Keychain key is created only on explicit enrollment using an add-only API. A stored authenticated verifier detects missing/wrong keys before any new biometric write. Plaintext payload and key buffers use zeroizing ownership. The metadata API does not read Keychain, and neither settings nor deletion requires the key.

Retain at most ten confirmed exemplars per person/channel/version, evicting oldest first. Default expiry is twelve calendar months, configurable from one through twenty-four; startup and periodic idle maintenance delete expired blobs. Expiry keeps named people/consent history; explicit Forget/Delete all removes people, consent and links, with past transcript-name removal an additional choice. Delete all clears the verifier only after removing every biometric blob, permitting safe recovery from genuine key loss. The published schedule distinguishes active-record deletion from exported/backed-up copies and forensic storage erasure.

## Alternatives considered

- Persist unnamed embeddings for later enrollment: rejected because the design prohibits creating an implicit biometric gallery.
- Reuse provider credentials or a future database key: rejected because voiceprints need independent key scope and failure behavior.
- Use model placeholder thresholds for recognition: rejected because version- and gallery-specific calibration is absent.
- Treat a locked/missing key as permission to create a new one or store plaintext: rejected because it destroys access to existing prints or violates the storage invariant.
- Let expiry silently remove people and historical identity links: rejected; automatic expiry destroys biometric blobs while explicit person deletion governs identity metadata.

## Consequences

Enrollment and destruction are concrete local capabilities even while automatic recognition remains gated. Biometric blobs are encrypted independently, but person/consent metadata, meeting data, exports and backups are not protected by this column key. A first failed SQLite publication can leave an unused Keychain key; no biometric payload is stored in that item. Physical Keychain behavior, calibrated recognition, gallery-of-fifty acceptance and whole-database/audio encryption remain open requirements.

The submitted enrollment form retains its saving state while the backend reservation temporarily disables other speaker edits. A held-command/polling regression reproduced the earlier false changed-speaker message before the display correction; backend eligibility and generation checks are unchanged.

Final review tightened the platform adapter to one SecItem generic-password API family for both reads and add-only creation. No update/replace operation is used. OS key I/O stays outside the final database/authority locks. Retained actual refined evidence remains eligible on unchanged verified source and has a separate regression; copied Live or absent evidence remains rejected.

The same feature includes pure matching and evaluation contracts for a future reviewed calibration profile. Trusted release code must pin exact artifact bytes plus channel/raw-contract/semantic-version/protocol; the app ships no pin and remains unavailable. Person centroids use at most ten exemplars; runner-up margins compare distinct people. The auto policy requires at least fifteen seconds and a positive covered margin, while singleton galleries only suggest. Conservative monotonic buckets raise auto thresholds above fifty and never extrapolate uncovered gallery sizes. Research ranking reuses this scoring without authorizing identity, and evaluation fixes person/query truth labels rather than optimizing a name permutation. This separates useful corpus measurement from an unsupported runtime-recognition claim.

An explicit isolated signed-native probe now verifies the unchanged production default-scope Keychain adapter, including a same-binary second-process read and exact deletion. It uses only a unique temporary-root account, records no secret bytes, has a hard parent deadline/recovery ledger, and leaves default/search-list metadata unchanged. This is separate from installed-app upgrade ACL and locked-Keychain acceptance.
