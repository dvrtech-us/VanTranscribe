# VanTranscribe local data and voiceprint retention

VanTranscribe stores recordings, transcripts, notes and settings on this computer. Recording and transcription do not upload audio. Model downloads and explicitly enabled external AI notes have separate controls. Recording consent and consent to retain a person's voiceprint are separate choices.

## Speaker memory

Speaker memory is off by default. Enabling it does not enroll anyone. Enrollment requires a separate explicit confirmation for each person: the operator records the participant's written-consent method and who made the attestation, or confirms their own consent when enrolling their own voice. The app records the attestation date. Recording consent alone does not authorize a voiceprint.

An enrollment uses at least twenty seconds of eligible, clean, non-overlapped speech from a saved refined transcript. Shared-room speakers cannot be enrolled. Voiceprints are separated by channel type and embedding version. Unnamed processing embeddings stay in memory; the app does not save a background gallery of everyone it hears.

Voiceprints are encrypted before they enter SQLite, using a separate random key kept in macOS Keychain. The key is scoped to the local data folder. A missing, locked or incorrect key stops enrollment; it never causes plaintext storage. Person names, consent records, source links, channel/version metadata and expiry dates are ordinary local database metadata. This feature does not encrypt the entire meeting database, recordings, exports or backups. Full database/audio encryption remains a separate roadmap item.

Automatic recognition and suggestions remain unavailable until matching has validated, version-specific calibration. Saving a voiceprint does not itself establish reliable identification.

## Published biometric retention and destruction schedule

A voiceprint expires after **twelve calendar months** without a confirmed enrollment or match by default. Settings permits one through twenty-four months. Confirmed new samples refresh the expiry date; reading People or generating a transcript does not. The exemplar set is capped at ten per person, channel type and embedding version; the oldest sample is evicted first.

Expired voiceprints are deleted when the app starts and during periodic idle maintenance while the app is running. An app that is closed cannot run its deletion schedule; the next startup applies it. Shortening the retention setting applies the shorter schedule to existing voiceprints immediately. Extending a schedule cannot restore a voiceprint that has already expired and been deleted. Named person entries and their consent history remain until explicitly forgotten, even when all their voiceprints have expired.

**Forget this person** deletes that person's voiceprints, consent attestations and person links. **Delete all voiceprints** deletes every stored person, consent attestation, voiceprint and person link. These operations work without unlocking the biometric key. Past meeting views retain their existing speaker-name labels unless **remove linked speaker-name labels from past meetings** is also selected. That option clears linked name overlays and invalidates derived summaries/search; it does not rewrite names inside spoken transcript text, personal notes, stored summaries, immutable source history or exports. Deletion cannot retract copies already exported, sent to an explicitly enabled AI provider or included in an external backup.

Deletion removes active database records. It is not a promise of forensic erasure from filesystem snapshots, SQLite pages or external backups. Such copies remain subject to their own storage and retention controls.

## Consent requests

The interface can prepare an editable consent request for the operator to copy and send. Preparing or copying a request does not send it, establish consent or enroll the recipient. The operator must record the actual consent before saving a voiceprint. The app's reminders and request template describe product behavior; they do not provide legal advice.
