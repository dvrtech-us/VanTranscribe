# Reversible Explicit Bleed Review

- Date: 2026-11-06
- Feature: bleed-review
- Related code: `crates/vt-session/src/bleed_store.rs`, `bleed_store_tests.rs`, desktop SQL007

## Context

Nearby text repetition can result from microphone bleed or genuine speech. Acoustic non-user evidence is unavailable. Processing can also change transcript boundaries and text, making blind annotation inheritance unsafe.

## Decisions

Text overlap suggests visible candidates; only an explicit user choice hides speech. Ordered token similarity uses a bounded temporal search. Immutable audit events and per-run match snapshots separate reversible choices from saved raw transcripts. Exact anchors must match uniquely in both directions on publication; otherwise speech remains visible. Only unresolved bleed creates a pending review; unmatched genuine choices need no repeated confirmation, and their audit still supports a later exact match. Dismissing an unmatched annotation appends a retired genuine event rather than guessing a target. Historical snapshots remain unchanged when a current choice changes.

## Alternatives Considered

- Automatic text-only hiding: rejected because genuine repetition and short replies would be lost without acoustic evidence.
- Mutable raw transcript deletion: rejected because audio/transcript history and user restoration must remain available.
- Nearest-time fuzzy reapplication: rejected because a changed line may represent different speech.
- Unbounded all-pairs candidate search: rejected because long meetings require bounded per-line work.

## Consequences

- Positive: users can confirm and restore choices with atomic search/revision/summary effects; uncertain rerun matches preserve speech.
- Negative: conservative matching and the 64-candidate/128-token limits can miss plausible duplicates; users review or handle those manually.
- No real-meeting quality, biometric evidence or automatic acceptance threshold is claimed by this workflow implementation.
