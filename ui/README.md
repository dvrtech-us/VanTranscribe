# Desktop interface

This is the Svelte 5, TypeScript and Bootstrap 5 interface embedded by Tauri. It reads real local state through `@tauri-apps/api`; a regular browser shows a connection message rather than seeded meetings or a simulated recorder.

```sh
npm install
npm run check
npm run build
```

The root application's `npm run dev` starts Tauri with the Vite server at `127.0.0.1:1420`. Run the root command for a connected desktop app.

`src/App.svelte` implements the library, recording consent/setup, live transcript, saved meeting detail, notes/bookmarks, export controls and local model setup. `src/lib/api.ts` defines the serialized Rust command contract. The interface polls snapshots and selected meeting details without overlapping scheduled polls: 500 ms during recording, processing, or either model download; 2 seconds while idle. Library search debounces for 250 ms, sends `search_meetings` to the backend FTS index, and discards outdated responses.

Recording requires a per-session participant-awareness acknowledgment. Microphone selection uses `input_devices`; a Bluetooth default must be selected explicitly before starting. Capture failures remain visible and allow another recording attempt. Model setup is an explicit download action; audio recording remains available before the model is installed. A saved meeting without transcript text can be submitted to `transcribe_meeting` after model installation.

Theme and selected microphone ID are stored locally in the WebView's storage. Meetings, notes, transcript content and model state come from the Rust backend. The interface does not play audio in the WebView.

Keyboard shortcuts: `⌥⌘R` opens recording setup or the current recording, `⌥⌘B` creates a bookmark, and `⌘F` focuses library search. The recording dialog traps keyboard focus; polling transcript updates preserve a user's scroll position until they choose to follow live text again.


Saved meeting detail includes a transcript-version selector. Historical versions are read only and remain selected during polling. Current, stopped transcripts support inline text corrections; the backend keeps original segment rows immutable. Unmatched overlays remain in a review panel until the user chooses a current segment or explicitly dismisses the edit. Partial live text is italic and muted and cannot be edited. Exports use the current transcript, and are disabled while viewing history.

The refinement model card in `src/lib/ProcessingSettings.svelte` shows installation status, download progress, cancellation and license attribution. Post-recording refinement and summary preferences are persisted through `update_preferences`. Ongoing refinement, replay and summary jobs show stage/progress and a cancellation action while the published transcript remains visible.

The Summary tab is always available. `src/lib/SummaryPanel.svelte` renders structured AI notes as escaped text, marks stale and partial results, exposes processing gaps, and links citations to their exact source run and segment. Summary-specific failures preserve transcript health and appear separately. Abandoned unpublished runs are omitted from transcript history choices. Missing or malformed structured envelopes fall back to escaped Markdown text. Generation requires a configured provider and a completed current transcript; otherwise the user can open Settings.

Provider capability discovery reports Apple availability and local model servers without enabling a provider. External providers require an explicit consent checkbox. Provider or endpoint changes revoke the draft consent and discard draft key input. Keys exist only in the password input and the explicit IPC request; they are cleared after save or leaving Settings, never stored in browser storage or read back. The remove-key checkbox explicitly targets the selected provider/origin. Saving with provider Off remains supported. A stored key is managed by the backend's macOS Keychain integration.

Browser checks in `tests/ui/desktop.spec.ts` use isolated synthetic IPC fixtures. They cover the existing recorder, download cancellation, real search-command dispatch, refinement isolation, historical read-only views, overlay review, source-run navigation, escaped summary output, stale/partial labeling, explicit cloud consent and ephemeral keys. No cloud inference or actual recordings are used by the browser suite.
