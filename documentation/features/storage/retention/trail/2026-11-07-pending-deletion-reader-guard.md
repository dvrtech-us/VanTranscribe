# Pending Deletion Reader Guard

- Date: 2026-11-07
- Feature: retention
- Related code: `crates/vt-session/src/retention.rs`, `retention_tests.rs`

## Context

A confirmed deletion can survive termination with audio in trash while ready chunk rows remain in SQLite. If cleanup or restoration is blocked, ordinary recorder recovery could mistake those temporarily moved files for lost recording data and convert recoverable ready rows to gaps.

## Decisions

Expose a direct confirmed/cleanup-intent lookup independent of meeting-row existence. Pending intent makes audio unavailable and requires the session layer to reject playback, inference and recorder recovery for that meeting until retention recovery finishes. Unconfirmed previews remain nonblocking. Missing/deleted meetings report audio unavailable; existing legacy meetings keep their prior default availability.

## Alternatives Considered

- Rely only on explicit purge metadata: rejected because database cleanup may not have committed yet.
- Join pending lookup to live meeting rows: rejected because physical cleanup can remain pending after those rows have been deleted.
- Let recorder recovery repair missing files: rejected because the retention intent still owns the recoverable originals.

## Consequences

- Positive: interrupted deletion preserves original ready chunk hashes/states until recovery can finish; processing never fabricates gaps from temporarily moved audio.
- Negative: an affected meeting's audio operations remain blocked while an obstruction requires recovery or user attention.
