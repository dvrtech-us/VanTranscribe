import { test, expect } from '@playwright/test';

// Only this test harness substitutes IPC. Production always invokes Tauri;
// native capture and storage have separate real-backend integration checks.
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const w = window as any;
    w.isTauri = true;
    const snapshot: any = {
      recording: null, meetings: [],
      models: { installed: false, busy: false, progress: 0, error: null },
      refinement_models: { installed: false, busy: false, progress: 0, error: null },
      vad_models: { installed: false, busy: false, progress: 0, error: null },
      diarization_models: { installed: false, busy: false, progress: 0, error: null },
      playback: null,
      processing: null,
      meeting_detection: { context_generation: Number(sessionStorage.getItem('fixture-detection-generation') ?? 0), enabled: false, status: 'disabled', error: null, catalog: [{ id: 'us.zoom.xos', name: 'Zoom', kind: 'meeting' }, { id: 'com.google.Chrome', name: 'Chrome', kind: 'browser' }], prompts: [], ended: null },
      settings: { capture_mode: 'remote', data_dir: '/tmp/vantranscribe-ui-fixture', auto_refine: true, auto_summarize: true, live_vad_enabled: true, diarization_enabled: false, echo_cancellation_enabled: true, summary_config: { provider: 'none', endpoint: null, model: null, context_tokens: 4096, template: 'general', cloud_consent: false } },
    };
    snapshot.settings.meeting_detection = { enabled: false, rules: [] };
    let detail: any = null;
    const calls: any[] = [];
    w.__testCalls = calls;
    const emptyReport = () => ({ completed: 0, meetings_deleted: 0, audio_purged: 0, runs_deleted: 0, bytes_reclaimed: 0, pending_cleanup: [], errors: [] });
    w.__testFixture = { liveModel: 'english_small', installedLiveModels: { english_small: false, english_standard: false }, memory: { enabled: false, retention_months: 12, matching_available: false, key_status: 'not_created', people: [] }, policy: { audio_keep_days: null, obsolete_run_days: 7, cleanup_obsolete_runs: true }, lastReport: emptyReport(), deletionPreviews: {}, previewSequence: 0, snapshot, get detail() { return detail; }, set detail(next: any) { detail = next; }, history: {} };
    w.__TAURI_INTERNALS__ = { invoke: async (command: string, args: any = {}) => {
      calls.push({ command, args });
      switch (command) {
        case 'frontend_ready': return true;
        case 'voice_memory_status': {
          if (w.__testFixture.memoryStatusError) throw new Error(w.__testFixture.memoryStatusError);
          const value = structuredClone(w.__testFixture.memory);
          if (w.__testFixture.holdNextMemoryStatus) {
            w.__testFixture.holdNextMemoryStatus = false;
            return new Promise(resolve => { w.__testFixture.releaseHeldMemoryStatus = () => resolve(value); });
          }
          return value;
        }
        case 'update_voice_memory': {
          if (w.__testFixture.holdNextMemorySave) { w.__testFixture.holdNextMemorySave = false; await new Promise<void>(resolve => { w.__testFixture.releaseHeldMemorySave = resolve; }); }
          if (args.context_generation !== snapshot.meeting_detection.context_generation) throw new Error('This privacy window was reloaded. Its previous settings are no longer authorized.');
          if (w.__testFixture.memorySaveError) throw new Error(w.__testFixture.memorySaveError);
          if (args.enabled && !w.__testFixture.memory.enabled && !args.explanation_acknowledged) throw new Error('Read and acknowledge the biometric explanation first.');
          if (!Number.isInteger(args.retention_months) || args.retention_months < 1 || args.retention_months > 24) throw new Error('Choose one to twenty-four months.');
          w.__testFixture.memory.enabled = args.enabled; w.__testFixture.memory.retention_months = args.retention_months;
          return structuredClone(w.__testFixture.memory);
        }
        case 'enroll_voice': {
          if (w.__testFixture.holdNextEnrollment) {
            w.__testFixture.holdNextEnrollment = false;
            snapshot.processing = { meeting_id: args.request.meeting_id, kind: 'voice_memory', stage: 'Preparing consented voice enrollment', progress: 0, error: null };
            await new Promise<void>(resolve => { w.__testFixture.releaseHeldEnrollment = resolve; });
            snapshot.processing = null;
          }
          if (args.context_generation !== snapshot.meeting_detection.context_generation) throw new Error('This privacy window was reloaded. Its previous enrollment is no longer authorized.');
          if (w.__testFixture.enrollError) throw new Error(w.__testFixture.enrollError);
          const r = args.request; const speaker = detail?.speakers.find((entry: any) => entry.id === r.speaker_id);
          const active = detail?.runs.find((run: any) => run.id === r.run_id);
          const context = detail?.diarization?.segmentation_context;
          if (!w.__testFixture.memory.enabled || !speaker || speaker.shared_room || detail.audio_available === false || detail.active_run_id !== r.run_id || active?.kind !== 'refine' || active.status !== 'complete' || context?.source_window_ms !== 10000 || context.ownership_stride_ms !== 2500 || context.native_window_ms !== 10000) throw new Error('This speaker cannot be enrolled.');
          if (!r.written_consent || !r.attested_by.trim() || !r.is_self && !r.consent_method.trim()) throw new Error('Separate written consent and attestation are required.');
          if (speaker.track === 'system' && !['remote_wideband','remote_narrowband'].includes(r.channel_type)) throw new Error('Choose the source audio type.');
          const person = { id: r.person_id || 'person-fixture', display_name: r.display_name, is_self: r.is_self, consent: { attested_at: '2026-11-07T12:00:00Z', method: r.is_self ? 'Own consent' : r.consent_method, attested_by: r.attested_by }, voiceprints: [{ id: 'voice-fixture', channel_type: r.channel_type ?? 'in_person', embedding_version: 'synthetic-version', n_exemplars: 1, last_matched_at: '2026-11-07T12:00:00Z', expires_at: '2027-11-07T12:00:00Z', stale: false }] };
          w.__testFixture.memory.people = [...w.__testFixture.memory.people.filter((p: any) => p.id !== person.id), person]; speaker.person_id = person.id;
          return structuredClone(w.__testFixture.memory);
        }
        case 'forget_person':
        case 'delete_all_voiceprints': {
          if (w.__testFixture.forgetError) throw new Error(w.__testFixture.forgetError);
          const removed = w.__testFixture.memory.people.filter((p: any) => command === 'delete_all_voiceprints' || p.id === args.person_id).map((p: any) => p.id);
          w.__testFixture.memory.people = w.__testFixture.memory.people.filter((p: any) => !removed.includes(p.id));
          for (const speaker of detail?.speakers ?? []) if (removed.includes(speaker.person_id)) { speaker.person_id = null; if (args.remove_names) speaker.user_name = null; }
          return structuredClone(w.__testFixture.memory);
        }
        case 'set_speaker_shared_room': {
          const speaker = detail.speakers.find((entry: any) => entry.id === args.speaker_id); if (!speaker) throw new Error('Speaker unavailable.'); speaker.shared_room = args.shared_room;
          return structuredClone(detail);
        }

        case 'set_detection_context': {
          if (w.__testFixture.holdNextContext) {
            w.__testFixture.holdNextContext = false;
            await new Promise<void>(resolve => { w.__testFixture.releaseHeldContext = resolve; });
          }
          if (args.context_generation !== snapshot.meeting_detection.context_generation) throw new Error('This recording window was reloaded. Its previous detection setup is no longer authorized.');
          if (w.__testFixture.contextError) throw new Error(w.__testFixture.contextError);
          snapshot.settings.capture_mode = args.mode;
          w.__testFixture.setupOpen = args.setup_open;
          snapshot.meeting_detection.status = !snapshot.meeting_detection.enabled ? 'disabled' : args.setup_open || snapshot.recording || snapshot.processing ? 'blocked' : 'watching';
          return structuredClone(snapshot);
        }
        case 'update_meeting_detection': {
          if (w.__testFixture.holdNextDetectionSave) {
            w.__testFixture.holdNextDetectionSave = false;
            await new Promise<void>(resolve => { w.__testFixture.releaseHeldDetectionSave = resolve; });
          }
          if (args.context_generation !== snapshot.meeting_detection.context_generation) throw new Error('This recording window was reloaded. Its previous detection settings are no longer authorized.');
          if (w.__testFixture.detectionSaveError) throw new Error(w.__testFixture.detectionSaveError);
          const settings = structuredClone(args.settings);
          for (const rule of settings.rules) if (rule.policy === 'always_record' && (!rule.participant_notice_attested || !rule.input)) {
            if (settings.enabled) throw new Error('Automatic recording requires participant notice and an explicit input choice.');
            rule.policy = 'prompt'; rule.participant_notice_attested = false; rule.input = null;
          }
          snapshot.settings.meeting_detection = settings;
          snapshot.meeting_detection.enabled = args.settings.enabled;
          snapshot.meeting_detection.status = args.settings.enabled ? (w.__testFixture.setupOpen ? 'blocked' : 'watching') : 'disabled';
          return structuredClone(snapshot);
        }
        case 'dismiss_detection': {
          const prompt = snapshot.meeting_detection.prompts.find((p: any) => p.episode_id === args.episode_id);
          if (!prompt) throw new Error('This microphone-use reminder is no longer current.');
          if (args.suppress) snapshot.settings.meeting_detection.rules = [...snapshot.settings.meeting_detection.rules.filter((r: any) => r.app_id !== prompt.app.id), { app_id: prompt.app.id, policy: 'suppress', participant_notice_attested: false, input: null }];
          snapshot.meeting_detection.prompts = snapshot.meeting_detection.prompts.filter((p: any) => p.episode_id !== args.episode_id);
          return structuredClone(snapshot);
        }
        case 'dismiss_detection_ended': {
          if (snapshot.meeting_detection.ended?.episode_id !== args.episode_id) throw new Error('Reminder expired.');
          snapshot.meeting_detection.ended = null; return structuredClone(snapshot);
        }
        case 'live_model_catalog': {
          if (w.__testFixture.catalogError) throw new Error(w.__testFixture.catalogError);
          if (!w.__testFixture.modelSelectionWarning) w.__testFixture.installedLiveModels[w.__testFixture.liveModel] = snapshot.models.installed;
          return { selected: w.__testFixture.liveModel, selection_warning: w.__testFixture.modelSelectionWarning ?? null, choices: [
            { model: 'english_small', title: 'English · compact', model_id: 'sherpa-onnx-streaming-zipformer-en-2023-06-26', installed: w.__testFixture.installedLiveModels.english_small, download_bytes: 310414022, installed_bytes: 72466360, source: 'https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-en-2023-06-26.tar.bz2', license: 'Apache-2.0' },
            { model: 'english_standard', title: 'English · standard', model_id: 'sherpa-onnx-streaming-zipformer-en-2023-06-21', installed: w.__testFixture.installedLiveModels.english_standard, download_bytes: 506956414, installed_bytes: 190180941, source: 'https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-en-2023-06-21.tar.bz2', license: 'Apache-2.0' },
          ] };
        }
        case 'select_live_model': {
          if (w.__testFixture.selectionError) throw new Error(w.__testFixture.selectionError);
          if (snapshot.models.busy || snapshot.processing || snapshot.recording && ['recording','paused','processing'].includes(snapshot.recording.status)) throw new Error('Finish recording, processing and speech-model setup first.');
          if (!w.__testFixture.modelSelectionWarning) w.__testFixture.installedLiveModels[w.__testFixture.liveModel] = snapshot.models.installed;
          w.__testFixture.modelSelectionWarning = null;
          w.__testFixture.liveModel = args.model;
          snapshot.models = { installed: w.__testFixture.installedLiveModels[args.model], busy: false, progress: 0, error: null };
          return structuredClone(snapshot);
        }
        case 'app_snapshot': {
          if (w.__testFixture.snapshotError) throw new Error(w.__testFixture.snapshotError);
          const value = structuredClone(snapshot);
          if (w.__testFixture.holdNextSnapshot) {
            w.__testFixture.holdNextSnapshot = false;
            return new Promise(resolve => { w.__testFixture.releaseHeldSnapshot = () => { resolve(value); w.__testFixture.releaseHeldSnapshot = null; }; });
          }
          return value;
        }
        case 'input_devices': if (new URL(window.location.href).searchParams.has('hangDevices')) return new Promise(() => {}); return { devices: [
          { id: 'bluetooth', name: 'Bluetooth input', is_default: true, bluetooth: true },
          { id: 'built-in', name: 'Built-in microphone', is_default: false, bluetooth: false },
        ], system_supported: true };
        case 'start_detected_recording':
        case 'start_recording': {
          if (command === 'start_detected_recording') {
            if (w.__testFixture.rejectDetectionStart || !snapshot.meeting_detection.prompts.some((p: any) => p.episode_id === args.episode_id)) throw new Error('This microphone-use reminder is no longer current.');
            if (!args.consent || snapshot.settings.capture_mode !== 'remote' || snapshot.recording || snapshot.processing) throw new Error('Recording is not available for this reminder.');
            snapshot.meeting_detection.prompts = [];
          }
          snapshot.playback = null;
          const meeting = { id: 'fixture-meeting', title: args.title, status: 'recording', started_at: '2026-11-06T12:00:00Z', duration_ms: 0, segment_count: 0 };
          snapshot.meetings = [meeting];
          snapshot.recording = { ...meeting, elapsed_ms: 1500, mic_level: 0.6, system_level: 0.3, gaps: 0, transcription_configured: false, transcription_active: false, error: null };
          if (snapshot.meeting_detection.enabled) snapshot.meeting_detection.status = 'blocked';
          detail = { meeting, audio_available: true, segments: [], notes: [], summary: null, active_run_id: 'live-run', run_id: 'live-run', runs: [{ id: 'live-run', meeting_id: meeting.id, kind: 'live', status: 'running', engines_json: '{}', constraints_version: 0, created_at: meeting.started_at }], corrections: [], summaries: [], correction_version: 0, speakers: [], speaker_turns: [], speaker_reviews: [], speaker_assignments: [], speaker_constraints: [], diarization: null };
          return structuredClone(snapshot);
        }
        case 'maintenance_policy': return structuredClone(w.__testFixture.policy);
        case 'last_maintenance_report': return structuredClone(w.__testFixture.lastReport);
        case 'save_maintenance_policy': {
          if (snapshot.recording && ['recording', 'paused', 'processing', 'stopping'].includes(snapshot.recording.status) || snapshot.processing) throw new Error('Finish recording and processing first.');
          w.__testFixture.policy = structuredClone(args.policy); return structuredClone(args.policy);
        }
        case 'preview_deletion': {
          if (w.__testFixture.previewError) throw new Error(w.__testFixture.previewError);
          const token = `fixture-deletion-${++w.__testFixture.previewSequence}`;
          const preview = { token, meeting_id: args.id, kind: args.kind, files: 2, bytes: 1048576, runs: args.kind === 'meeting' ? 2 : 0, fingerprint: `fixture-version-${detail.correction_version}`, protected_runs: 0, protected_active: 0, protected_references: 0, foreign_key_tables: ['audio_chunks', 'app_run_segments'] };
          w.__testFixture.deletionPreviews[token] = preview; return structuredClone(preview);
        }
        case 'commit_deletion': {
          if (w.__testFixture.commitError) throw new Error(w.__testFixture.commitError);
          if (w.__testFixture.restoredFailure) return { ...emptyReport(), errors: ['Deletion failed; original files restored.'] };
          const preview = w.__testFixture.deletionPreviews[args.token]; if (!preview) throw new Error('Invalid preview.');
          snapshot.playback = null;
          const report: any = { ...emptyReport(), completed: 1, bytes_reclaimed: preview.bytes };
          if (preview.kind === 'audio') { detail.audio_available = false; Object.values(w.__testFixture.history).forEach((entry: any) => entry.audio_available = false); report.audio_purged = 1; }
          else { snapshot.meetings = snapshot.meetings.filter((m: any) => m.id !== preview.meeting_id); detail = null; w.__testFixture.history = {}; report.meetings_deleted = 1; }
          if (w.__testFixture.pendingCleanup) { report.pending_cleanup = ['confirmed-fixture']; report.errors = ['Data deletion committed; trash cleanup pending.']; report.completed = 0; }
          w.__testFixture.lastReport = report; if (w.__testFixture.failRefreshAfterCommit) w.__testFixture.snapshotError = 'Fixture refresh unavailable.'; return structuredClone(report);
        }
        case 'preview_cleanup': return structuredClone(w.__testFixture.cleanupPreview ?? { policy: w.__testFixture.policy, actions: [], protected_runs: 2, skipped_busy: [] });
        case 'apply_cleanup': {
          if (JSON.stringify(args.preview.policy) !== JSON.stringify(w.__testFixture.policy)) throw new Error('Retention preferences changed. Preview cleanup again.');
          const report = { ...emptyReport(), completed: args.preview.actions.length, runs_deleted: args.preview.actions.reduce((sum: number, a: any) => sum + a.runs, 0) };
          w.__testFixture.lastReport = report; return structuredClone(report);
        }
        case 'meeting_detail': return structuredClone(args.run_id ? w.__testFixture.history[args.run_id] ?? detail : detail);
        case 'pause_recording': snapshot.recording.status = 'paused'; return structuredClone(snapshot);
        case 'resume_recording': snapshot.recording.status = 'recording'; return structuredClone(snapshot);
        case 'stop_recording': if (w.__testFixture.savingOnStop) { snapshot.recording = { ...snapshot.recording, status: 'stopping' }; snapshot.recorder = { queued_ms: 12000, max_queued_ms: 20000, dropped_ms: 0, saving_audio: true }; return structuredClone(snapshot); } snapshot.recording = null; detail.meeting.status = 'ready'; detail.runs[0].status = 'complete'; return structuredClone(snapshot);
        case 'save_note': detail.notes.push({ id: `note-${detail.notes.length}`, t_ms: 1500, text: args.text }); return;
        case 'rename_meeting': detail.meeting.title = args.title; if (snapshot.recording) snapshot.recording.title = args.title; return;
        case 'export_meeting': return '/tmp/vantranscribe-ui-fixture/Exports/fixture.md';
        case 'install_model': snapshot.models.busy = true; snapshot.models.progress = 0.5; return;
        case 'cancel_model_download': snapshot.models.busy = false; snapshot.models.error = 'Model setup was cancelled.'; return;
        case 'install_refinement_model': snapshot.refinement_models.busy = true; snapshot.refinement_models.progress = 0.4; return;
        case 'cancel_refinement_model_download': snapshot.refinement_models.busy = false; snapshot.refinement_models.error = 'Refinement setup was cancelled.'; return;
        case 'install_vad_model': snapshot.vad_models.busy = true; snapshot.vad_models.progress = 0.6; return;
        case 'cancel_vad_model_download': snapshot.vad_models.busy = false; snapshot.vad_models.error = 'Voice activity setup was cancelled.'; return;
        case 'install_diarization_model': {
          if (!args.acknowledge_terms) throw new Error('Read and acknowledge the speaker model notice first.');
          snapshot.diarization_models.busy = true; snapshot.diarization_models.progress = 0.3; return;
        }
        case 'cancel_diarization_model_download': snapshot.diarization_models.busy = false; snapshot.diarization_models.error = 'Speaker setup was cancelled.'; return;
        case 'update_audio_preferences': snapshot.settings.live_vad_enabled = args.live_vad_enabled; snapshot.settings.diarization_enabled = args.diarization_enabled; snapshot.settings.echo_cancellation_enabled = args.echo_cancellation_enabled; return structuredClone(snapshot);
        case 'set_bleed_decision': {
          detail.bleed_decisions = (detail.bleed_decisions ?? []).filter((d: any) => d.segment_id !== args.segment_id);
          detail.bleed_decisions.push({ id: 'decision-' + args.segment_id, segment_id: args.segment_id, decision: args.decision, status: 'applied', reason: 'Synthetic explicit user choice' });
          detail.bleed_candidates = (detail.bleed_candidates ?? []).filter((c: any) => c.mic_segment_id !== args.segment_id);
          return;
        }
        case 'resolve_bleed_review': detail.bleed_decisions.find((d: any) => d.id === args.decision_id).status = 'dismissed'; return;
        case 'meeting_waveform': {
          const bins = Math.max(1, Math.min(2048, args.bins ?? 512));
          return { meeting_id: args.id, duration_ms: 20000, mic: Array.from({ length: bins }, (_, i) => i % 8 ? 0.45 : 0), system: Array.from({ length: bins }, (_, i) => i % 5 ? 0.25 : 0) };
        }
        case 'play_meeting_audio':
        case 'pause_meeting_audio':
        case 'seek_meeting_audio':
        case 'set_playback_volumes': {
          if (snapshot.recording && ['recording', 'paused', 'processing'].includes(snapshot.recording.status)) throw new Error('Playback is disabled while recording.');
          if (!snapshot.playback || (args.id && snapshot.playback.meeting_id !== args.id)) snapshot.playback = { meeting_id: args.id, status: 'paused', position_ms: 0, duration_ms: 20000, mic_volume: 1, system_volume: 1, error: null };
          if (args.position_ms !== undefined) snapshot.playback.position_ms = args.position_ms;
          if (command === 'play_meeting_audio') snapshot.playback.status = 'playing';
          if (command === 'pause_meeting_audio') snapshot.playback.status = 'paused';
          if (command === 'set_playback_volumes') { snapshot.playback.mic_volume = args.mic_volume; snapshot.playback.system_volume = args.system_volume; }
          return structuredClone(snapshot);
        }
        case 'stop_meeting_audio': snapshot.playback = null; return structuredClone(snapshot);
        case 'summary_capabilities': return { apple: { available: true, context_tokens: 4096 }, local_servers: [{ endpoint: 'http://127.0.0.1:11434/v1', available: true, models: ['fixture-local-model'] }, { endpoint: 'http://127.0.0.1:1234/v1', available: false, models: [], reason: 'No server detected' }] };
        case 'update_preferences': snapshot.settings = { ...snapshot.settings, auto_refine: args.auto_refine, auto_summarize: args.auto_summarize, summary_config: args.summary_config }; return structuredClone(snapshot);
        case 'refine_meeting': snapshot.processing = { meeting_id: args.id, kind: 'refine', stage: 'Recognizing speech', progress: 0.35 }; return;
        case 'generate_summary': snapshot.processing = { meeting_id: args.id, kind: 'summary', stage: 'Summarizing', progress: 0.2 }; return;
        case 'cancel_processing': snapshot.processing = null; return;
        case 'correct_segment': detail.segments.find((segment: any) => segment.id === args.segment_id).text = args.replacement; detail.correction_version++; detail.summaries.forEach((s: any) => s.is_stale = true); return;
        case 'resolve_correction': {
          const correction = detail.corrections.find((c: any) => c.id === args.correction_id);
          correction.status = args.segment_id ? 'applied' : 'dismissed';
          if (args.segment_id) detail.segments.find((s: any) => s.id === args.segment_id).text = correction.replacement_text;
          return;
        }
        case 'rename_speaker': {
          if (w.__testFixture.renameError) throw new Error(w.__testFixture.renameError);
          const speaker = detail.speakers.find((entry: any) => entry.id === args.speaker_id);
          if (!speaker) throw new Error('Speaker unavailable.');
          speaker.user_name = args.name.trim() || null; speaker.label = speaker.user_name ?? 'Speaker 1';
          detail.speaker_turns.filter((turn: any) => turn.speaker_ids.includes(speaker.id)).forEach((turn: any) => { turn.label = speaker.label; turn.provenance = 'user'; turn.provisional = false; });
          detail.speaker_assignments.filter((assignment: any) => assignment.speaker_ids.includes(speaker.id)).forEach((assignment: any) => {
            detail.segments.find((segment: any) => segment.id === assignment.segment_id).speaker = speaker.label;
            assignment.provenance = 'user'; assignment.words.forEach((word: any) => { if (word.speaker_ids.includes(speaker.id)) word.label = speaker.label; });
          });
          detail.correction_version++; detail.summaries.forEach((summary: any) => summary.is_stale = true); return;
        }
        case 'add_speaker_constraint': {
          if (!['must_link', 'cannot_link'].includes(args.kind) || args.left_turn_id === args.right_turn_id) throw new Error('Choose two excerpts.');
          const left = detail.speaker_turns.find((turn: any) => turn.id === args.left_turn_id);
          const right = detail.speaker_turns.find((turn: any) => turn.id === args.right_turn_id);
          if (!left || !right || left.track !== right.track || left.overlap || right.overlap || left.end_ms - left.start_ms < 1000 || right.end_ms - right.start_ms < 1000) throw new Error('Choose clean same-track excerpts.');
          detail.speaker_constraints ??= [];
          detail.speaker_constraints.push({ id: `constraint-${detail.speaker_constraints.length + 1}`, kind: args.kind, left: { track: left.track, start_ms: left.start_ms, end_ms: left.end_ms }, right: { track: right.track, start_ms: right.start_ms, end_ms: right.end_ms }, active: true });
          detail.correction_version++; detail.summaries.forEach((summary: any) => summary.is_stale = true); return;
        }
        case 'revoke_speaker_constraint': {
          const constraint = detail.speaker_constraints.find((entry: any) => entry.id === args.constraint_id);
          if (!constraint) throw new Error('Speaker correction unavailable.');
          constraint.active = false;
          detail.speaker_reviews.filter((review: any) => review.constraint_id === constraint.id).forEach((review: any) => review.status = 'dismissed');
          detail.correction_version++; detail.summaries.forEach((summary: any) => summary.is_stale = true); return;
        }
        case 'resolve_speaker_review': {
          const review = detail.speaker_reviews.find((entry: any) => entry.id === args.review_id);
          if (!review) throw new Error('Review unavailable.');
          if (args.target_id) {
            if (!args.previous_id) throw new Error('Choose the earlier named speaker.');
            review.status = 'resolved';
          } else review.status = 'dismissed';
          return;
        }
        case 'search_meetings': return snapshot.meetings.filter((meeting: any) => [meeting.title, ...detail.segments.map((s: any) => s.text), ...detail.notes.map((n: any) => n.text)].join(' ').toLowerCase().includes(args.query.toLowerCase()));
        default: throw new Error(`Unexpected test IPC command: ${command}`);
      }
    }};
  });
  await page.goto('/');
});

test('recording requires consent and an explicit Bluetooth input decision', async ({ page }) => {
  await expect(page.getByRole('heading', { name: 'Your next conversation starts here' })).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.some((call: any) => call.command === 'frontend_ready'))).toBe(true);
  await page.getByRole('button', { name: 'Record your first meeting' }).click();
  const start = page.getByRole('button', { name: 'Start recording', exact: true });
  await expect(start).toBeDisabled();
  await page.getByLabel('Meeting title', { exact: true }).fill('Planning fixture');
  await page.getByRole('checkbox').check();
  await expect(start).toBeDisabled();
  await page.getByLabel('Microphone', { exact: true }).selectOption('built-in');
  await expect(start).toBeEnabled();
  await start.click();
  await expect(page.getByRole('heading', { name: 'Planning fixture' })).toBeVisible();
  await expect(page.getByRole('meter', { name: 'Microphone input level' })).toHaveAttribute('aria-valuenow', '60');
  await page.getByRole('button', { name: 'Pause', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Resume', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Resume', exact: true }).click();
  await page.getByLabel('Add a note', { exact: true }).fill('A useful fixture note');
  await page.getByRole('button', { name: 'Add note', exact: true }).click();
  await expect(page.getByText('A useful fixture note', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await page.getByRole('button', { name: 'Export', exact: true }).click();
  await expect(page.getByRole('status').filter({ hasText: 'fixture.md' })).toBeVisible();
  const startCall = await page.evaluate(() => (window as any).__testCalls.find((c: any) => c.command === 'start_recording'));
  expect(startCall.args).toMatchObject({ consent_confirmed: true, input_device: 'built-in', mode: 'remote' });
  await page.screenshot({ path: 'test-results/meeting.png', fullPage: true });
});

test('model download progress and cancellation remain actionable', async ({ page }) => {
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  await page.getByRole('button', { name: 'Download transcription model' }).click();
  await expect(page.getByRole('progressbar', { name: 'Model download progress' })).toHaveAttribute('aria-valuenow', '50');
  await page.getByRole('button', { name: 'Cancel download' }).click();
  await expect(page.getByText('Model setup was cancelled.', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Download transcription model' })).toBeEnabled();
  await page.screenshot({ path: 'test-results/settings.png', fullPage: true });
});

test('empty library renders without invented meetings and handles narrow windows', async ({ page }) => {
  await expect(page.locator('.meeting-row')).toHaveCount(0);
  await page.setViewportSize({ width: 900, height: 650 });
  await expect(page.getByRole('button', { name: 'Record your first meeting' })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.screenshot({ path: 'test-results/library.png', fullPage: true });
});

async function seedSavedMeeting(page: import('@playwright/test').Page) {
  await page.evaluate(() => {
    const fixture = (window as any).__testFixture;
    const meeting = { id: 'saved-fixture', title: 'Design fixture', status: 'ready', started_at: '2026-11-06T12:00:00Z', duration_ms: 20000, segment_count: 1 };
    const run = (id: string, kind: string) => ({ id, meeting_id: meeting.id, kind, status: 'complete', engines_json: '{}', constraints_version: 0, created_at: meeting.started_at });
    const segment = (id: string, text: string) => ({ id, track: 'mic', start_ms: 1000, end_ms: 4000, speaker: 'Me', text, words: [] });
    fixture.snapshot.meetings = [meeting];
    fixture.snapshot.models.installed = true;
    fixture.snapshot.refinement_models.installed = true;
    fixture.snapshot.settings.summary_config.provider = 'apple';
    fixture.detail = { meeting, audio_available: true, active_run_id: 'refined-run', run_id: 'refined-run', runs: [run('refined-run', 'refine'), run('live-run', 'live')], segments: [segment('current-segment', 'The project status is green.')], notes: [], summary: null, corrections: [{ id: 'review-edit', track: 'mic', start_ms: 1000, end_ms: 4000, original_text_hash: 'fixture', replacement_text: 'A retained correction for review.', status: 'needs_review', segment_id: null }], summaries: [{ id: 'older-summary', meeting_id: meeting.id, run_id: 'live-run', correction_version: 0, provider: 'apple', model: 'fixture-on-device', template: 'general', content_json: JSON.stringify({ content: { tldr: [{ text: 'Earlier supported point.', source_ids: ['old-segment'] }], key_points: [], decisions: [], action_items: [], open_questions: [] }, partial: true, errors: ['A fixture chunk was unavailable.'] }), content_md: 'Earlier supported point.', is_stale: true, created_at: meeting.started_at, source_segment_ids: ['old-segment'] }], correction_version: 0, speakers: [], speaker_turns: [], speaker_reviews: [], speaker_assignments: [], speaker_constraints: [], diarization: null };
    fixture.history['live-run'] = { ...structuredClone(fixture.detail), run_id: 'live-run', segments: [segment('old-segment', 'Original source words from the live pass.')], corrections: [] };
  });
  await page.getByRole('button', { name: /Design fixture/ }).click();
  await expect(page.getByText('The project status is green.', { exact: true })).toBeVisible();
}

test('refinement keeps the published transcript visible and exposes cancellation', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.getByRole('button', { name: 'Refine transcript', exact: true }).click();
  await expect(page.getByText('Recognizing speech · 35%', { exact: true })).toBeVisible();
  await expect(page.getByText('The project status is green.', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Refine transcript', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Cancel processing' }).click();
  await expect(page.getByRole('button', { name: 'Cancel processing' })).toHaveCount(0);
  await expect(page.getByText('The project status is green.', { exact: true })).toBeVisible();
});

test('historical transcripts are read only and unmatched overlays require an explicit choice', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.getByLabel('Transcript version').selectOption('live-run');
  await expect(page.getByText('Historical transcript · read only')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Edit transcript at 00:01' })).toHaveCount(0);
  await page.getByLabel('Transcript version').selectOption('');
  await page.getByRole('button', { name: 'Edit transcript at 00:01' }).click();
  await page.getByLabel('Correct transcript text').fill('A user correction with durable provenance.');
  await page.getByRole('button', { name: 'Save correction' }).click();
  await expect(page.getByText('A user correction with durable provenance.', { exact: true })).toBeVisible();
  const review = page.getByRole('region', { name: 'Corrections needing review' });
  await expect(review.getByRole('button', { name: 'Apply correction' })).toBeDisabled();
  await review.getByLabel('Apply correction to').selectOption('current-segment');
  await review.getByRole('button', { name: 'Apply correction', exact: true }).click();
  await expect(review).toHaveCount(0);
  const calls = await page.evaluate(() => (window as any).__testCalls);
  expect(calls.find((call: any) => call.command === 'correct_segment').args).toMatchObject({ id: 'saved-fixture', run_id: 'refined-run', segment_id: 'current-segment', replacement: 'A user correction with durable provenance.' });
  expect(calls.find((call: any) => call.command === 'resolve_correction').args).toMatchObject({ correction_id: 'review-edit', segment_id: 'current-segment' });
});

test('stale partial AI notes link to their historical source run and escape output', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.evaluate(() => {
    const fixture = (window as any).__testFixture;
    const envelope = JSON.parse(fixture.detail.summaries[0].content_json);
    envelope.content.tldr[0].text = '<img src=x onerror="window.injected=true">';
    fixture.detail.summaries[0].content_json = JSON.stringify(envelope);
    fixture.history['live-run'].bleed_decisions = [{ id: 'historical-hidden-source', segment_id: 'old-segment', decision: 'bleed', status: 'applied', reason: 'Earlier explicit user choice' }];
  });
  await page.getByRole('tab', { name: 'Summary', exact: true }).click();
  await expect(page.getByText('Stale AI notes', { exact: true })).toBeVisible();
  await expect(page.getByText('Partial AI notes', { exact: true })).toBeVisible();
  await expect(page.locator('.summary-section li').first()).toContainText('<img src=x onerror="window.injected=true">');
  await expect(page.locator('.ai-notes-content img')).toHaveCount(0);
  expect(await page.evaluate(() => Boolean((window as any).injected))).toBe(false);
  await page.screenshot({ path: 'test-results/grounded-ai-notes.png', fullPage: true });
  await page.getByRole('button', { name: 'Open source 1 for old-segment' }).click();
  await expect(page.getByLabel('Transcript version')).toHaveValue('live-run');
  await expect(page.locator('#segment-old-segment')).toHaveClass(/cited-entry/);
  await expect(page.locator('#segment-old-segment p')).toHaveText('Original source words from the live pass.');
  await expect(page.locator('#segment-old-segment')).toContainText('Confirmed bleed · shown for review');
  await expect(page.getByRole('button', { name: 'Restore speech', exact: true })).toBeDisabled();
  const historicalCall = await page.evaluate(() => (window as any).__testCalls.find((call: any) => call.command === 'meeting_detail' && call.args.run_id === 'live-run'));
  expect(historicalCall).toBeTruthy();
});

test('summary failure keeps the refined transcript healthy and hides abandoned work', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.evaluate(() => {
    const fixture = (window as any).__testFixture;
    fixture.detail.summary_error = 'The selected provider is unavailable.';
    fixture.detail.runs.push({ ...fixture.detail.runs[0], id: 'abandoned-refine', status: 'abandoned' });
  });
  await expect(page.getByText('AI notes could not be generated. Your transcript is preserved.')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Retry transcription', exact: true })).toHaveCount(0);
  await expect(page.getByLabel('Transcript version').locator('option[value="abandoned-refine"]')).toHaveCount(0);
  await page.getByRole('button', { name: 'View details', exact: true }).click();
  await expect(page.getByText('AI notes could not be generated: The selected provider is unavailable.')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Regenerate AI notes' })).toBeEnabled();
  await page.getByRole('tab', { name: /^Transcript/ }).click();
  await expect(page.getByText('The project status is green.', { exact: true })).toBeVisible();
});

test('cloud summaries require explicit consent and keys never enter local storage', async ({ page }) => {
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  await page.getByLabel('Summary provider').selectOption('open_ai');
  await page.getByLabel('Model', { exact: true }).fill('fixture-cloud-model');
  await page.getByLabel('API key', { exact: true }).fill('fixture-key-do-not-persist-in-webview');
  const save = page.getByRole('button', { name: 'Save processing preferences' });
  await expect(save).toBeDisabled();
  await page.getByRole('checkbox', { name: 'I allow transcript text and notes to be sent to this provider for AI notes.' }).check();
  await save.click();
  await expect(page.getByText('Preferences saved.', { exact: true })).toBeVisible();
  await expect(page.getByLabel('API key', { exact: true })).toHaveValue('');
  const preferences = await page.evaluate(() => (window as any).__testCalls.find((call: any) => call.command === 'update_preferences'));
  expect(preferences.args.summary_config).toMatchObject({ provider: 'open_ai', cloud_consent: true });
  expect(preferences.args.api_key).toBe('fixture-key-do-not-persist-in-webview');
  expect(await page.evaluate(() => JSON.stringify(localStorage))).not.toContain('fixture-key');
  await page.getByLabel('Server endpoint').fill('https://fixture.example/v1');
  await page.getByLabel('Server endpoint').press('Tab');
  await expect(save).toBeDisabled();
  await page.getByRole('checkbox', { name: 'I allow transcript text and notes to be sent to this provider for AI notes.' }).check();
  await page.getByRole('checkbox', { name: 'Remove the stored key for this provider and endpoint' }).check();
  await save.click();
  await expect(page.getByText('Preferences saved.', { exact: true })).toBeVisible();
  const removal = await page.evaluate(() => (window as any).__testCalls.filter((call: any) => call.command === 'update_preferences').at(-1));
  expect(removal.args).toMatchObject({ clear_key: true, summary_config: { provider: 'open_ai', endpoint: 'https://fixture.example/v1' } });
  expect(removal.args.api_key).toBeUndefined();
  await page.getByLabel('Summary provider').selectOption('compatible');
  await expect(save).toBeDisabled();
  await page.getByLabel('Summary provider').selectOption('none');
  await expect(save).toBeEnabled();
  await save.click();
  await expect(page.getByText('Preferences saved.', { exact: true })).toBeVisible();
});

test('refinement pack progress and server discovery never enable providers automatically', async ({ page }) => {
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  await page.getByRole('button', { name: 'Download refinement model' }).click();
  await expect(page.getByRole('progressbar', { name: 'Refinement model download progress' })).toHaveAttribute('aria-valuenow', '40');
  await page.getByRole('button', { name: 'Cancel refinement download' }).click();
  await expect(page.getByText('Refinement setup was cancelled.', { exact: true })).toBeVisible();
  await page.getByLabel('Summary provider').selectOption('local');
  await page.getByRole('button', { name: 'http://127.0.0.1:11434/v1 · 1 models' }).click();
  await expect(page.getByLabel('Model', { exact: true })).toHaveValue('fixture-local-model');
  expect(await page.evaluate(() => (window as any).__testCalls.some((call: any) => call.command === 'update_preferences'))).toBe(false);
  await page.screenshot({ path: 'test-results/refinement-settings.png', fullPage: true });
});

test('library search invokes transcript and note search rather than filtering titles', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.getByRole('button', { name: 'Meeting library', exact: false }).first().click();
  await page.getByLabel('Search meetings', { exact: true }).fill('green');
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.some((call: any) => call.command === 'search_meetings' && call.args.query === 'green'))).toBe(true);
  await expect(page.getByRole('button', { name: /Design fixture/ })).toBeVisible();
});


test('live partials are visually distinct and never expose editing controls', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.evaluate(() => {
    const fixture = (window as any).__testFixture;
    fixture.detail.meeting.status = 'recording';
    fixture.detail.runs[0].kind = 'live';
    fixture.detail.runs[0].status = 'running';
    fixture.detail.segments.push({ id: 'partial-mic', track: 'mic', start_ms: 4000, end_ms: 5000, speaker: 'Me', text: 'Words still being recognized', words: [] });
    fixture.snapshot.recording = { id: 'saved-fixture', title: 'Design fixture', status: 'recording', elapsed_ms: 5000, mic_level: 0.4, system_level: 0.1, gaps: 0, transcription_configured: true, transcription_active: true };
  });
  await expect(page.locator('.partial-entry')).toContainText('Words still being recognized');
  await expect(page.locator('.partial-entry p')).toHaveCSS('font-style', 'italic');
  await expect(page.locator('.partial-entry')).toContainText('In progress');
  await expect(page.getByRole('button', { name: /^Edit transcript at/ })).toHaveCount(0);
  await page.getByRole('tab', { name: 'Summary', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Regenerate AI notes', exact: true })).toBeDisabled();
});

test('stopped live transcription is shown in the transcript pane with existing segments', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.evaluate(() => {
    const fixture = (window as any).__testFixture;
    fixture.detail.meeting.status = 'recording';
    fixture.detail.runs[0].kind = 'live';
    fixture.detail.runs[0].status = 'running';
    fixture.snapshot.recording = { id: 'saved-fixture', title: 'Design fixture', status: 'recording', elapsed_ms: 2861000, mic_level: 0.4, system_level: 0.1, gaps: 0, transcription_configured: true, transcription_active: false, live_transcription: { state: 'stopped', since_source_ms: 2861000, reason: 'disk_protection' } };
  });
  const notice = page.locator('.live-stopped');
  await expect(notice).toContainText('Live transcription stopped at 00:47:41');
  await expect(notice).toContainText('Audio is still being recorded');
  await expect(notice).toContainText('produced from the saved audio after you stop recording');
  await expect(notice).toContainText('Disk space was running low');
  await expect(notice).not.toContainText('disk_protection');
  await expect(notice).toHaveAttribute('role', 'status');
  await expect(page.locator('.transcript-hint')).toHaveText('Live transcription stopped');
  await expect(page.locator('.transcript-entry').first()).toBeVisible();
  await page.screenshot({ path: 'test-results/live-transcription-stopped.png', fullPage: true });
});

test('stopped live transcription with no segments replaces the listening empty state', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.evaluate(() => {
    const fixture = (window as any).__testFixture;
    fixture.detail.meeting.status = 'recording';
    fixture.detail.segments = [];
    fixture.snapshot.recording = { id: 'saved-fixture', title: 'Design fixture', status: 'recording', elapsed_ms: 70000, mic_level: 0, system_level: 0, gaps: 0, transcription_configured: true, transcription_active: true, live_transcription: { state: 'stopped', since_source_ms: 65000, reason: 'worker_failed' } };
  });
  await expect(page.locator('.live-stopped')).toContainText('Live transcription stopped at 00:01:05');
  await expect(page.locator('.live-stopped')).toContainText('A processing fault interrupted live transcription.');
  await expect(page.getByRole('heading', { name: 'Listening for the conversation' })).toHaveCount(0);
  await expect(page.locator('.transcript-entry')).toHaveCount(0);
});

test('capture stall warning appears with live outage time and clears on recovery', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.evaluate(() => {
    const fixture = (window as any).__testFixture;
    fixture.detail.meeting.status = 'recording';
    fixture.snapshot.recording = { id: 'saved-fixture', title: 'Design fixture', status: 'recording', elapsed_ms: 100000, mic_level: 0, system_level: 0, gaps: 0, transcription_configured: true, transcription_active: true, capture: { state: 'ok' } };
  });
  await expect(page.locator('.capture-stall')).toHaveCount(0);
  await page.evaluate(() => { (window as any).__testFixture.snapshot.recording = { ...(window as any).__testFixture.snapshot.recording, elapsed_ms: 147000, capture: { state: 'stalled', since_source_ms: 100000, kind: 'helper_silent' } }; });
  const warning = page.locator('.capture-stall');
  await expect(warning).toHaveAttribute('role', 'alert');
  await expect(warning).toContainText('No audio is being captured right now');
  await expect(warning).toContainText('Nothing has been recorded for 00:00:47');
  await expect(warning).toContainText('Check your microphone or audio device');
  await page.evaluate(() => { (window as any).__testFixture.snapshot.recording = { ...(window as any).__testFixture.snapshot.recording, elapsed_ms: 152000 }; });
  await expect(warning).toContainText('Nothing has been recorded for 00:00:52');
  await page.screenshot({ path: 'test-results/capture-stalled.png', fullPage: true });
  await page.evaluate(() => { (window as any).__testFixture.snapshot.recording = { ...(window as any).__testFixture.snapshot.recording, elapsed_ms: 153000, capture: { state: 'ok' } }; });
  await expect(page.locator('.capture-stall')).toHaveCount(0);
});

test('consumer_slow is not shown as a capture outage; absent fields render nothing', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.evaluate(() => {
    const fixture = (window as any).__testFixture;
    fixture.detail.meeting.status = 'recording';
    fixture.snapshot.recording = { id: 'saved-fixture', title: 'Design fixture', status: 'recording', elapsed_ms: 9000, mic_level: 0, system_level: 0, gaps: 0, transcription_configured: true, transcription_active: true, capture: { state: 'consumer_slow', since_source_ms: 5000, kind: 'consumer_slow' } };
  });
  await expect(page.getByText('Audio processing is running slowly. Recording is not affected.')).toBeVisible();
  await expect(page.locator('.capture-stall')).toHaveCount(0);
  await page.evaluate(() => { const f = (window as any).__testFixture; const r = { ...f.snapshot.recording, live_transcription: null }; delete r.capture; f.snapshot.recording = r; });
  await expect(page.getByText('Audio processing is running slowly.')).toHaveCount(0);
  await expect(page.locator('.live-stopped')).toHaveCount(0);
  await page.evaluate(() => { const f = (window as any).__testFixture; const r = { ...f.snapshot.recording }; delete r.live_transcription; f.snapshot.recording = r; });
  await expect(page.locator('.live-stopped, .capture-stall')).toHaveCount(0);
  await expect(page.getByText('Saved as the conversation unfolds')).toBeVisible();
});

async function setAudioRange(control: import('@playwright/test').Locator, value: string) {
  await expect(control).toBeEnabled();
  await control.evaluate((element, next) => {
    const input = element as HTMLInputElement;
    input.value = next;
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new Event('change', { bubbles: true }));
  }, value);
}

test('saved audio shows both waveforms and supports transcript seek, pause, scrub and track volume', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.getByRole('button', { name: 'Play from 00:01', exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.find((call: any) => call.command === 'play_meeting_audio')?.args)).toEqual({ id: 'saved-fixture', position_ms: 1000 });
  await page.getByRole('tab', { name: 'Audio', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Meeting audio' })).toBeVisible();
  for (const label of ['Microphone waveform', 'System audio waveform']) {
    const waveform = page.getByRole('img', { name: label });
    await expect(waveform).toBeVisible();
    await expect(waveform.locator('.waveform-peaks')).not.toHaveAttribute('d', '');
  }
  await page.screenshot({ path: 'test-results/native-playback-fixture.png', fullPage: true });
  await page.getByRole('button', { name: 'Pause playback', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Play audio', exact: true })).toBeEnabled();
  await setAudioRange(page.getByRole('slider', { name: 'Playback position', exact: true }), '7000');
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.filter((call: any) => call.command === 'seek_meeting_audio').at(-1)?.args)).toEqual({ id: 'saved-fixture', position_ms: 7000 });
  await expect(page.getByRole('slider', { name: 'Playback position', exact: true })).toHaveValue('7000');
  await setAudioRange(page.getByRole('slider', { name: /Microphone volume/ }), '0.4');
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.filter((call: any) => call.command === 'set_playback_volumes').at(-1)?.args)).toEqual({ id: 'saved-fixture', mic_volume: 0.4, system_volume: 1 });
  await setAudioRange(page.getByRole('slider', { name: /System volume/ }), '0.25');
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.filter((call: any) => call.command === 'set_playback_volumes').at(-1)?.args)).toEqual({ id: 'saved-fixture', mic_volume: 0.4, system_volume: 0.25 });
  await page.getByRole('button', { name: 'Play audio', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Pause playback', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'Stop playback', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Play audio', exact: true })).toBeEnabled();
  const calls = await page.evaluate(() => (window as any).__testCalls);
  expect(calls.some((call: any) => call.command === 'pause_meeting_audio')).toBe(true);
  expect(calls.find((call: any) => call.command === 'meeting_waveform').args).toEqual({ id: 'saved-fixture', bins: 512 });
  expect(calls.filter((call: any) => call.command === 'play_meeting_audio').at(-1).args).toEqual({ id: 'saved-fixture', position_ms: 7000 });
  expect(calls.some((call: any) => call.command === 'stop_meeting_audio')).toBe(true);
});

for (const status of ['recording', 'paused', 'processing']) {
  test(`all saved-audio controls are disabled while another meeting is ${status}`, async ({ page }) => {
    await seedSavedMeeting(page);
    await page.evaluate((recordingStatus) => {
      const fixture = (window as any).__testFixture;
      fixture.snapshot.playback = { meeting_id: 'saved-fixture', status: 'playing', position_ms: 1000, duration_ms: 20000, mic_volume: 1, system_volume: 1, error: null };
      fixture.snapshot.recording = { id: 'other-active-meeting', title: 'Different active meeting', status: recordingStatus, elapsed_ms: 5000, mic_level: 0, system_level: 0, gaps: 0, transcription_configured: false, transcription_active: false, error: null };
    }, status);
    await expect(page.getByRole('button', { name: 'Play from 00:01', exact: true })).toBeDisabled();
    await page.getByRole('tab', { name: 'Audio', exact: true }).click();
    await expect(page.getByText('Playback is disabled while any recording is active, including while paused or stopping.', { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Pause playback', exact: true })).toBeDisabled();
    await expect(page.getByRole('button', { name: 'Stop playback', exact: true })).toBeDisabled();
    await expect(page.getByRole('slider', { name: 'Playback position', exact: true })).toBeDisabled();
    await expect(page.getByRole('slider', { name: /Microphone volume/ })).toBeDisabled();
    await expect(page.getByRole('slider', { name: /System volume/ })).toBeDisabled();
    const sent = await page.evaluate(() => (window as any).__testCalls.filter((call: any) => ['play_meeting_audio', 'pause_meeting_audio', 'seek_meeting_audio', 'set_playback_volumes', 'meeting_waveform'].includes(call.command)));
    expect(sent).toHaveLength(0);
  });
}

test('voice detection and provisional speakers require explicit setup and speaker source acknowledgement', async ({ page }) => {
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  const vad = page.getByRole('checkbox', { name: 'Use live voice activity detection when its model is installed', exact: true });
  const speakers = page.getByRole('checkbox', { name: 'Use provisional speaker labels when the speaker models are installed', exact: true });
  await expect(vad).toBeChecked();
  await expect(speakers).not.toBeChecked();
  const download = page.getByRole('button', { name: 'Download speaker models', exact: true });
  await expect(download).toBeDisabled();
  expect(await page.evaluate(() => (window as any).__testCalls.some((call: any) => ['install_vad_model', 'install_diarization_model', 'update_audio_preferences'].includes(call.command)))).toBe(false);
  await page.getByText('Speaker model sources and license notice', { exact: true }).click();
  await expect(page.getByText(/Clustering thresholds are experimental/)).toBeVisible();
  await page.getByRole('checkbox', { name: 'I have read the speaker model source and license notice.', exact: true }).check();
  await download.click();
  await expect(page.getByRole('progressbar', { name: 'Speaker model download progress' })).toHaveJSProperty('value', 0.3);
  await page.getByRole('button', { name: 'Cancel speaker model download', exact: true }).click();
  await expect(page.getByText('Speaker setup was cancelled.', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Download voice activity model', exact: true }).click();
  await expect(page.getByRole('progressbar', { name: 'Voice activity model download progress' })).toHaveJSProperty('value', 0.6);
  await page.getByRole('button', { name: 'Cancel voice activity download', exact: true }).click();
  await expect(page.getByText('Voice activity setup was cancelled.', { exact: true })).toBeVisible();
  await vad.uncheck();
  await speakers.check();
  await page.getByRole('button', { name: 'Save audio processing preferences', exact: true }).click();
  await expect(page.getByText('Audio processing preferences saved. Changes apply to the next recording or refinement.', { exact: true })).toBeVisible();
  const calls = await page.evaluate(() => (window as any).__testCalls);
  expect(calls.find((call: any) => call.command === 'install_diarization_model').args).toEqual({ acknowledge_terms: true });
  expect(calls.find((call: any) => call.command === 'update_audio_preferences').args).toEqual({ live_vad_enabled: false, diarization_enabled: true, echo_cancellation_enabled: true });
});

test('an installed voice detection pack keeps the default preference on without enabling speakers', async ({ page }) => {
  await page.evaluate(() => { (window as any).__testFixture.snapshot.vad_models.installed = true; });
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  await expect(page.locator('.model-panel').filter({ hasText: 'Silero voice activity detection' }).getByText('Installed', { exact: true })).toBeVisible();
  await expect(page.getByRole('checkbox', { name: 'Use live voice activity detection when its model is installed', exact: true })).toBeChecked();
  await expect(page.getByRole('checkbox', { name: 'Use provisional speaker labels when the speaker models are installed', exact: true })).not.toBeChecked();
  expect(await page.evaluate(() => (window as any).__testCalls.some((call: any) => call.command === 'update_audio_preferences'))).toBe(false);
});

async function seedSpeakerMeeting(page: import('@playwright/test').Page) {
  await seedSavedMeeting(page);
  await page.evaluate(() => {
    const fixture = (window as any).__testFixture;
    const segment = (id: string, start: number, text: string, speaker: string) => ({ id, track: 'system', start_ms: start, end_ms: start + 1500, speaker, text, words: [{ text, start_ms: start + 100, end_ms: start + 800 }] });
    const turn = (id: string, start: number, speaker: string, label: string) => ({ id, track: 'system', start_ms: start, end_ms: start + 2000, speaker_ids: [speaker], label, provenance: 'audio-derived', score: 0.9, overlap: false, uncertain: false, provisional: false });
    const speakers = [{ id: 'sp-a', track: 'system', label: 'Speaker 1', user_name: null, speech_ms: 4000, provisional: false }, { id: 'sp-b', track: 'system', label: 'Speaker 2', user_name: null, speech_ms: 2000, provisional: false }];
    const segments = [segment('current-segment', 1000, 'The project status is green.', 'Speaker 1'), segment('other-segment', 4000, 'A different participant responds.', 'Speaker 2'), segment('repeat-segment', 7000, 'The first participant speaks again.', 'Speaker 1')];
    const assignments = segments.map((entry, index) => ({ segment_id: entry.id, speaker_ids: [index === 1 ? 'sp-b' : 'sp-a'], provenance: 'audio-derived', overlap: false, uncertain: false, words: [{ index: 0, speaker_ids: [index === 1 ? 'sp-b' : 'sp-a'], label: entry.speaker, overlap: false, uncertain: false }] }));
    fixture.detail.segments = segments;
    fixture.detail.speakers = speakers;
    fixture.detail.speaker_turns = [turn('turn-a', 1000, 'sp-a', 'Speaker 1'), turn('turn-b', 4000, 'sp-b', 'Speaker 2'), turn('turn-repeat', 7000, 'sp-a', 'Speaker 1')];
    fixture.detail.speaker_assignments = assignments;
    fixture.detail.speaker_reviews = [{ id: 'split-review', kind: 'split', previous_ids: ['prior-alice'], previous_labels: ['Alice'], current_ids: ['sp-a', 'sp-b'], reason: 'A named earlier speaker split into two current clusters.', status: 'needs_review' }, { id: 'dismiss-review', kind: 'ambiguous', previous_ids: ['prior-alice'], previous_labels: ['Alice'], current_ids: ['sp-b'], reason: 'The earlier name cannot be inherited without a choice.', status: 'needs_review' }];
    fixture.detail.diarization = { embedding_version: 'public-synthetic-fixture', calibrated: false, warnings: ['Synthetic speaker labels have not been calibrated.'] };
    const historical = fixture.history['live-run'];
    historical.speakers = [{ id: 'prior-alice', track: 'system', label: 'Alice', user_name: 'Alice', speech_ms: 6000, provisional: false }];
    historical.speaker_turns = [turn('historical-turn', 1000, 'prior-alice', 'Alice')];
    historical.speaker_assignments = [{ segment_id: 'old-segment', speaker_ids: ['prior-alice'], provenance: 'user', overlap: false, uncertain: false, words: [] }];
    historical.speaker_reviews = [];
    historical.diarization = structuredClone(fixture.detail.diarization);
    fixture.snapshot.diarization_models.installed = true;
  });
  await page.getByRole('tab', { name: 'Speakers', exact: true }).click();
  await expect(page.getByRole('textbox', { name: 'Name for Speaker 1', exact: true })).toBeVisible();
}

test('speaker names update every assigned transcript line and remain meeting-local suggestions', async ({ page }) => {
  await seedSpeakerMeeting(page);
  await expect(page.getByText('Meeting names stay in this meeting. Saving a voiceprint requires a separate, explicit enrollment with consent.', { exact: true })).toBeVisible();
  await page.screenshot({ path: 'test-results/speaker-management-fixture.png', fullPage: true });
  await page.getByRole('textbox', { name: 'Name for Speaker 1', exact: true }).fill('Alice');
  await page.getByRole('button', { name: 'Save name', exact: true }).first().click();
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.find((call: any) => call.command === 'rename_speaker')?.args)).toEqual({ id: 'saved-fixture', speaker_id: 'sp-a', name: 'Alice' });
  await page.getByRole('tab', { name: /^Transcript/ }).click();
  await expect(page.locator('#segment-current-segment .speaker-label-button')).toHaveText('Alice');
  await expect(page.locator('#segment-repeat-segment .speaker-label-button')).toHaveText('Alice');
  await expect(page.locator('#segment-other-segment .speaker-label-button')).toHaveText('Speaker 2');
  expect(await page.evaluate(() => JSON.stringify(localStorage))).not.toContain('Alice');
});

test('same-person and different-people choices dispatch clean temporal excerpt constraints', async ({ page }) => {
  await seedSpeakerMeeting(page);
  const save = page.getByRole('button', { name: 'Save speaker correction', exact: true });
  await expect(save).toBeDisabled();
  await page.getByRole('combobox', { name: 'First speaker excerpt', exact: true }).selectOption('turn-a');
  await page.getByRole('combobox', { name: 'Second speaker excerpt', exact: true }).selectOption('turn-repeat');
  await page.getByRole('combobox', { name: 'Speaker relationship', exact: true }).selectOption('must_link');
  await save.click();
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.filter((call: any) => call.command === 'add_speaker_constraint').at(-1)?.args)).toEqual({ id: 'saved-fixture', run_id: 'refined-run', left_turn_id: 'turn-a', right_turn_id: 'turn-repeat', kind: 'must_link' });
  const saved = page.getByRole('region', { name: 'Saved speaker corrections', exact: true });
  await expect(saved.locator('[data-constraint-id="constraint-1"]')).toBeVisible();
  await saved.getByRole('button', { name: 'Remove speaker correction', exact: true }).click();
  await expect(saved.locator('[data-constraint-id="constraint-1"]')).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.find((call: any) => call.command === 'revoke_speaker_constraint')?.args)).toEqual({ id: 'saved-fixture', constraint_id: 'constraint-1' });
  await page.getByRole('combobox', { name: 'First speaker excerpt', exact: true }).selectOption('turn-a');
  await page.getByRole('combobox', { name: 'Second speaker excerpt', exact: true }).selectOption('turn-b');
  await page.getByRole('combobox', { name: 'Speaker relationship', exact: true }).selectOption('cannot_link');
  await save.click();
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.filter((call: any) => call.command === 'add_speaker_constraint').at(-1)?.args)).toEqual({ id: 'saved-fixture', run_id: 'refined-run', left_turn_id: 'turn-a', right_turn_id: 'turn-b', kind: 'cannot_link' });
  await expect(saved.locator('[data-constraint-id="constraint-2"]')).toContainText('Different people');
});

test('speaker reconciliation reviews require explicit earlier-name and current-speaker choices or dismissal', async ({ page }) => {
  await seedSpeakerMeeting(page);
  await expect(page.getByRole('button', { name: 'Keep selected name', exact: true }).first()).toBeDisabled();
  const previous = page.getByLabel('Previous name for review split-review', { exact: true });
  await expect(previous.getByRole('option', { name: 'Alice', exact: true })).toBeAttached();
  await previous.selectOption('prior-alice');
  await expect(page.getByRole('button', { name: 'Keep selected name', exact: true }).first()).toBeDisabled();
  await page.getByLabel('Current speaker for review split-review', { exact: true }).selectOption('sp-a');
  await page.getByRole('button', { name: 'Keep selected name', exact: true }).first().click();
  await expect(page.getByLabel('Previous name for review split-review', { exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Dismiss review', exact: true }).click();
  await expect(page.getByLabel('Previous name for review dismiss-review', { exact: true })).toHaveCount(0);
  const calls = await page.evaluate(() => (window as any).__testCalls.filter((call: any) => call.command === 'resolve_speaker_review'));
  expect(calls[0].args).toEqual({ id: 'saved-fixture', run_id: 'refined-run', review_id: 'split-review', previous_id: 'prior-alice', target_id: 'sp-a' });
  expect(calls[1].args).toEqual({ id: 'saved-fixture', run_id: 'refined-run', review_id: 'dismiss-review' });
});

test('historical speaker annotations are read-only and current excerpts cannot play during another recording', async ({ page }) => {
  await seedSpeakerMeeting(page);
  await page.getByRole('button', { name: 'Play speaker excerpt at 00:01', exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.find((call: any) => call.command === 'play_meeting_audio')?.args)).toEqual({ id: 'saved-fixture', position_ms: 1000 });
  await page.getByLabel('Transcript version').selectOption('live-run');
  await expect(page.getByText('Historical transcript · read only')).toBeVisible();
  await expect(page.getByRole('textbox', { name: 'Name for Alice', exact: true })).toBeVisible();
  for (const control of await page.getByRole('textbox', { name: /^Name for/ }).all()) await expect(control).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Save name', exact: true }).and(page.locator(':enabled'))).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Save speaker correction', exact: true }).and(page.locator(':enabled'))).toHaveCount(0);
  await page.getByLabel('Transcript version').selectOption('');
  await page.evaluate(() => {
    const fixture = (window as any).__testFixture;
    fixture.snapshot.recording = { id: 'another-meeting', title: 'Another recording', status: 'paused', elapsed_ms: 5000, mic_level: 0, system_level: 0, gaps: 0, transcription_configured: false, transcription_active: false, error: null };
  });
  await expect(page.getByRole('button', { name: 'Play speaker excerpt at 00:01', exact: true })).toBeDisabled();
  expect(await page.evaluate(() => (window as any).__testCalls.some((call: any) => ['rename_speaker', 'add_speaker_constraint', 'resolve_speaker_review'].includes(call.command)))).toBe(false);
});


test('an unresolved speaker constraint offers removal without unrelated name inheritance choices', async ({ page }) => {
  await seedSpeakerMeeting(page);
  await page.evaluate(() => {
    const fixture = (window as any).__testFixture;
    fixture.detail.speaker_constraints.push({ id: 'constraint-pending', kind: 'cannot_link', left: { track: 'system', start_ms: 1000, end_ms: 3000 }, right: { track: 'system', start_ms: 4000, end_ms: 6000 }, active: true });
    fixture.detail.speaker_reviews.push({ id: 'constraint-review', kind: 'constraint', constraint_id: 'constraint-pending', previous_ids: [], previous_labels: [], current_ids: [], reason: 'The saved excerpts could not be matched safely. Affected speech remains Unknown until reviewed.', status: 'needs_review' });
  });
  const review = page.getByRole('article', { name: 'Speaker correction review constraint-review', exact: true });
  await expect(review).toBeVisible();
  await expect(review).toContainText('Affected speech remains Unknown');
  await expect(review.getByRole('combobox')).toHaveCount(0);
  await expect(review.getByRole('button', { name: 'Keep selected name', exact: true })).toHaveCount(0);
  await review.getByRole('button', { name: 'Remove affected speaker correction', exact: true }).click();
  await expect(review).toHaveCount(0);
  await expect(page.locator('[data-constraint-id="constraint-pending"]')).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.filter((call: any) => call.command === 'revoke_speaker_constraint').at(-1)?.args)).toEqual({ id: 'saved-fixture', constraint_id: 'constraint-pending' });
});


test('echo processing preference is explicit and applies to the next recording or refinement', async ({ page }) => {
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  const echo=page.getByRole('checkbox', {name:'Reduce microphone echo when the built-in speaker route is verified',exact:true});
  await expect(echo).toBeChecked();
  await expect(page.getByText('Headphones and unverified routes skip echo cancellation. Processing leaves original audio unchanged. Storage retention preferences are separate.', {exact:true})).toBeVisible();
  await echo.uncheck();
  await page.getByRole('button',{name:'Save audio processing preferences',exact:true}).click();
  await expect(page.getByText('Audio processing preferences saved. Changes apply to the next recording or refinement.',{exact:true})).toBeVisible();
  expect(await page.evaluate(()=>(window as any).__testCalls.filter((call:any)=>call.command==='update_audio_preferences').at(-1).args.echo_cancellation_enabled)).toBe(false);
  expect(await page.evaluate(()=>(window as any).__testCalls.filter((call:any)=>['start_recording','play_meeting_audio','install_model'].includes(call.command)))).toHaveLength(0);
});


async function seedBleedMeeting(page: import('@playwright/test').Page) {
  await seedSavedMeeting(page);
  await page.evaluate(() => {
    const f=(window as any).__testFixture;
    f.detail.segments.push({id:'echo-mic',track:'mic',start_ms:4000,end_ms:6000,text:'The delivery window opens on Friday.',speaker:'Me',words:[]},{id:'echo-system',track:'system',start_ms:3800,end_ms:5800,text:'The delivery window opens on Friday.',speaker:'Them',words:[]},{id:'short-reply',track:'mic',start_ms:7000,end_ms:7600,text:'Yes, okay.',speaker:'Me',words:[]});
    f.detail.bleed_candidates=[{mic_segment_id:'echo-mic',system_segment_id:'echo-system',similarity:1}];
    f.detail.bleed_decisions=[];
    f.detail.echo={enabled:true,applied_mic_ms:5000,bypassed_mic_ms:2000,engine:'synthetic-test-engine',warnings:['Synthetic route evidence was unavailable for part of the recording.']};
  });
  await page.getByRole('tab',{name:'Audio',exact:true}).click();
  await page.getByRole('tab',{name:/^Transcript/}).click();
  await expect(page.locator('[data-bleed-segment="echo-mic"]')).toBeVisible();
}

test('bleed stays visible until confirmed and can be restored without losing a short reply',async({page})=>{
  await seedBleedMeeting(page);
  await expect(page.locator('#segment-echo-mic')).toBeVisible();
  await page.screenshot({ path: 'test-results/echo-bleed-review-fixture.png', fullPage: true });
  await expect(page.locator('#segment-short-reply')).toHaveText(/Yes, okay/);
  const card=page.locator('[data-bleed-segment="echo-mic"]');
  await card.getByRole('button',{name:'Play microphone from here',exact:true}).click();
  await expect.poll(()=>page.evaluate(()=>(window as any).__testCalls.filter((c:any)=>c.command==='set_playback_volumes').at(-1)?.args)).toEqual({id:'saved-fixture',mic_volume:1,system_volume:0});
  await card.getByRole('button',{name:'Hide as probable bleed',exact:true}).click();
  await expect(page.locator('#segment-echo-mic')).toHaveCount(0);
  await expect(page.locator('#segment-echo-system')).toBeVisible();
  await expect(page.locator('#segment-short-reply')).toBeVisible();
  await page.getByRole('button',{name:'Show hidden bleed (1)',exact:true}).click();
  await expect(page.locator('#segment-echo-mic')).toBeVisible();
  await page.locator('[data-hidden-bleed="echo-mic"]').getByRole('button',{name:'Restore speech',exact:true}).click();
  await expect(page.locator('#segment-echo-mic')).toBeVisible();
  await expect(page.getByRole('button',{name:/hidden bleed/})).toHaveCount(0);
  expect(await page.evaluate(()=>(window as any).__testCalls.filter((c:any)=>c.command==='set_bleed_decision').map((c:any)=>c.args.decision))).toEqual(['bleed','genuine']);
});

test('unmatched bleed can stay visible and saved echo provenance starts no playback',async({page})=>{
  await seedBleedMeeting(page);
  await page.evaluate(()=>{const d=(window as any).__testFixture.detail;d.bleed_candidates=[];d.bleed_decisions=[{id:'pending-bleed',segment_id:null,decision:'bleed',status:'needs_review',reason:'The earlier source has no unique exact match.'}];});
  await page.getByRole('tab',{name:'Audio',exact:true}).click();
  await expect(page.getByRole('region',{name:'Transcript echo processing',exact:true})).toContainText('Applied to 00:05 of microphone audio; skipped for 00:02');
  expect(await page.evaluate(()=>(window as any).__testCalls.filter((c:any)=>c.command==='play_meeting_audio'))).toHaveLength(0);
  await page.getByRole('tab',{name:/^Transcript/}).click();
  await page.locator('[data-bleed-review="pending-bleed"]').getByRole('button',{name:'Keep this speech visible',exact:true}).click();
  await expect(page.locator('[data-bleed-review="pending-bleed"]')).toHaveCount(0);
  await expect(page.locator('#segment-echo-mic')).toBeVisible();
});


test('genuine-speech decisions remain reachable beyond the first review page',async({page})=>{
  await seedBleedMeeting(page);
  await page.evaluate(()=>{
    const d=(window as any).__testFixture.detail;
    d.bleed_candidates=[];d.bleed_decisions=[];
    for(let i=0;i<10;i++){
      d.segments.push({id:`genuine-${i}`,track:'mic',start_ms:8000+i*500,end_ms:8400+i*500,text:`Genuine speech number ${i} remains available.`,speaker:'Me',words:[]});
      d.bleed_decisions.push({id:`decision-genuine-${i}`,segment_id:`genuine-${i}`,decision:'genuine',status:'applied',reason:'Synthetic explicit genuine choice'});
    }
  });
  await page.getByRole('tab',{name:'Audio',exact:true}).click();
  await page.getByRole('tab',{name:/^Transcript/}).click();
  await page.getByText('Kept as genuine speech (10)',{exact:true}).click();
  await expect(page.locator('[data-genuine-bleed]')).toHaveCount(8);
  await page.getByRole('button',{name:'Next genuine lines',exact:true}).click();
  await expect(page.locator('[data-genuine-bleed]')).toHaveCount(2);
  await page.locator('[data-genuine-bleed="genuine-9"]').getByRole('button',{name:'Mark as probable bleed',exact:true}).click();
  await expect.poll(()=>page.evaluate(()=>(window as any).__testCalls.filter((c:any)=>c.command==='set_bleed_decision').at(-1)?.args.segment_id)).toBe('genuine-9');
});

test('privacy defaults keep audio and allow keeping every transcript version independently', async ({ page }) => {
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  const privacy = page.getByRole('region', { name: 'Privacy and retention' });
  await expect(privacy.getByRole('radio', { name: 'Keep all audio', exact: true })).toBeChecked();
  await expect(privacy.getByRole('radio', { name: 'Clean up unreferenced old versions' })).toBeChecked();
  await expect(privacy.getByLabel('Version retention days', { exact: true })).toHaveValue('7');
  await privacy.getByRole('radio', { name: 'Remove audio after a set number of days' }).check();
  await privacy.getByLabel('Audio retention days', { exact: true }).fill('30');
  await privacy.getByRole('radio', { name: 'Keep all transcript versions', exact: true }).check();
  await expect(privacy.getByRole('button', { name: 'Preview eligible cleanup' })).toBeDisabled();
  await privacy.getByRole('button', { name: 'Save retention preferences' }).click();
  await expect(privacy.getByRole('status').filter({ hasText: 'Retention preferences saved' })).toBeVisible();
  const calls = await page.evaluate(() => (window as any).__testCalls);
  expect(calls.find((c: any) => c.command === 'save_maintenance_policy').args.policy).toEqual({ audio_keep_days: 30, obsolete_run_days: 7, cleanup_obsolete_runs: false });
  expect(calls.some((c: any) => ['commit_deletion', 'apply_cleanup'].includes(c.command))).toBe(false);
  await privacy.scrollIntoViewIfNeeded();
  await privacy.screenshot({ path: '/private/tmp/vantranscribe-privacy-settings.png' });
});

test('audio removal requires reviewed confirmation and keeps transcript and citations available', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.getByText('Stored meeting data', { exact: true }).click();
  await page.getByRole('button', { name: 'Review audio removal' }).click();
  let dialog = page.getByRole('dialog', { name: 'Remove saved audio?' });
  await expect(dialog.getByRole('button', { name: 'Remove audio', exact: true })).toBeDisabled();
  await expect(dialog.getByText('Exported copies and backups remain where saved.', { exact: false })).toBeVisible();
  await page.keyboard.press('Meta+Alt+r');
  await expect(page.getByRole('button', { name: 'Start recording', exact: true })).toHaveCount(0);
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  expect(await page.evaluate(() => (window as any).__testCalls.some((c: any) => c.command === 'commit_deletion'))).toBe(false);
  await page.getByRole('button', { name: 'Review audio removal' }).click();
  dialog = page.getByRole('dialog', { name: 'Remove saved audio?' });
  await dialog.getByRole('checkbox', { name: 'I understand that this removal cannot be undone.' }).check();
  await page.screenshot({ path: '/private/tmp/vantranscribe-audio-removal-confirmation.png', fullPage: true });
  await dialog.getByRole('button', { name: 'Remove audio', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(page.getByText('The project status is green.', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Refine transcript', exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Play from 00:01', exact: true })).toBeDisabled();
  await page.getByRole('tab', { name: 'Audio', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Saved audio is unavailable' })).toBeVisible();
  expect(await page.evaluate(() => (window as any).__testCalls.some((c: any) => c.command === 'meeting_waveform'))).toBe(false);
  await page.getByRole('tab', { name: 'Summary', exact: true }).click();
  await page.locator('.citation-button').first().click();
  await expect(page.getByText('Original source words from the live pass.', { exact: true })).toBeVisible();
});

test('whole meeting deletion stays preview-only until explicit confirmation and returns to library', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.getByText('Stored meeting data', { exact: true }).click();
  await page.getByRole('button', { name: 'Review meeting deletion' }).click();
  const dialog = page.getByRole('dialog', { name: 'Delete this meeting?' });
  await expect(dialog.getByText('This removes the meeting’s audio, transcript versions, notes, summaries and annotations from this recordings folder.')).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Delete meeting', exact: true })).toBeDisabled();
  await dialog.getByRole('checkbox').check();
  await dialog.getByRole('button', { name: 'Delete meeting', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Your next conversation starts here' })).toBeVisible();
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'commit_deletion').length)).toBe(1);
});

test('stale deletion previews show the error and require a fresh review', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.evaluate(() => { (window as any).__testFixture.commitError = 'Deletion preview is stale. Review a fresh preview.'; });
  await page.getByText('Stored meeting data', { exact: true }).click();
  await page.getByRole('button', { name: 'Review audio removal' }).click();
  const dialog = page.getByRole('dialog', { name: 'Remove saved audio?' });
  await dialog.getByRole('checkbox').check();
  await dialog.getByRole('button', { name: 'Remove audio', exact: true }).click();
  await expect(dialog.getByRole('alert')).toContainText('Deletion preview is stale');
  await expect(dialog.getByRole('checkbox')).not.toBeChecked();
  await expect(dialog.getByRole('button', { name: 'Remove audio', exact: true })).toBeDisabled();
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  await page.getByRole('button', { name: 'Review audio removal' }).click();
  const previews = await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'preview_deletion'));
  expect(previews).toHaveLength(2);
});

test('cleanup preview reports protected versions and maintenance cannot be cancelled', async ({ page }) => {
  await page.evaluate(() => {
    const f = (window as any).__testFixture;
    f.cleanupPreview = { policy: structuredClone(f.policy), protected_runs: 4, skipped_busy: ['busy-fixture'], actions: [{ token: 'policy-fixture', meeting_id: 'saved-fixture', kind: 'runs', files: 0, bytes: 0, runs: 2, fingerprint: 'fixture', protected_runs: 4, protected_active: 1, protected_references: 3, foreign_key_tables: [] }] };
  });
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  await page.getByRole('button', { name: 'Preview eligible cleanup' }).click();
  const dialog = page.getByRole('dialog', { name: 'Apply storage cleanup?' });
  await expect(dialog.getByText('Transcript versions protected', { exact: true })).toBeVisible();
  await expect(dialog.getByText('1 busy meeting was skipped.')).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Apply cleanup', exact: true })).toBeDisabled();
  await dialog.getByRole('checkbox').check();
  await dialog.getByRole('button', { name: 'Apply cleanup', exact: true }).click();
  await expect(page.getByRole('status', { name: 'Storage maintenance result' })).toContainText('2 transcript versions removed');
  await page.evaluate(() => { (window as any).__testFixture.snapshot.processing = { meeting_id: '', kind: 'maintenance', stage: 'Removing selected data', progress: 0 }; });
  await expect(page.getByText('Finishing safely · cannot be cancelled', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Cancel processing' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'View meeting', exact: true })).toHaveCount(0);
});

test('unavailable audio blocks waveform and inference while notes show source provenance', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.evaluate(() => {
    const f = (window as any).__testFixture;
    f.detail.audio_available = false; f.detail.meeting.status = 'recovered';
    f.detail.notes = [{ id: 'legacy', t_ms: 0, text: 'Legacy timing fixture note.' }, { id: 'untimed', t_ms: 0, text: 'Untimed fixture note.', timing_provenance: 'untimed_note', timing_synchronized: false }, { id: 'approximate', t_ms: 1400, text: 'Approximate fixture note.', timing_provenance: 'latest_audio_observation', timing_synchronized: false }];
    f.snapshot.recording = { id: 'other-paused', title: 'Other paused meeting', status: 'paused', elapsed_ms: 1400, mic_level: 0, system_level: 0, gaps: 0 };
    f.snapshot.source_timing = { synchronized: false, provenance: 'latest_audio_observation', elapsed_ms: 1400, latest_audio_ms: 1400, warning: 'Latest observed source position may lag.' };
    f.snapshot.health = { free_bytes: 350000000, checked_at: '2026-11-07T12:00:00Z', disk_state: 'inference_suspended', inference_suspended: true, automatic_processing_allowed: false, warning: 'Live inference is suspended; raw audio recording continues.' };
  });
  await expect(page.getByRole('button', { name: 'Retry transcription', exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Refine transcript', exact: true })).toBeDisabled();
  await expect(page.getByRole('region', { name: 'Recording health' })).toContainText('Live inference is suspended');
  await expect(page.getByRole('region', { name: 'Recording health' })).toContainText('Latest observed source position may lag.');
  await page.getByRole('tab', { name: /^Notes/ }).click();
  const notes = page.locator('.notes-list');
  await expect(notes.getByText('Untimed note', { exact: true })).toBeVisible();
  await expect(notes.getByText('Timing unverified', { exact: true })).toBeVisible();
  await expect(notes.getByText('Approximate audio position', { exact: true })).toBeVisible();
  await page.getByRole('tab', { name: 'Audio', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Saved audio is unavailable' })).toBeVisible();
  expect(await page.evaluate(() => (window as any).__testCalls.some((c: any) => ['meeting_waveform', 'transcribe_meeting', 'refine_meeting', 'play_meeting_audio'].includes(c.command)))).toBe(false);
});

test('device discovery cannot delay library readiness when its promise never resolves', async ({ page }) => {
  await page.goto('/?hangDevices=1');
  await expect(page.getByRole('heading', { name: 'Your next conversation starts here' })).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.some((c: any) => c.command === 'frontend_ready'))).toBe(true);
  expect(await page.evaluate(() => (window as any).__testCalls.some((c: any) => c.command === 'input_devices'))).toBe(true);
});

test('committed deletion cannot be resubmitted when refreshing the view fails', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.evaluate(() => { (window as any).__testFixture.failRefreshAfterCommit = true; });
  await page.getByText('Stored meeting data', { exact: true }).click();
  await page.getByRole('button', { name: 'Review audio removal' }).click();
  let dialog = page.getByRole('dialog', { name: 'Remove saved audio?' });
  await dialog.getByRole('checkbox').check();
  await dialog.getByRole('button', { name: 'Remove audio', exact: true }).click();
  dialog = page.getByRole('dialog', { name: 'Storage operation finished' });
  await expect(dialog.getByRole('alert')).toContainText('view could not be refreshed');
  await expect(dialog.getByRole('button', { name: 'Remove audio', exact: true })).toHaveCount(0);
  await expect(dialog.getByRole('button', { name: 'Close', exact: true })).toBeEnabled();
  await dialog.getByRole('button', { name: 'Close', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Refine transcript', exact: true })).toBeDisabled();
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'commit_deletion').length)).toBe(1);
  await page.evaluate(() => { delete (window as any).__testFixture.snapshotError; });
  await page.getByRole('button', { name: 'Retry', exact: true }).first().click();
  await expect(page.getByRole('status', { name: 'Storage maintenance result' })).toContainText('1 audio recordings removed');
});

test('a returned restoration failure report does not clear the selected meeting', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.evaluate(() => { (window as any).__testFixture.restoredFailure = true; });
  await page.getByText('Stored meeting data', { exact: true }).click();
  await page.getByRole('button', { name: 'Review meeting deletion' }).click();
  const dialog = page.getByRole('dialog', { name: 'Delete this meeting?' });
  await dialog.getByRole('checkbox').check();
  await dialog.getByRole('button', { name: 'Delete meeting', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Design fixture', exact: true })).toBeVisible();
  await expect(page.getByText('The project status is green.', { exact: true })).toBeVisible();
  await expect(page.getByRole('status', { name: 'Storage maintenance result' })).toContainText('original files restored');
});

test('privacy changes stay disabled during any paused stopping or active capture', async ({ page }) => {
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  const privacy = page.getByRole('region', { name: 'Privacy and retention' });
  await expect(privacy.getByRole('button', { name: 'Save retention preferences' })).toBeEnabled();
  for (const status of ['recording', 'paused', 'processing', 'stopping']) {
    await page.evaluate((state) => { (window as any).__testFixture.snapshot.recording = { id: 'other-meeting', title: 'Other active meeting', status: state, elapsed_ms: 5000, mic_level: 0, system_level: 0, gaps: 0 }; }, status);
    await expect(privacy.getByRole('button', { name: 'Save retention preferences' })).toBeDisabled();
    await expect(privacy.getByRole('button', { name: 'Preview eligible cleanup' })).toBeDisabled();
  }
  expect(await page.evaluate(() => (window as any).__testCalls.some((c: any) => ['save_maintenance_policy', 'commit_deletion', 'apply_cleanup'].includes(c.command)))).toBe(false);
});


test('English model choice retains both installed packs and never downloads on selection', async ({ page }) => {
  await page.evaluate(() => { const f = (window as any).__testFixture; f.snapshot.models.installed = true; });
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  const choice = page.getByLabel('Live English model');
  await expect(choice).toHaveValue('english_small');
  await choice.selectOption('english_standard');
  await expect(choice).toHaveValue('english_standard');
  await expect(page.getByText(/483.5 MiB download/)).toBeVisible();
  await expect(page.getByRole('button', { name: 'Download transcription model', exact: true })).toBeVisible();
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'install_model').length)).toBe(0);
  await choice.selectOption('english_small');
  await expect(page.getByText('Ready for your next meeting', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Verify or repair model', exact: true }).click();
  await expect(choice).toBeDisabled();
  await page.getByRole('button', { name: 'Cancel download', exact: true }).click();
  await expect(choice).toBeEnabled();
  await choice.selectOption('english_standard');
  await page.getByRole('button', { name: 'Download transcription model', exact: true }).click();
  await expect(choice).toBeDisabled();
  await page.getByRole('button', { name: 'Cancel download', exact: true }).click();
  await expect(choice).toBeEnabled();
  await expect(page.getByText('Model setup was cancelled.', { exact: true })).toBeVisible();
});

test('rejected model selection retains the persisted choice and explains the failure', async ({ page }) => {
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  const choice = page.getByLabel('Live English model');
  await expect(choice).toHaveValue('english_small');
  await page.evaluate(() => { (window as any).__testFixture.selectionError = 'Fixture choice is locked during work.'; });
  await choice.selectOption('english_standard');
  await expect(page.getByText('Fixture choice is locked during work.', { exact: true })).toBeVisible();
  await expect(choice).toHaveValue('english_small');
  expect(await page.evaluate(() => (window as any).__testFixture.liveModel)).toBe('english_small');
});

test('active and paused recording keep model choice fixed', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.evaluate(() => { const f = (window as any).__testFixture; f.snapshot.recording = { ...f.detail.meeting, status: 'recording', elapsed_ms: 1000, mic_level: 0, system_level: 0 }; });
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  const choice = page.getByLabel('Live English model');
  await expect(choice).toBeDisabled();
  await page.evaluate(() => { (window as any).__testFixture.snapshot.recording.status = 'paused'; });
  await expect(choice).toBeDisabled();
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'select_live_model').length)).toBe(0);
});


test('live overload notice preserves visible final text and shows staged processing state', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.evaluate(() => {
    const f = (window as any).__testFixture;
    f.snapshot.recording = { ...f.detail.meeting, status: 'recording', elapsed_ms: 16000, mic_level: 0, system_level: 0, transcription_configured: true, transcription_active: true };
    f.snapshot.live_processing = { vad_active: true, diarization_active: true, backlog: { stage: 'partials_disabled', lag_ms: 12000, partials_enabled: false, speakers_enabled: true, stt_enabled: true, warning: 'Live partial updates are disabled while inference catches up.' } };
  });
  await expect(page.getByText('Live processing reduced', { exact: true })).toBeVisible();
  await expect(page.getByText(/Original audio continues to be recorded/)).toBeVisible();
  await expect(page.getByText('The project status is green.', { exact: true })).toBeVisible();
});

test('inference history stays with its transcript version and explains unavailable source audio', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.evaluate(() => {
    const f = (window as any).__testFixture;
    f.detail.runs[0].kind = 'live';
    f.detail.audio_available = false;
    f.detail.inference_health = { stages: [{ stage: 'stt_disabled', source_ms: 16000, lag_ms: 12000 }], gaps: [{ worker: 'stt', track: 'mic', generation: 0, start_ms: 12000, end_ms: 16000, reason: 'Inference queue overflow' }], warnings: ['Live transcription was suspended after sustained inference lag.'], truncated: false, max_lag_ms: 12000 };
  });
  await expect(page.getByRole('region', { name: 'Transcript processing history' })).toBeVisible();
  await expect(page.getByText(/Source audio is unavailable for reprocessing/)).toBeVisible();
  await expect(page.getByText('The project status is green.', { exact: true })).toBeVisible();
  await page.getByLabel('Transcript version').selectOption('live-run');
  await expect(page.getByRole('region', { name: 'Transcript processing history' })).toHaveCount(0);
  await expect(page.getByText('Original source words from the live pass.', { exact: true })).toBeVisible();
});


test('unpersistable inference warning stays visible after recording ends without blocking a new recording', async ({ page }) => {
  await page.evaluate(() => {
    const f = (window as any).__testFixture;
    f.snapshot.recording = { id: 'finished-audit-fixture', title: 'Finished fixture', status: 'ready', elapsed_ms: 1000, mic_level: 0, system_level: 0, error: 'Live inference history could not be saved.' };
    f.snapshot.live_processing = { vad_active: false, diarization_active: false, backlog: { stage: 'normal', lag_ms: 0, partials_enabled: false, speakers_enabled: false, stt_enabled: false, warning: 'Live inference history could not be saved; processing gaps may be incomplete.' } };
  });
  await expect(page.getByText('Live processing notice', { exact: true })).toBeVisible();
  await expect(page.getByText(/processing gaps may be incomplete/)).toBeVisible();
  await expect(page.getByRole('button', { name: /New recording/ })).toBeEnabled();
  await expect(page.getByText('Recording in progress', { exact: true })).toHaveCount(0);
});


test('invalid saved model choice requires confirmation without silently activating or downloading', async ({ page }) => {
  await page.evaluate(() => {
    const f = (window as any).__testFixture;
    f.modelSelectionWarning = 'The saved live-model choice is invalid or unsupported. Confirm a supported model in Settings before using live transcription.';
    f.installedLiveModels.english_small = true;
    f.snapshot.models.installed = false;
    f.snapshot.models.error = f.modelSelectionWarning;
  });
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  await expect(page.getByText('Choice required', { exact: true })).toBeVisible();
  await expect(page.getByText(/saved live-model choice is invalid or unsupported/)).toHaveCount(1);
  await expect(page.getByRole('button', { name: 'Download transcription model', exact: true })).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'install_model' || c.command === 'select_live_model').length)).toBe(0);
  await page.getByRole('button', { name: 'Confirm this model choice', exact: true }).click();
  await expect(page.getByText('Ready for your next meeting', { exact: true })).toBeVisible();
  await expect(page.getByText('Choice required', { exact: true })).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'install_model').length)).toBe(0);
});

async function showDetection(page: import('@playwright/test').Page, kind: 'meeting' | 'browser' = 'browser', episode = 'episode-1') {
  await page.evaluate(({ kind, episode }) => {
    const f = (window as any).__testFixture;
    const app = f.snapshot.meeting_detection.catalog.find((app: any) => app.kind === kind);
    f.snapshot.settings.meeting_detection.enabled = true;
    f.snapshot.meeting_detection.enabled = true;
    f.snapshot.meeting_detection.status = 'watching';
    f.snapshot.meeting_detection.prompts = [{ episode_id: episode, app, message: 'Synthetic microphone observation' }];
  }, { kind, episode });
}

test('meeting detection stays off until explicitly saved and browser rules explain any microphone use', async ({ page }) => {
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  const section = page.getByRole('region', { name: 'Meeting detection', exact: true });
  await expect(section.getByLabel('Enable meeting reminders')).not.toBeChecked();
  await expect(section.getByText(/not evidence of a meeting/)).toBeVisible();
  await expect(section.getByLabel('Chrome detection rule')).toHaveValue('prompt');
  await section.getByLabel('Enable meeting reminders').check();
  await section.getByLabel('Chrome detection rule').selectOption('always_record');
  const save = section.getByRole('button', { name: 'Save meeting detection preferences' });
  await expect(save).toBeDisabled();
  await section.getByText(/including any browser microphone use outside meetings/).isVisible();
  await section.getByLabel('I have informed participants about automatic recording for Chrome.').check();
  await expect(save).toBeDisabled();
  await section.getByLabel('Chrome automatic recording microphone').selectOption('built-in');
  await save.click();
  await expect(section.getByText('Meeting detection preferences saved.')).toBeVisible();
  const calls = await page.evaluate(() => (window as any).__testCalls);
  expect(calls.filter((c: any) => ['start_recording', 'start_detected_recording'].includes(c.command))).toHaveLength(0);
  expect(calls.find((c: any) => c.command === 'update_meeting_detection').args.settings).toMatchObject({ enabled: true, rules: [{ app_id: 'com.google.Chrome', policy: 'always_record', participant_notice_attested: true, input: { kind: 'device', id: 'built-in' } }] });
  expect(await page.evaluate(() => (window as any).__testFixture.setupOpen)).toBe(true);
  expect(await page.evaluate(() => (window as any).__testFixture.snapshot.meeting_detection.status)).toBe('blocked');
});

test('detected browser Prompt requires fresh setup consent and an explicit Bluetooth input decision', async ({ page }) => {
  await showDetection(page);
  const reminder = page.getByRole('status', { name: 'Chrome microphone reminder' });
  await expect(reminder).toContainText('Chrome is using the microphone');
  await expect(reminder).not.toContainText('Meeting detected');
  await reminder.getByRole('button', { name: 'Review recording setup' }).click();
  const dialog = page.getByRole('dialog');
  const start = dialog.getByRole('button', { name: 'Start recording', exact: true });
  await expect(start).toBeDisabled();
  expect(await page.evaluate(() => (window as any).__testFixture.setupOpen)).toBe(true);
  await dialog.getByRole('checkbox').check();
  await expect(start).toBeDisabled();
  await dialog.getByLabel('Microphone', { exact: true }).selectOption('built-in');
  await dialog.getByLabel('Meeting title', { exact: true }).fill('Detected fixture');
  await start.click();
  await expect(page.getByRole('heading', { name: 'Detected fixture', exact: true })).toBeVisible();
  const calls = await page.evaluate(() => (window as any).__testCalls);
  expect(calls.filter((c: any) => c.command === 'start_recording')).toHaveLength(0);
  expect(calls.find((c: any) => c.command === 'start_detected_recording').args).toEqual({ episode_id: 'episode-1', title: 'Detected fixture', consent: true, input_device: 'built-in' });
});

test('in-person choice is synchronized before setup closes and suppresses detected prompts', async ({ page }) => {
  await page.getByRole('button', { name: 'Record your first meeting' }).click();
  await page.getByRole('radio', { name: /In person/ }).check();
  await expect.poll(() => page.evaluate(() => (window as any).__testFixture.snapshot.settings.capture_mode)).toBe('in_person');
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await showDetection(page, 'meeting');
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'app_snapshot').length)).toBeGreaterThan(1);
  await expect(page.getByRole('status', { name: 'Zoom microphone reminder' })).toHaveCount(0);
  const contexts = await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'set_detection_context'));
  expect(contexts.some((c: any) => c.args.mode === 'in_person' && c.args.setup_open)).toBe(true);
  expect(contexts.at(-1).args).toEqual({ mode: 'in_person', setup_open: false, context_generation: 0 });
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => ['start_recording', 'start_detected_recording'].includes(c.command)).length)).toBe(0);
});

test('a stale detection episode cannot submit or fall back to manual capture', async ({ page }) => {
  await showDetection(page, 'meeting');
  await page.getByRole('button', { name: 'Review recording setup' }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('checkbox').check();
  await dialog.getByLabel('Microphone', { exact: true }).selectOption('built-in');
  await page.evaluate(() => { (window as any).__testFixture.snapshot.meeting_detection.prompts = []; });
  await expect(dialog.getByRole('alert')).toContainText('no longer current');
  await expect(dialog.getByRole('button', { name: 'Start recording', exact: true })).toBeDisabled();
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => ['start_recording', 'start_detected_recording'].includes(c.command)).length)).toBe(0);
});

test('backend freshness rejection preserves setup and never silently retries a manual start', async ({ page }) => {
  await showDetection(page, 'meeting');
  await page.getByRole('button', { name: 'Review recording setup' }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('checkbox').check();
  await dialog.getByLabel('Microphone', { exact: true }).selectOption('built-in');
  await page.evaluate(() => { (window as any).__testFixture.rejectDetectionStart = true; });
  await dialog.getByRole('button', { name: 'Start recording', exact: true }).click();
  await expect(dialog.getByRole('alert')).toContainText('no longer current');
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'start_recording').length)).toBe(0);
  await expect(dialog).toBeVisible();
});

test('dismiss and per-app suppress are separate choices without starting capture', async ({ page }) => {
  await showDetection(page);
  await page.getByRole('button', { name: 'Not now', exact: true }).click();
  await expect(page.getByRole('status', { name: 'Chrome microphone reminder' })).toHaveCount(0);
  await showDetection(page, 'browser', 'episode-2');
  await page.getByRole('button', { name: 'Suppress Chrome', exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__testFixture.snapshot.settings.meeting_detection.rules[0]?.policy)).toBe('suppress');
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => ['start_recording', 'start_detected_recording'].includes(c.command)).length)).toBe(0);
});

test('ended-source reminder never stops automatically and supports dismiss then explicit stop', async ({ page }) => {
  await showDetection(page, 'meeting');
  await page.getByRole('button', { name: 'Review recording setup' }).click();
  await page.getByRole('dialog').getByRole('checkbox').check();
  await page.getByLabel('Microphone', { exact: true }).selectOption('built-in');
  await page.getByRole('dialog').getByRole('button', { name: 'Start recording', exact: true }).click();
  await page.evaluate(() => { const f = (window as any).__testFixture; f.snapshot.meeting_detection.ended = { episode_id: 'ended-1', meeting_id: f.snapshot.recording.id, app_id: 'us.zoom.xos', message: 'Synthetic confirmed release >60s' }; });
  const reminder = page.getByRole('status', { name: 'Meeting may have ended', exact: true });
  await expect(reminder).toContainText('Recording continues until you stop it');
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'stop_recording').length)).toBe(0);
  await reminder.getByRole('button', { name: 'Keep recording' }).click();
  await expect(reminder).toHaveCount(0);
  await page.evaluate(() => { const f = (window as any).__testFixture; f.snapshot.meeting_detection.ended = { episode_id: 'ended-2', meeting_id: f.snapshot.recording.id, app_id: 'us.zoom.xos', message: 'Synthetic second release' }; });
  await reminder.getByRole('button', { name: 'Stop recording', exact: true }).click();
  await expect(reminder).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'stop_recording').length)).toBe(1);
});

test('failed detection settings save restores persisted settings and missing input cannot arm Always record', async ({ page }) => {
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  const section = page.getByRole('region', { name: 'Meeting detection', exact: true });
  await section.getByLabel('Enable meeting reminders').check();
  await page.evaluate(() => { (window as any).__testFixture.detectionSaveError = 'Synthetic settings write failure'; });
  await section.getByRole('button', { name: 'Save meeting detection preferences' }).click();
  await expect(section.getByRole('alert')).toContainText('Synthetic settings write failure');
  await expect(section.getByLabel('Enable meeting reminders')).not.toBeChecked();
  await page.evaluate(() => {
    const f = (window as any).__testFixture;
    delete f.detectionSaveError;
    f.snapshot.settings.meeting_detection.rules = [{ app_id: 'com.google.Chrome', policy: 'always_record', participant_notice_attested: true, input: { kind: 'device', id: 'disconnected-input' } }];
  });
  await expect(section.getByLabel('Chrome detection rule')).toHaveValue('always_record');
  await section.getByLabel('Enable meeting reminders').check();
  await expect(section.getByRole('button', { name: 'Save meeting detection preferences' })).toBeDisabled();
});

test('old snapshot responses cannot overwrite newly saved detection preferences', async ({ page }) => {
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  const section = page.getByRole('region', { name: 'Meeting detection', exact: true });
  await page.evaluate(() => { (window as any).__testFixture.holdNextSnapshot = true; });
  await expect.poll(() => page.evaluate(() => typeof (window as any).__testFixture.releaseHeldSnapshot)).toBe('function');
  await section.getByLabel('Enable meeting reminders').check();
  await section.getByRole('button', { name: 'Save meeting detection preferences' }).click();
  await expect(section.getByText('Meeting detection preferences saved.')).toBeVisible();
  await page.evaluate(async () => { (window as any).__testFixture.releaseHeldSnapshot(); await new Promise(resolve => setTimeout(resolve, 50)); });
  await expect(section.getByLabel('Enable meeting reminders')).toBeChecked();
  expect(await page.evaluate(() => (window as any).__testFixture.snapshot.settings.meeting_detection.enabled)).toBe(true);
});

test('recording or unknown observations suppress prompts and setup context errors fail closed', async ({ page }) => {
  await showDetection(page, 'meeting');
  await expect(page.getByRole('status', { name: 'Zoom microphone reminder' })).toBeVisible();
  await page.evaluate(() => { (window as any).__testFixture.snapshot.meeting_detection.status = 'unknown'; });
  await expect(page.getByRole('status', { name: 'Zoom microphone reminder' })).toHaveCount(0);
  await page.evaluate(() => { const f = (window as any).__testFixture; f.snapshot.meeting_detection.status = 'watching'; f.snapshot.recording = { id: 'other-paused', title: 'Existing paused fixture', status: 'paused', elapsed_ms: 1000, mic_level: 0, system_level: 0, gaps: 0 }; });
  await expect(page.getByRole('button', { name: 'Current recording', exact: false })).toBeVisible();
  await expect(page.getByRole('status', { name: 'Zoom microphone reminder' })).toHaveCount(0);
  await page.evaluate(() => { const f = (window as any).__testFixture; f.snapshot.recording = null; f.contextError = 'Synthetic setup synchronization failure'; });
  await page.getByRole('button', { name: 'Review recording setup' }).click();
  await expect(page.getByRole('alert')).toContainText('Synthetic setup synchronization failure');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => ['start_recording', 'start_detected_recording'].includes(c.command)).length)).toBe(0);
});

test('an existing recording arriving during detected setup disables capture rather than double-starting', async ({ page }) => {
  await showDetection(page, 'meeting');
  await page.getByRole('button', { name: 'Review recording setup' }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('checkbox').check();
  await dialog.getByLabel('Microphone', { exact: true }).selectOption('built-in');
  await page.evaluate(() => { const f = (window as any).__testFixture; f.snapshot.recording = { id: 'other-active', title: 'Already started fixture', status: 'recording', elapsed_ms: 1000, mic_level: 0, system_level: 0, gaps: 0 }; });
  await expect(dialog.getByRole('alert')).toContainText('no longer current');
  await expect(dialog.getByRole('button', { name: 'Start recording', exact: true })).toBeDisabled();
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => ['start_recording', 'start_detected_recording'].includes(c.command)).length)).toBe(0);
});

test('meeting detection public fake-IPC layouts fit desktop and minimum width', async ({ page }) => {
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  const section = page.getByRole('region', { name: 'Meeting detection', exact: true });
  await section.getByLabel('Enable meeting reminders').check();
  await section.getByLabel('Chrome detection rule').selectOption('always_record');
  await section.getByLabel('I have informed participants about automatic recording for Chrome.').check();
  await section.getByLabel('Chrome automatic recording microphone').selectOption('built-in');
  for (const width of [1280, 860]) {
    await page.setViewportSize({ width, height: 900 });
    await section.screenshot({ path: `/private/tmp/vantranscribe-meeting-detection-ui/settings-${width}.png` });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  }
  await page.getByRole('link', { name: 'VanTranscribe library' }).click();
  await showDetection(page);
  const prompt = page.getByRole('status', { name: 'Chrome microphone reminder' });
  for (const width of [1280, 860]) {
    await page.setViewportSize({ width, height: 900 });
    await prompt.screenshot({ path: `/private/tmp/vantranscribe-meeting-detection-ui/browser-prompt-${width}.png` });
  }
  await prompt.getByRole('button', { name: 'Review recording setup' }).click();
  await page.getByRole('dialog').getByRole('checkbox').check();
  await page.getByLabel('Microphone', { exact: true }).selectOption('built-in');
  await page.getByRole('dialog').getByRole('button', { name: 'Start recording', exact: true }).click();
  await page.evaluate(() => { const f = (window as any).__testFixture; f.snapshot.meeting_detection.ended = { episode_id: 'visual-ended', meeting_id: f.snapshot.recording.id, app_id: 'com.google.Chrome', message: 'Synthetic confirmed release >60s' }; });
  const ended = page.getByRole('status', { name: 'Meeting may have ended', exact: true });
  for (const width of [1280, 860]) {
    await page.setViewportSize({ width, height: 900 });
    await ended.screenshot({ path: `/private/tmp/vantranscribe-meeting-detection-ui/ended-reminder-${width}.png` });
  }
});

test('failed setup release stays blocked without a retry loop and exposes explicit recovery', async ({ page }) => {
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  await page.evaluate(() => { (window as any).__testFixture.contextError = 'Synthetic context release failure'; });
  const before = await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'set_detection_context').length);
  await page.getByRole('link', { name: 'VanTranscribe library' }).click();
  await expect(page.getByRole('alert')).toContainText('Synthetic context release failure');
  await expect(page.getByRole('button', { name: 'Retry detection setup' })).toBeVisible();
  const pollCount = await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'app_snapshot').length);
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'app_snapshot').length)).toBeGreaterThan(pollCount);
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'set_detection_context').length)).toBe(before + 1);
  expect(await page.evaluate(() => (window as any).__testFixture.setupOpen)).toBe(true);
  await page.evaluate(() => { delete (window as any).__testFixture.contextError; });
  await page.getByRole('button', { name: 'Retry detection setup' }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__testFixture.setupOpen)).toBe(false);
});

test('turning reminders off never waits for microphone discovery or a disconnected automatic input', async ({ page }) => {
  await page.goto('/?hangDevices=1');
  await page.evaluate(() => {
    const f = (window as any).__testFixture;
    f.snapshot.settings.meeting_detection = { enabled: true, rules: [{ app_id: 'com.google.Chrome', policy: 'always_record', participant_notice_attested: true, input: { kind: 'device', id: 'disconnected-input' } }] };
    f.snapshot.meeting_detection.enabled = true;
  });
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  const section = page.getByRole('region', { name: 'Meeting detection', exact: true });
  await expect(section.getByLabel('Enable meeting reminders')).toBeChecked();
  await section.getByLabel('Enable meeting reminders').uncheck();
  await section.getByRole('button', { name: 'Save meeting detection preferences' }).click();
  await expect(section.getByText('Meeting detection preferences saved.')).toBeVisible();
  expect(await page.evaluate(() => (window as any).__testFixture.snapshot.settings.meeting_detection.enabled)).toBe(false);
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => ['start_recording', 'start_detected_recording'].includes(c.command)).length)).toBe(0);
});

test('saving reminders off discards an unarmed automatic rule instead of authorizing it for later', async ({ page }) => {
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  const section = page.getByRole('region', { name: 'Meeting detection', exact: true });
  await section.getByLabel('Chrome detection rule').selectOption('always_record');
  await expect(section.getByLabel('Enable meeting reminders')).not.toBeChecked();
  await section.getByRole('button', { name: 'Save meeting detection preferences' }).click();
  await expect(section.getByText('Meeting detection preferences saved.')).toBeVisible();
  await expect(section.getByLabel('Chrome detection rule')).toHaveValue('prompt');
  expect(await page.evaluate(() => (window as any).__testFixture.snapshot.settings.meeting_detection.rules[0])).toMatchObject({ policy: 'prompt', participant_notice_attested: false, input: null });
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => ['start_recording', 'start_detected_recording'].includes(c.command)).length)).toBe(0);
});

test('automatic-input failure is visible beside its manual Prompt fallback and starts no hidden capture', async ({ page }) => {
  await showDetection(page, 'browser');
  await page.evaluate(() => { (window as any).__testFixture.snapshot.meeting_detection.error = 'Saved automatic recording microphone is unavailable. Choose an input before recording.'; });
  await expect(page.getByRole('status', { name: 'Meeting detection status', exact: true })).toContainText('microphone is unavailable');
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => ['start_recording', 'start_detected_recording'].includes(c.command)).length)).toBe(0);
  await page.getByRole('button', { name: 'Review recording setup' }).click();
  await page.getByRole('dialog').getByRole('checkbox').check();
  await page.getByLabel('Microphone', { exact: true }).selectOption('built-in');
  await page.getByRole('dialog').getByRole('button', { name: 'Start recording', exact: true }).click();
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'start_detected_recording').length)).toBe(1);
});

test('a queued old-page context cannot release new-page setup or borrow its newer snapshot generation', async ({ page }) => {
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  await page.evaluate(() => { (window as any).__testFixture.holdNextContext = true; });
  await page.getByRole('link', { name: 'VanTranscribe library' }).click();
  await expect.poll(() => page.evaluate(() => typeof (window as any).__testFixture.releaseHeldContext)).toBe('function');
  await page.evaluate(() => {
    const f = (window as any).__testFixture;
    f.snapshot.meeting_detection.context_generation = 1;
    f.setupOpen = true; // The replacement WebView's setup is now blocked.
    sessionStorage.setItem('fixture-detection-generation', '1');
  });
  const polls = await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'app_snapshot').length);
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'app_snapshot').length)).toBeGreaterThan(polls);
  await page.evaluate(() => { (window as any).__testFixture.releaseHeldContext(); });
  await expect(page.getByRole('alert')).toContainText('previous detection setup is no longer authorized');
  expect(await page.evaluate(() => (window as any).__testFixture.setupOpen)).toBe(true);
  const previous = await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'set_detection_context').length);
  await page.getByRole('button', { name: 'Retry detection setup' }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'set_detection_context').length)).toBe(previous + 1);
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'set_detection_context').at(-1).args.context_generation)).toBe(0);
  expect(await page.evaluate(() => (window as any).__testFixture.setupOpen)).toBe(true);
  await page.reload();
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.some((c: any) => c.command === 'frontend_ready'))).toBe(true);
  expect(await page.evaluate(() => (window as any).__testCalls.find((c: any) => c.command === 'set_detection_context').args.context_generation)).toBe(1);
});

test('a queued old-page Always record save cannot override newer Off and Suppress settings', async ({ page }) => {
  await page.getByRole('button', { name: 'Settings', exact: false }).click();
  const section = page.getByRole('region', { name: 'Meeting detection', exact: true });
  await section.getByLabel('Enable meeting reminders').check();
  await section.getByLabel('Chrome detection rule').selectOption('always_record');
  await section.getByLabel('I have informed participants about automatic recording for Chrome.').check();
  await section.getByLabel('Chrome automatic recording microphone').selectOption('built-in');
  await page.evaluate(() => { (window as any).__testFixture.holdNextDetectionSave = true; });
  await section.getByRole('button', { name: 'Save meeting detection preferences' }).click();
  await expect.poll(() => page.evaluate(() => typeof (window as any).__testFixture.releaseHeldDetectionSave)).toBe('function');
  await page.evaluate(() => {
    const f = (window as any).__testFixture;
    f.snapshot.meeting_detection.context_generation = 1;
    f.snapshot.settings.meeting_detection = { enabled: false, rules: [{ app_id: 'com.google.Chrome', policy: 'suppress', participant_notice_attested: false, input: null }] };
    f.snapshot.meeting_detection.enabled = false;
    f.snapshot.meeting_detection.status = 'disabled';
    f.setupOpen = true;
    sessionStorage.setItem('fixture-detection-generation', '1');
  });
  const polls = await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'app_snapshot').length);
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'app_snapshot').length)).toBeGreaterThan(polls);
  await page.evaluate(() => { (window as any).__testFixture.releaseHeldDetectionSave(); });
  await expect(section.getByRole('alert')).toContainText('previous detection settings are no longer authorized');
  expect(await page.evaluate(() => (window as any).__testCalls.find((c: any) => c.command === 'update_meeting_detection').args.context_generation)).toBe(0);
  expect(await page.evaluate(() => (window as any).__testFixture.snapshot.settings.meeting_detection)).toEqual({ enabled: false, rules: [{ app_id: 'com.google.Chrome', policy: 'suppress', participant_notice_attested: false, input: null }] });
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => ['start_recording', 'start_detected_recording'].includes(c.command)).length)).toBe(0);
});

async function seedRememberableVoice(page: import('@playwright/test').Page) {
  await seedSpeakerMeeting(page);
  await page.evaluate(() => {
    const f = (window as any).__testFixture; f.memory.enabled = true;
    f.detail.diarization.segmentation_context = { source_window_ms: 10000, ownership_stride_ms: 2500, native_window_ms: 10000 };
    f.detail.speakers[0].speech_ms = 24000;
    f.detail.speaker_turns = [{ ...f.detail.speaker_turns[0], start_ms: 0, end_ms: 24000 }];
  });
  await expect(page.getByRole('button', { name: 'Remember this voice', exact: true }).first()).toBeEnabled();
}

test('voice memory starts off and requires its own biometric explanation before enabling', async ({ page }) => {
  await page.getByRole('button', { name: /^Settings/ }).click();
  const people = page.getByRole('region', { name: 'People and voiceprints', exact: true });
  await expect(people.getByLabel('Enable voice memory', { exact: true })).not.toBeChecked();
  await expect(people).toContainText('Automatic recognition is unavailable');
  await people.getByLabel('Enable voice memory', { exact: true }).check();
  await expect(people.getByRole('button', { name: 'Save voice memory preferences', exact: true })).toBeDisabled();
  await people.getByRole('checkbox', { name: /I understand that voiceprints/ }).check();
  await people.getByRole('combobox', { name: 'Delete voiceprints after no confirmed enrollment or match for', exact: true }).selectOption('6');
  await people.getByRole('button', { name: 'Save voice memory preferences', exact: true }).click();
  await expect(people.getByText('Voice memory preferences saved.', { exact: true })).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.find((c: any) => c.command === 'update_voice_memory')?.args)).toMatchObject({ enabled: true, retention_months: 6, explanation_acknowledged: true });
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'enroll_voice').length)).toBe(0);
});

test('remember voice requires separate written consent and explicit remote source classification', async ({ page }) => {
  await seedRememberableVoice(page);
  await page.getByRole('button', { name: 'Remember this voice', exact: true }).first().click();
  const enrollment = page.getByRole('region', { name: 'Remember voice enrollment', exact: true });
  await enrollment.getByLabel('Voiceprint person name', { exact: true }).fill('Fictional participant');
  await enrollment.getByLabel('Consent attested by', { exact: true }).fill('Test operator');
  await enrollment.getByLabel('Written consent method', { exact: true }).fill('Fictional signed consent fixture');
  await expect(enrollment.getByRole('button', { name: 'Save encrypted voiceprint', exact: true })).toBeDisabled();
  await enrollment.getByRole('checkbox', { name: /This person has separately consented in writing/ }).check();
  await expect(enrollment.getByRole('button', { name: 'Save encrypted voiceprint', exact: true })).toBeDisabled();
  await enrollment.getByLabel('Voiceprint source audio type', { exact: true }).selectOption('remote_narrowband');
  for (const width of [1280, 860]) { await page.setViewportSize({ width, height: 900 }); await enrollment.screenshot({ path: `/private/tmp/vantranscribe-voice-memory-ui/enrollment-${width}.png` }); expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true); }
  await enrollment.getByRole('button', { name: 'Save encrypted voiceprint', exact: true }).click();
  await expect(page.getByText('Encrypted voiceprint saved with consent. Automatic recognition remains unavailable.', { exact: true })).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.find((c: any) => c.command === 'enroll_voice')?.args.request)).toMatchObject({ meeting_id: 'saved-fixture', run_id: 'refined-run', speaker_id: 'sp-a', person_id: null, display_name: 'Fictional participant', is_self: false, written_consent: true, channel_type: 'remote_narrowband', consent_method: 'Fictional signed consent fixture', attested_by: 'Test operator' });
  expect(await page.evaluate(() => JSON.stringify(localStorage))).not.toContain('Fictional participant');
});

test('an enrollment stays visibly in progress while its own backend reservation disables other speaker edits', async ({ page }) => {
  await seedRememberableVoice(page);
  await page.evaluate(() => { (window as any).__testFixture.holdNextEnrollment = true; });
  await page.getByRole('button', { name: 'Remember this voice', exact: true }).first().click();
  const enrollment = page.getByRole('region', { name: 'Remember voice enrollment', exact: true });
  await enrollment.getByLabel('Voiceprint person name', { exact: true }).fill('Fictional self');
  await enrollment.getByLabel('This is my own voice', { exact: true }).check();
  await enrollment.getByRole('checkbox', { name: /I consent to storing my own/ }).check();
  await enrollment.getByLabel('Consent attested by', { exact: true }).fill('Test operator');
  await enrollment.getByLabel('Voiceprint source audio type', { exact: true }).selectOption('remote_wideband');
  const polls = await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'app_snapshot').length);
  await enrollment.getByRole('button', { name: 'Save encrypted voiceprint', exact: true }).click();
  await expect.poll(() => page.evaluate(() => typeof (window as any).__testFixture.releaseHeldEnrollment)).toBe('function');
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'app_snapshot').length)).toBeGreaterThan(polls);
  await expect(enrollment.getByRole('button', { name: 'Saving voice…', exact: true })).toBeVisible();
  await expect(enrollment).not.toContainText('This speaker or transcript changed');
  await page.evaluate(() => { (window as any).__testFixture.releaseHeldEnrollment(); });
  await expect(page.getByText('Encrypted voiceprint saved with consent. Automatic recognition remains unavailable.', { exact: true })).toBeVisible();
});

test('voice enrollment excludes short shared-room historical and missing-audio speakers', async ({ page }) => {
  await seedSpeakerMeeting(page);
  const remember = page.getByRole('button', { name: 'Remember this voice', exact: true }).first();
  await expect(remember).toBeDisabled();
  await page.evaluate(() => { const f = (window as any).__testFixture; f.detail.diarization.segmentation_context = { source_window_ms: 10000, ownership_stride_ms: 2500, native_window_ms: 10000 }; f.detail.speakers[0].speech_ms = 24000; f.detail.speaker_turns[0].start_ms = 0; f.detail.speaker_turns[0].end_ms = 24000; });
  await expect(remember).toBeEnabled();
  await page.getByLabel('Shared room with multiple voices', { exact: true }).first().check();
  await expect(remember).toBeDisabled();
  await expect(page.getByText('Shared-room voices cannot be remembered.', { exact: true })).toBeVisible();
  await page.getByLabel('Shared room with multiple voices', { exact: true }).first().uncheck();
  await expect(remember).toBeEnabled();
  await page.getByRole('combobox', { name: 'Transcript version', exact: true }).selectOption('live-run');
  await expect(page.getByRole('button', { name: 'Remember this voice', exact: true }).first()).toBeDisabled();
  await page.getByRole('combobox', { name: 'Transcript version', exact: true }).selectOption('');
  await page.evaluate(() => { (window as any).__testFixture.detail.audio_available = false; });
  await expect(page.getByRole('button', { name: 'Remember this voice', exact: true }).first()).toBeDisabled();
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'enroll_voice').length)).toBe(0);
});

test('locked-key enrollment preserves the consent form and reports failure without storing a voice', async ({ page }) => {
  await seedRememberableVoice(page);
  await page.evaluate(() => { (window as any).__testFixture.enrollError = 'The voiceprint key is locked. Unlock Keychain and retry.'; });
  await page.getByRole('button', { name: 'Remember this voice', exact: true }).first().click();
  const enrollment = page.getByRole('region', { name: 'Remember voice enrollment', exact: true });
  await enrollment.getByLabel('Voiceprint person name', { exact: true }).fill('Fictional self');
  await enrollment.getByLabel('This is my own voice', { exact: true }).check();
  await enrollment.getByRole('checkbox', { name: /I consent to storing my own/ }).check();
  await enrollment.getByLabel('Consent attested by', { exact: true }).fill('Test operator');
  await enrollment.getByLabel('Voiceprint source audio type', { exact: true }).selectOption('remote_wideband');
  await enrollment.getByRole('button', { name: 'Save encrypted voiceprint', exact: true }).click();
  await expect(enrollment.getByRole('alert')).toContainText('key is locked');
  await expect(enrollment.getByLabel('Voiceprint person name', { exact: true })).toHaveValue('Fictional self');
  expect(await page.evaluate(() => (window as any).__testFixture.memory.people.length)).toBe(0);
});

test('People deletion confirms its scope and can remove past names without reading the key', async ({ page }) => {
  await page.evaluate(() => { const f = (window as any).__testFixture; f.memory = { enabled: false, retention_months: 12, matching_available: false, key_status: 'locked', people: [{ id: 'person-delete', display_name: 'Fictional person', is_self: false, consent: { attested_at: '2026-11-07T12:00:00Z', method: 'Fixture written consent', attested_by: 'Test operator' }, voiceprints: [{ id: 'voice-delete', channel_type: 'remote_wideband', embedding_version: 'fixture', n_exemplars: 1, last_matched_at: '2026-11-07T12:00:00Z', expires_at: '2027-11-07T12:00:00Z', stale: false }] }] }; });
  await page.getByRole('button', { name: /^Settings/ }).click();
  const people = page.getByRole('region', { name: 'People and voiceprints', exact: true });
  for (const width of [1280, 860]) { await page.setViewportSize({ width, height: 900 }); await people.screenshot({ path: `/private/tmp/vantranscribe-voice-memory-ui/people-${width}.png` }); expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true); }
  await people.getByRole('button', { name: 'Forget this person', exact: true }).click();
  await expect(people.getByRole('region', { name: 'Confirm voiceprint deletion', exact: true })).toContainText('1 saved voiceprint');
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'forget_person').length)).toBe(0);
  await people.getByRole('checkbox', { name: 'Remove linked speaker-name labels from past meetings', exact: true }).check();
  await people.getByRole('button', { name: 'Confirm deletion', exact: true }).click();
  await expect(people.getByText('No voices are saved.', { exact: false })).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.find((c: any) => c.command === 'forget_person')?.args)).toEqual({ person_id: 'person-delete', remove_names: true });
});

test('old People status cannot repaint a newly opened settings page and opt-out does not need key access', async ({ page }) => {
  await page.evaluate(() => { const f = (window as any).__testFixture; f.memory.enabled = true; f.holdNextMemoryStatus = true; });
  await page.getByRole('button', { name: /^Settings/ }).click();
  await expect.poll(() => page.evaluate(() => Boolean((window as any).__testFixture.releaseHeldMemoryStatus))).toBe(true);
  await page.getByRole('button', { name: /^Meeting library/ }).click();
  await page.evaluate(() => { const f = (window as any).__testFixture; f.memory.enabled = false; f.memory.key_status = 'locked'; });
  await page.getByRole('button', { name: /^Settings/ }).click();
  const people = page.getByRole('region', { name: 'People and voiceprints', exact: true });
  await expect(people.getByLabel('Enable voice memory', { exact: true })).not.toBeChecked();
  await page.evaluate(() => (window as any).__testFixture.releaseHeldMemoryStatus());
  await expect(people.getByLabel('Enable voice memory', { exact: true })).not.toBeChecked();
  await people.getByRole('button', { name: 'Save voice memory preferences', exact: true }).click();
  await expect(people.getByText('Voice memory preferences saved.', { exact: true })).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'update_voice_memory').at(-1)?.args.enabled)).toBe(false);
});

test('a queued old-page voice-memory enable cannot overwrite newer opt-out', async ({ page }) => {
  await page.getByRole('button', { name: /^Settings/ }).click();
  const people = page.getByRole('region', { name: 'People and voiceprints', exact: true });
  await people.getByLabel('Enable voice memory', { exact: true }).check();
  await people.getByRole('checkbox', { name: /I understand that voiceprints/ }).check();
  await page.evaluate(() => { (window as any).__testFixture.holdNextMemorySave = true; });
  await people.getByRole('button', { name: 'Save voice memory preferences', exact: true }).click();
  await expect.poll(() => page.evaluate(() => typeof (window as any).__testFixture.releaseHeldMemorySave)).toBe('function');
  await page.evaluate(() => {
    const f = (window as any).__testFixture; f.snapshot.meeting_detection.context_generation = 1; f.memory.enabled = false;
    sessionStorage.setItem('fixture-detection-generation', '1');
  });
  const polls = await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'app_snapshot').length);
  await expect.poll(() => page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'app_snapshot').length)).toBeGreaterThan(polls);
  await page.evaluate(() => (window as any).__testFixture.releaseHeldMemorySave());
  await expect(people.getByRole('alert')).toContainText('previous settings are no longer authorized');
  expect(await page.evaluate(() => (window as any).__testCalls.find((c: any) => c.command === 'update_voice_memory').args.context_generation)).toBe(0);
  expect(await page.evaluate(() => (window as any).__testFixture.memory.enabled)).toBe(false);
});

test('People status and deletion failures stay visible and preserve saved entries', async ({ page }) => {
  await page.evaluate(() => { (window as any).__testFixture.memoryStatusError = 'People metadata is temporarily unavailable.'; });
  await page.getByRole('button', { name: /^Settings/ }).click();
  const people = page.getByRole('region', { name: 'People and voiceprints', exact: true });
  await expect(people.getByRole('alert')).toContainText('metadata is temporarily unavailable');
  await expect(people.getByLabel('Enable voice memory', { exact: true })).toHaveCount(0);
  await page.evaluate(() => { const f = (window as any).__testFixture; f.memoryStatusError = null; f.forgetError = 'Deletion did not commit; saved voices are unchanged.'; f.memory.people = [{ id: 'person-retry', display_name: 'Fictional retry', is_self: true, consent: { attested_at: '2026-11-07T12:00:00Z', method: 'Own consent in app', attested_by: 'Test operator' }, voiceprints: [] }]; });
  await people.getByRole('button', { name: 'Retry People', exact: true }).click();
  await people.getByRole('button', { name: 'Delete all voiceprints', exact: true }).click();
  await people.getByRole('button', { name: 'Cancel deletion', exact: true }).click();
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'delete_all_voiceprints').length)).toBe(0);
  await people.getByRole('button', { name: 'Delete all voiceprints', exact: true }).click();
  await people.getByRole('button', { name: 'Confirm deletion', exact: true }).click();
  await expect(people.getByRole('alert')).toContainText('saved voices are unchanged');
  await expect(people.getByText('Fictional retry · Me', { exact: true })).toBeVisible();
  await page.evaluate(() => { (window as any).__testFixture.forgetError = null; });
  await people.getByRole('button', { name: 'Confirm deletion', exact: true }).click();
  await expect(people.getByText('No voices are saved.', { exact: false })).toBeVisible();
});

test('remember voice requires actual refined speaker context even after copied live labels are renamed', async ({ page }) => {
  await seedRememberableVoice(page);
  const remember = page.getByRole('button', { name: 'Remember this voice', exact: true }).first();
  await page.evaluate(() => {
    const f = (window as any).__testFixture;
    f.detail.speakers[0].user_name = 'Fictional renamed speaker';
    f.detail.speaker_turns[0].provenance = 'user';
    f.detail.diarization.segmentation_context = { source_window_ms: 5000, ownership_stride_ms: 2500, native_window_ms: 10000 };
  });
  await expect(remember).toBeDisabled();
  await expect(page.getByText('Refine with speaker models enabled before remembering this voice.', { exact: true }).first()).toBeVisible();
  await page.evaluate(() => { (window as any).__testFixture.detail.diarization.segmentation_context.source_window_ms = 10000; });
  await expect(remember).toBeEnabled();
  await page.evaluate(() => { delete (window as any).__testFixture.detail.diarization.segmentation_context; });
  await expect(remember).toBeDisabled();
  await expect(page.getByText('Refine with speaker models enabled before remembering this voice.', { exact: true }).first()).toBeVisible();
  expect(await page.evaluate(() => (window as any).__testCalls.filter((c: any) => c.command === 'enroll_voice').length)).toBe(0);
});


async function renameCalls(page: import('@playwright/test').Page) {
  return page.evaluate(() => (window as any).__testCalls.filter((call: any) => call.command === 'rename_speaker').map((call: any) => call.args));
}
const transcriptLabel = (page: import('@playwright/test').Page, segment: string) => page.locator(`#segment-${segment} [data-speaker-label]`);

async function openSpeakerTranscript(page: import('@playwright/test').Page) {
  await seedSpeakerMeeting(page);
  await page.getByRole('tab', { name: /^Transcript/ }).click();
  await expect(transcriptLabel(page, 'current-segment')).toHaveText('Speaker 1');
}

test('clicking a transcript speaker label edits it in place and Enter renames that speaker everywhere', async ({ page }) => {
  await openSpeakerTranscript(page);
  await page.getByRole('button', { name: 'Speaker 1', exact: true }).first().click();
  const field = page.getByRole('textbox', { name: 'Rename Speaker 1', exact: true });
  await expect(field).toBeFocused();
  await expect(field).toHaveValue('Speaker 1');
  expect(await field.evaluate((input: HTMLInputElement) => input.selectionStart === 0 && input.selectionEnd === input.value.length)).toBe(true);
  await expect(page.getByRole('textbox', { name: /^Rename/ })).toHaveCount(1);
  await field.fill('  Alice Morgan  ');
  await field.press('Enter');
  await expect.poll(() => renameCalls(page)).toEqual([{ id: 'saved-fixture', speaker_id: 'sp-a', name: 'Alice Morgan' }]);
  await expect(transcriptLabel(page, 'current-segment')).toHaveText('Alice Morgan');
  await expect(transcriptLabel(page, 'repeat-segment')).toHaveText('Alice Morgan');
  await expect(transcriptLabel(page, 'other-segment')).toHaveText('Speaker 2');
  await expect(page.getByRole('status').filter({ hasText: 'Speaker 1 renamed to Alice Morgan' })).toBeVisible();
  await page.getByRole('tab', { name: 'Speakers', exact: true }).click();
  await expect(page.getByRole('textbox', { name: 'Name for Alice Morgan', exact: true })).toHaveValue('Alice Morgan');
});

test('Escape cancels a transcript speaker rename without calling the backend', async ({ page }) => {
  await openSpeakerTranscript(page);
  await transcriptLabel(page, 'current-segment').click();
  await page.getByRole('textbox', { name: 'Rename Speaker 1', exact: true }).fill('Discarded');
  await page.keyboard.press('Escape');
  await expect(page.getByRole('textbox', { name: /^Rename/ })).toHaveCount(0);
  await expect(transcriptLabel(page, 'current-segment')).toHaveText('Speaker 1');
  await expect(transcriptLabel(page, 'current-segment')).toBeFocused();
  expect(await renameCalls(page)).toEqual([]);
});

test('leaving a transcript speaker field saves it while empty and unchanged names do nothing', async ({ page }) => {
  await openSpeakerTranscript(page);
  await transcriptLabel(page, 'current-segment').click();
  await page.getByRole('textbox', { name: 'Rename Speaker 1', exact: true }).fill('   ');
  await page.locator('.transcript-subbar').click();
  await expect(page.getByRole('textbox', { name: /^Rename/ })).toHaveCount(0);
  await transcriptLabel(page, 'current-segment').click();
  await page.getByRole('textbox', { name: 'Rename Speaker 1', exact: true }).fill(' Speaker 1 ');
  await page.getByRole('textbox', { name: 'Rename Speaker 1', exact: true }).press('Enter');
  await expect(page.getByRole('textbox', { name: /^Rename/ })).toHaveCount(0);
  expect(await renameCalls(page)).toEqual([]);
  await transcriptLabel(page, 'other-segment').click();
  await page.getByRole('textbox', { name: 'Rename Speaker 2', exact: true }).fill('Bob');
  await page.locator('.transcript-subbar').click();
  await expect.poll(() => renameCalls(page)).toEqual([{ id: 'saved-fixture', speaker_id: 'sp-b', name: 'Bob' }]);
  await expect(transcriptLabel(page, 'other-segment')).toHaveText('Bob');
});

test('channel labels, overlap lines and historical transcripts are not rename buttons', async ({ page }) => {
  await openSpeakerTranscript(page);
  await page.evaluate(() => {
    const fixture = (window as any).__testFixture;
    const line = (id: string, start: number, speaker: string) => ({ id, track: 'mic', start_ms: start, end_ms: start + 1000, speaker, text: `Line ${id}`, words: [] });
    fixture.detail.segments.push(line('channel-segment', 9000, 'Me'), line('overlap-segment', 11000, 'Overlap'));
    fixture.detail.speaker_assignments.push({ segment_id: 'overlap-segment', speaker_ids: ['sp-a', 'sp-b'], provenance: 'audio-derived', overlap: true, uncertain: false, words: [] });
  });
  const channel = page.locator('#segment-channel-segment .speaker-label-static');
  await expect(channel).toHaveText('Me');
  await expect(channel).toHaveAttribute('title', /channel label/);
  await expect(page.locator('#segment-overlap-segment .speaker-label-static')).toHaveAttribute('title', /Overlapping/);
  await expect(page.locator('#segment-channel-segment').getByRole('button', { name: 'Me', exact: true })).toHaveCount(0);
  await expect(page.locator('#segment-overlap-segment').getByRole('button', { name: 'Overlap', exact: true })).toHaveCount(0);
  await page.getByLabel('Transcript version').selectOption('live-run');
  await expect(page.getByText('Historical transcript · read only')).toBeVisible();
  await expect(page.locator('[data-speaker-label]')).toHaveCount(0);
  expect(await renameCalls(page)).toEqual([]);
});

test('a refused transcript speaker rename shows the error and keeps the old label', async ({ page }) => {
  await openSpeakerTranscript(page);
  await page.evaluate(() => { (window as any).__testFixture.renameError = 'Wait for processing to finish before changing speakers.'; });
  await transcriptLabel(page, 'current-segment').click();
  const field = page.getByRole('textbox', { name: 'Rename Speaker 1', exact: true });
  await field.fill('Alice');
  await field.press('Enter');
  await expect(page.getByRole('alert').filter({ hasText: 'Wait for processing to finish before changing speakers.' })).toBeVisible();
  await expect(transcriptLabel(page, 'current-segment')).toHaveText('Speaker 1');
  await expect(transcriptLabel(page, 'repeat-segment')).toHaveText('Speaker 1');
  expect(await renameCalls(page)).toHaveLength(1);
});

test('transcript speaker rename works from the keyboard and returns focus to the label', async ({ page }) => {
  await openSpeakerTranscript(page);
  await transcriptLabel(page, 'other-segment').focus();
  await page.keyboard.press('Enter');
  const field = page.getByRole('textbox', { name: 'Rename Speaker 2', exact: true });
  await expect(field).toBeFocused();
  await page.keyboard.type('Carol');
  await page.keyboard.press('Enter');
  await expect(transcriptLabel(page, 'other-segment')).toHaveText('Carol');
  await expect(transcriptLabel(page, 'other-segment')).toBeFocused();
  await page.keyboard.press('Space');
  await expect(page.getByRole('textbox', { name: 'Rename Carol', exact: true })).toBeFocused();
  expect(await renameCalls(page)).toEqual([{ id: 'saved-fixture', speaker_id: 'sp-b', name: 'Carol' }]);
});

test('long speaker names stay inside the transcript layout', async ({ page }) => {
  await openSpeakerTranscript(page);
  await transcriptLabel(page, 'current-segment').click();
  const field = page.getByRole('textbox', { name: 'Rename Speaker 1', exact: true });
  await field.fill('A'.repeat(110));
  await field.press('Enter');
  await expect(transcriptLabel(page, 'current-segment')).toHaveText('A'.repeat(110));
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});

test('saving-audio state follows the recorder after stop and the meeting is not announced as saved until it clears', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.evaluate(() => {
    const fixture = (window as any).__testFixture;
    fixture.savingOnStop = true;
    fixture.detail.meeting.status = 'recording';
    fixture.snapshot.recording = { id: 'saved-fixture', title: 'Design fixture', status: 'recording', elapsed_ms: 5000, mic_level: 0.4, system_level: 0.1, gaps: 0, transcription_configured: true, transcription_active: true };
  });
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  const saving = page.getByRole('status', { name: 'Saving audio', exact: true });
  await expect(saving).toContainText('00:12 of audio remaining');
  await expect(page.getByText('Recording saved to your library.')).toHaveCount(0);
  await page.evaluate(() => { const fixture = (window as any).__testFixture; fixture.snapshot.recording = null; fixture.snapshot.recorder = { queued_ms: 0, max_queued_ms: 20000, dropped_ms: 0, saving_audio: false }; });
  await expect(saving).toHaveCount(0);
  await expect(page.getByText('Recording saved to your library.')).toBeVisible();
});

test('recorder drops and backlog extend existing recording messages and absent recorder data is tolerated', async ({ page }) => {
  await seedSavedMeeting(page);
  await page.evaluate(() => {
    const fixture = (window as any).__testFixture;
    fixture.detail.meeting.status = 'recording';
    fixture.snapshot.recording = { id: 'saved-fixture', title: 'Design fixture', status: 'recording', elapsed_ms: 5000, mic_level: 0.4, system_level: 0.1, gaps: 0, transcription_configured: true, transcription_active: true };
  });
  await expect(page.getByText('Live VAD:')).toBeVisible();
  await expect(page.getByText('catching up')).toHaveCount(0);
  await expect(page.getByText('could not be saved')).toHaveCount(0);
  await page.evaluate(() => { (window as any).__testFixture.snapshot.recorder = { queued_ms: 6000, max_queued_ms: 20000, dropped_ms: 2000, saving_audio: false }; });
  await expect(page.getByText('Audio saving is catching up (00:06 queued)')).toBeVisible();
  await expect(page.getByRole('status').filter({ hasText: 'About 00:02 of audio could not be saved in this recording.' })).toBeVisible();
  await expect(page.getByRole('status', { name: 'Saving audio' })).toHaveCount(0);
  await page.evaluate(() => { const fixture = (window as any).__testFixture; fixture.snapshot.recorder = null; fixture.snapshot.recording.gaps = 2; });
  await expect(page.getByText('2 audio gaps have been marked in this recording.', { exact: true })).toBeVisible();
  await expect(page.getByText('could not be saved')).toHaveCount(0);
});
