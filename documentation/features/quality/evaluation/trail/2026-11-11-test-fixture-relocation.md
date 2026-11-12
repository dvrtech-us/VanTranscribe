# Test fixtures move from /private/tmp to a durable folder

- Date: 2026-11-11
- Feature: evaluation (also used by production-inference and speaker-turns tests)
- Related code: crates/vt-session/src/test_fixtures.rs, crates/vt-session/src/live_speaker_tests.rs, crates/vt-session/src/processing_tests.rs, crates/vt-session/src/speaker_pipeline_tests.rs

## Context

A machine restart on 2026-11-10 wiped `/private/tmp`, which held public fixtures and verified model caches that ignored tests and evaluation notes referenced.

## Decisions

- Restore them to `/Volumes/dev/vantranscribe-eval/test-fixtures` with a hash manifest (source, size, SHA-256, licence), a README and `env.sh`, keeping the old relative layout.
- One test helper resolves `$VT_TEST_FIXTURES`, else an existing `/private/tmp` path, else that folder, so nothing needs setting after a restart and older invocations still work.
- AMI evaluation data lives under `/Volumes/dev/vantranscribe-eval`. Earlier trail and planning notes citing `/private/tmp/vantranscribe-ami-eval` or `ami-room-v3` are historical and are not rewritten.

## Alternatives Considered

- Keep `/private/tmp` and re-create after each restart: fragile; the restart showed it loses data.
- Commit fixtures to the repository: rejected for size and licences.

## Consequences

- Positive: tests survive a restart; provenance and hashes are recorded.
- Negative: not restorable: the four-speaker Chinese fixture (no recorded source), the 507 MB standard zipformer archive (re-downloadable) and the hand-built WeSpeaker research packs; tests that need them stay unrunnable.
