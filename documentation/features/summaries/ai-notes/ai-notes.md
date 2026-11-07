# AI notes

AI notes are an explicitly selected inference capability. `vt-summarize` accepts final transcript segments, generates structured notes with source citations, and renders Markdown. Provider `none` is the default. The desktop session layer owns meeting selection, settings persistence, summary run identity, stale-result checks, and export.

## User flow

1. Select Apple on-device, a numeric-loopback OpenAI-compatible server, or an explicitly consented cloud provider. Enter a model ID for HTTP providers; none is guessed.
2. Save a credential through the desktop's explicit credential action. It is stored only in macOS Keychain. Local servers may be unauthenticated.
3. Request notes for a completed transcript. A background worker calls `summarize` with immutable segment IDs and a cancellation flag.
4. Review TL;DR, key points, decisions, action items (owner/task/due), open questions, and their source citations. Model claims require human review against the cited transcript.
5. Cancel an active operation or repeat it with another configured provider. A failed request does not silently select another provider.

## Technical flow

`SummaryConfig` contains provider, endpoint, model, context token count, template and cloud-consent state. It never contains a credential. The public `validate_config` checks enabled providers; storing a disabled configuration is handled by the caller. Templates are `general`, `one_on_one`, `standup`, `client_call`, and `interview`; `meeting` is a compatibility alias.

`summarize` validates the configuration and constructs a `Summarizer`. `summarize_with` supports deterministic test providers. It bounds source input at 16 MiB, translates real immutable IDs to compact numeric strings, splits UTF-8 transcript text while preserving IDs, and packs map calls. The estimate allows at most 2 UTF-8 bytes per token, with a 2,200-token instructions/schema/output reserve and an input cap of 2,600 bytes (about 1,300 conservatively estimated tokens). JSON escaping is measured after encoding. Each call requests at most 600 output tokens.

Apple's helper checks actual prompt, instructions and generated-schema token counts on macOS 26.4+, reserves 256 framing tokens and the requested output, and refuses input above the 4,096-token limit before inference. Older runtimes use a conservative byte count of the prompt, actual instructions, and serialized generated schema. A rejected map input is split and retried, to depth eight. Every map/reduce invocation uses a fresh Apple session. Structured note fragments are packed for at most six reduction levels; uncombined grounded notes remain available with an explicit partial flag. A reduction failure retains the prior grounded notes. No source chunks summarized successfully means an error, so previous notes can remain intact.

All factual items must have nonempty citation arrays containing IDs present in that individual map/reduce call. Unknown or cross-chunk IDs reject the generated chunk. Compact IDs are mapped back before results leave the crate. This is citation validation, not an entailment or semantic-accuracy guarantee. Model refusals receive exactly one neutral retry. Failed source chunks and incomplete synthesis set `partial` and add sanitized errors.

HTTP transport uses a 3-second connection timeout, 120-second overall timeout, 1 MiB response bound, no proxy discovery, and no redirects. Inflight cancellation is polled every 40 ms and drops the request. Apple process cancellation/timeout kills and reaps the helper; process polling is every 25 ms. Local model listing uses only GET `/models`, a 3-second timeout, at most 100 names, and the same loopback/response constraints. Apple availability is read-only and bounded at eight seconds. Neither discovery route enables a provider or downloads a model.

## Key classes and entry points

| Entry | File | Responsibility |
| --- | --- | --- |
| `SummaryConfig`, `Provider` | `crates/vt-summarize/src/lib.rs` | Serializable settings with safe disabled default |
| `Summarizer`, `GenerationRequest` | `crates/vt-summarize/src/lib.rs` | Provider-independent generation contract |
| `summarize`, `summarize_with` | `crates/vt-summarize/src/lib.rs` | Bounded map/reduce, refusal handling, citation validation |
| `SummaryContent`, `SummaryResult` | `crates/vt-summarize/src/lib.rs` | Structured cited notes, Markdown, metadata and partial errors |
| `HttpProvider`, `AppleProvider` | `crates/vt-summarize/src/providers.rs` | Explicit HTTP adapters and isolated Apple process |
| `secret_set`, `secret_get`, `secret_present`, `secret_delete` | `crates/vt-summarize/src/credentials.rs` | Exact-provider/origin Keychain credential boundary |
| `apple_availability`, `discover_local_models` | `crates/vt-summarize/src/providers.rs` | Read-only bounded capability probes |
| `SummaryHelper`, `Notes` | `native/macos/SummaryHelper.swift` | Swift async/session, `@Generable` schema and SDK token accounting |

## Integration and access control

The session service supplies a final immutable transcript snapshot, controls the cancel flag, and persists a result only if its transcript run and correction revision remain current. No recording/capture permission is requested by this crate. Transcript text is quoted data; providers receive no tools and cannot execute actions. HTTP cloud generation requires explicit consent, HTTPS, a chosen model and a Keychain credential. No cloud request is made by discovery. Cloud processing uses the selected provider's service and retention terms; the app does not promise that those services are local.

A local endpoint must have a numeric loopback IPv4/IPv6 address. `localhost`, LAN/private addresses and DNS hostnames are rejected rather than trusted as local. Endpoint userinfo, query parameters and fragments are rejected. Keychain accounts are scoped to provider plus normalized scheme/host/port. Changing an origin never reuses another origin's credential. The getter is a backend-only integration primitive and must not be returned to the webview. Provider error bodies, prompt fragments and credentials are never included in errors or logs.

## Database and SQL artifacts

This crate has no database schema and no SQL artifacts. Session-layer summary runs/results and configuration persistence are documented by the desktop-session feature. Only non-secret `SummaryConfig` may be persisted there. Results carry structured content, Markdown, provider/model/template, cited immutable segment IDs, partial state and sanitized errors.

## Build and validation

Build the helper with a current SDK while keeping the application deployment baseline:

```sh
xcrun swiftc -disable-sandbox -module-cache-path /tmp/vt-summary-module-cache \
  -parse-as-library -O -target arm64-apple-macosx14.4 \
  -Xlinker -weak_framework -Xlinker FoundationModels \
  native/macos/SummaryHelper.swift -o /tmp/vt-summary-helper
```

The Swift compiler flag avoids a nested macro-compiler sandbox on restricted build hosts; it does not change app runtime permissions. `otool -l` verifies `LC_LOAD_WEAK_DYLIB` for FoundationModels and deployment target 14.4. All FoundationModels calls and types have macOS 26 availability guards. Availability reports a reason on older or unsupported devices.

`cargo test -p vt-summarize` exercises synthetic providers and loopback fake HTTP servers; the execution environment must permit local socket binding. `cargo clippy -p vt-summarize --all-targets -- -D warnings` checks the crate. `cargo run -p vt-summarize --example synthetic -- /tmp/vt-summary-helper` is an explicit fictional-text on-device smoke check and does not read meeting storage or Keychain. A restricted execution sandbox may permit availability queries while denying the model service itself. The development host passed the 19-test synthetic/loopback suite and both short and 24-topic on-device synthetic smoke checks; the long check returned all 24 immutable citation IDs with no partial flag or errors. Add `--long` to the example to repeat that check.

Protocol sources: [OpenAI Chat Completions](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create), [Anthropic Messages](https://platform.claude.com/docs/en/api/messages/create), and the installed macOS 27.2 SDK Swift interface for FoundationModels.

## Confirmed bleed inputs

The desktop excludes only applied user-confirmed bleed from summary input, while retaining the original segment rows and historical source citations. Restore or a changed bleed decision advances the input revision and makes prior notes stale. With all lines hidden, summary reservation rejects before provider access. A loopback-fixture service regression inspects actual request text and current source IDs, verifies restoration, and preserves the full detail/JSON transcript. Similar-text candidates alone never change provider input.
