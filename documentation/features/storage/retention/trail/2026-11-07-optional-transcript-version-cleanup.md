# Optional Transcript Version Cleanup

- Date: 2026-11-07
- Feature: retention
- Related code: `crates/vt-session/src/retention.rs`, `retention_tests.rs`

## Context

The approved seven-day obsolete-run default must coexist with an explicit user choice to retain every transcript version. Audio-age retention is independent of that choice.

## Decisions

Add `cleanup_obsolete_runs`, default true in both new policies and older serialized policies missing the field. A false flag protects all otherwise-eligible old runs and suppresses run-GC actions, while an enabled audio-age policy continues to purge audio only. Keep `obsolete_run_days` positive instead of overloading zero as an infinite-retention sentinel. Policy previews serialize the flag for the session service's policy-staleness check; durable deletion payload format stays unchanged.

## Alternatives Considered

- Zero-day sentinel: rejected because an explicit flag makes retention intent clear and keeps age validation consistent.
- Disable all maintenance together: rejected because users may retain transcript history while separately limiting audio storage.

## Consequences

- Positive: users can retain all transcript versions without giving up independent audio retention; existing policies preserve the approved default.
- Negative: disabling cleanup intentionally allows transcript-version storage to grow.
