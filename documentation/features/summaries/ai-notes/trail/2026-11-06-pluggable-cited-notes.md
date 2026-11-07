# Pluggable cited notes with isolated on-device inference

- Date: 2026-11-06
- Feature: ai-notes
- Related code: `crates/vt-summarize/`, `native/macos/SummaryHelper.swift`

## Context

The application needs useful structured AI notes while preserving local-first operation, explicit cloud consent, immutable transcript evidence and the macOS 14.4 deployment baseline. Apple's on-device model has a small context window and cannot safely receive an entire long meeting in one request.

## Decisions

Use a provider trait with explicitly selected none/Apple/local/OpenAI/Anthropic/compatible modes. Keep all credentials in Keychain under exact provider/origin accounts. Treat transcript and generated output as untrusted data; offer no tool execution. Disable redirects/proxy discovery, restrict local classification to numeric loopback, require cloud HTTPS and explicit consent, and bound request sizes/timeouts/cancellation.

Translate immutable IDs to compact strings and validate every item's citations against its individual generation input before remapping. Use fresh bounded map sessions and bounded recursive reduction. Retain partial grounded results on source/reduction failures, with explicit gaps; never publish a successful-looking empty summary if every map call failed.

Keep FoundationModels-specific async sessions, `@Generable` models and token accounting in a small subprocess. Compile against the actual installed SDK, weak-link FoundationModels, and guard all framework APIs so the process still launches on macOS 14.4. Query actual token counts on macOS 26.4+, use conservative guarded fallback otherwise, and split maps adaptively when the helper rejects a context budget. Cancellation terminates and reaps the process.

## Alternatives considered

- `fm-rs` 0.3: evaluated its upstream README/API and build script before choosing the helper. The crate builds its Swift bridge at minimum macOS 26 and emits a strong `framework=FoundationModels` link from `build.rs`. Direct adoption would conflict with the macOS 14.4 launch baseline and makes cancellation/token/schema boundaries less directly controllable. A later adoption would require a separately isolated binary or upstream changes, so this implementation uses the Swift helper instead. Sources: [upstream](https://github.com/blacktop/fm-rs), [build script](https://raw.githubusercontent.com/blacktop/fm-rs/main/crates/fm-rs/build.rs).
- A single full-transcript request: rejected because it cannot respect Apple's 4096-token context.
- Automatic cloud fallback: rejected because unavailable local inference is not consent to send a meeting elsewhere.

## Consequences

- Positive: local operation, transport/credential isolation, source traceability, explicit partial failure and controlled cancellation are independently testable with synthetic data.
- Negative: source citations cannot establish that a model's claim follows from its source. Small on-device models can misclassify actions/decisions and omit stated metadata, so users must review cited output. A small input budget increases calls for long meetings. Uncombined notes can be larger than a fully reduced summary.
- Validation: synthetic Rust provider/loopback tests cover bounded map/reduce, Unicode escaping, citation rejection, refusal retry, cancellation, transport contracts and credential origin scope. The 19-test suite, actual availability query, and short/24-topic on-device fictional-text checks succeeded on the development machine; the long check retained 24 immutable citation IDs without partial errors. On-device inference required execution outside the tool sandbox. No real meeting, recording or existing credential was used.
