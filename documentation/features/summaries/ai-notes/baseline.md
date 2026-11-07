# AI notes baseline

## Flow invariants

- Provider `none` is the default. No provider is automatically enabled or selected as fallback.
- Cloud generation requires explicit cloud consent, HTTPS, a selected model and an origin-scoped Keychain credential.
- Availability/model discovery never starts generation, downloads models, enables Apple Intelligence, or contacts a cloud endpoint.
- Every factual item/action/decision contains nonempty source citations scoped to that individual generation request.
- Immutable real segment IDs are not sent to a provider; compact IDs are remapped before results are returned.
- Unknown or cross-chunk citations reject that generated chunk.
- Model refusal receives exactly one neutral retry. It never causes provider switching.
- A total generation failure is an error. Partial source failures and incomplete bounded synthesis remain explicitly partial.
- Cancellation prevents publication and terminates inflight HTTP/helper work.
- Transcript/model text is untrusted data; no generated actions are executed.
- Credentials never appear in settings, databases, serialized results, provider error bodies, or logs.
- AI output requires review against cited sources; a syntactically valid citation does not prove semantic accuracy.

## Flag rules

| Condition | Severity | Blocks publication? |
| --- | --- | --- |
| Disabled, invalid settings, missing cloud consent/key | Error | Yes |
| All source chunks failed | Error | Yes |
| Cancelled | Error | Yes |
| Some source chunks failed | Partial | No; marked partial |
| Reduction context/iteration limit | Partial | No; prior grounded notes retained |
| Unknown/empty/cross-chunk citations | Error for chunk | That chunk only |
| Apple unavailable | Capability | Apple generation only |

## Side effects in order

1. Explicit save/delete credential actions change only provider/origin Keychain entries.
2. Summary generation reads a supplied immutable transcript snapshot and selected credential.
3. Only the explicitly selected provider receives text.
4. Background progress is reported and a structured result is returned.
5. The caller validates snapshot identity/current revision before database publication.

## Defaults and bounds

| Setting/bound | Value |
| --- | --- |
| provider | `none` |
| cloud_consent | false |
| template | `general` |
| context_tokens | 4096; supported configured range 3072–131072 |
| Apple context | at most 4096 |
| Estimated input map budget | at most 2600 UTF-8 bytes; 2 bytes/token estimate |
| Output token budget | 600 |
| Reduction depth | 6 |
| Adaptive map splitting depth | 8 |
| Transcript bound | 16 MiB |
| Response bound | 1 MiB |
| Connection/request timeout | 3 seconds / 120 seconds |
| Model discovery | 3 seconds, 100 model names, numeric loopback only |
| Redirects / proxy discovery | disabled |

## Access control

Only explicit configured cloud providers receive transcript text. The Keychain service is `com.vantranscribe.summary-provider`; account scope includes exact provider and normalized origin (scheme, host, port). Numeric loopback is the only local HTTP classification. The backend-only secret getter is never a webview API. FoundationModels is weak-linked and availability guarded so macOS 14.4 remains the app deployment baseline.

- Applied user-confirmed bleed is excluded from desktop provider input; visible suggestions are not excluded. Hide/restore advances the summary revision, while raw segments and historical citations remain intact. All-hidden input fails before provider access.
