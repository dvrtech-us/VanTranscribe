<script lang="ts">
  import { onMount, tick } from 'svelte';
  import Icon from './lib/Icon.svelte';
  import ProcessingSettings from './lib/ProcessingSettings.svelte';
  import SummaryPanel from './lib/SummaryPanel.svelte';
  import AudioPanel from './lib/AudioPanel.svelte';
  import AudioSettings from './lib/AudioSettings.svelte';
  import LiveModelSettings from './lib/LiveModelSettings.svelte';
  import MeetingDetectionSettings from './lib/MeetingDetectionSettings.svelte';
  import VoiceMemorySettings from './lib/VoiceMemorySettings.svelte';
  import DetectionNotice from './lib/DetectionNotice.svelte';
  import InferenceHealth from './lib/InferenceHealth.svelte';
  import SpeakersPanel from './lib/SpeakersPanel.svelte';
  import TranscriptWords from './lib/TranscriptWords.svelte';
  import BleedPanel from './lib/BleedPanel.svelte';
  import PrivacyPanel from './lib/PrivacyPanel.svelte';
  import ResourceStatus from './lib/ResourceStatus.svelte';
  import { echoRouteMessage, command, dateLabel, message, time, noteTimingLabel } from './lib/api';
  import type { CaptureMode, DetectionPrompt, ExportFormat, InputDevices, Meeting, MeetingDetail, Snapshot, MaintenanceReport } from './lib/api';

  let snapshot = $state<Snapshot | null>(null);
  let loading = $state(true);
  let connectionError = $state('');
  let actionError = $state('');
  let notice = $state('');
  let busyCommand = $state<string | null>(null);
  let view = $state<'library' | 'meeting' | 'settings'>('library');
  let selectedId = $state<string | null>(null);
  let detail = $state<MeetingDetail | null>(null);
  let detailError = $state('');
  let detailLoading = $state(false);
  let storageRefreshRequired = $state(false);
  let detailTab = $state<'transcript' | 'notes' | 'summary' | 'audio' | 'speakers'>('transcript');
  let search = $state('');
  let searchResults = $state<Meeting[]>([]);
  let searchLoading = $state(false);
  let searchError = $state('');
  let searchRequest = 0;
  let selectedRun = $state('');
  let editedSegmentId = $state('');
  let editedSegmentText = $state('');
  let reviewTargets = $state<Record<string, string>>({});
  let highlightedSegment = $state('');
  let transcriptSearch = $state('');
  let showHiddenBleed = $state(false);
  let newTitle = $state('');
  let mode = $state<CaptureMode>('remote');
  let showConsent = $state(false);
  let consent = $state(false);
  let detectionIntent = $state<DetectionPrompt | null>(null);
  let detectionContextReady = $state(false);
  let openingSetup = $state(false);
  let detectionContextOpen = false;
  let contextCloseFailed = $state(false);
  // This page's authority is fixed by its first accepted snapshot. A late poll
  // from a replaced page must never borrow the new WebView's generation.
  let pageContextGeneration = $state<number | undefined>(undefined);
  let noteText = $state('');
  let exportFormat = $state<ExportFormat>('markdown');
  let editingTitle = $state(false);
  let editedTitle = $state('');
  let followTranscript = $state(true);
  let inputDevices = $state<InputDevices | null>(null);
  let selectedInput = $state('');
  let deviceError = $state('');
  let theme = $state<'light' | 'dark' | 'system'>('system');
  let searchInput = $state<HTMLInputElement>();
  let titleInput = $state<HTMLInputElement>();
  let transcriptPanel = $state<HTMLDivElement>();
  let consentPanel = $state<HTMLDivElement>();
  let detailRequest = 0;
  let snapshotRequest = 0;
  let appliedSnapshot = 0;
  let refreshingDetail = false;
  let noticeTimer: ReturnType<typeof setTimeout>;

  const sessionBusy = $derived(Boolean(snapshot?.processing) || ['processing', 'stopping'].includes(snapshot?.recording?.status ?? '') || snapshot?.meetings.some((meeting) => meeting.status === 'processing'));
  const playbackBlocked = $derived(Boolean(snapshot?.recording && ['recording', 'paused', 'processing', 'stopping'].includes(snapshot.recording.status)));
  const audioUnavailable = $derived(detail?.audio_available === false || storageRefreshRequired);
  const maintenanceBlocked = $derived(sessionBusy || playbackBlocked || storageRefreshRequired || Boolean(busyCommand));
  const active = $derived(snapshot?.recording && ['recording', 'paused'].includes(snapshot.recording.status) ? snapshot.recording : null);
  const failedRecording = $derived(snapshot?.recording?.status === 'failed' ? snapshot.recording : null);
  const defaultBluetooth = $derived(Boolean(inputDevices?.devices.find((device) => device.is_default)?.bluetooth));
  const inputNeedsChoice = $derived(defaultBluetooth && !selectedInput);
  const detectionStale = $derived(Boolean(detectionIntent && (!snapshot?.meeting_detection?.enabled || ['unknown', 'disabled'].includes(snapshot.meeting_detection.status) || !snapshot.meeting_detection.prompts.some(prompt => prompt.episode_id === detectionIntent?.episode_id && prompt.app.id === detectionIntent?.app.id) || snapshot?.settings.capture_mode !== 'remote' || snapshot?.recording || snapshot?.processing)));
  $effect(() => {
    if (detectionContextReady && snapshot?.meeting_detection && !busyCommand && !showConsent && view !== 'settings' && !openingSetup && detectionContextOpen && !contextCloseFailed) {
      contextCloseFailed = true;
      void syncDetectionContext(mode, false).then(ok => { if (ok) contextCloseFailed = false; });
    }
  });
  const isSelectedActive = $derived(Boolean(active && active.id === selectedId));
  const meetings = $derived(search.trim() ? searchResults : snapshot?.meetings ?? []);
  const isCurrentRun = $derived(Boolean(detail?.run_id && detail.run_id === detail.active_run_id));
  const canEditSpeakers = $derived(Boolean(detail && isCurrentRun && !detailLoading && !snapshot?.processing && snapshot?.recording?.status !== 'processing'));
  const canEditTranscript = $derived(Boolean(detail && isCurrentRun && !isSelectedActive && !detailLoading && !snapshot?.processing));
  const activeRun = $derived(detail?.runs?.find((run) => run.id === detail?.active_run_id));
  const hiddenBleedIds = $derived(new Set((detail?.bleed_decisions ?? []).filter((d) => d.status === 'applied' && d.decision === 'bleed').map((d) => d.segment_id)));
  const summaryConfigured = $derived(Boolean(snapshot?.settings.summary_config && snapshot.settings.summary_config.provider !== 'none'));
  const canGenerateSummary = $derived(Boolean(detail && isCurrentRun && !isSelectedActive && activeRun?.status === 'complete' && detail.segments.some((s) => !hiddenBleedIds.has(s.id)) && !snapshot?.processing));
  const reviewCorrections = $derived((detail?.corrections ?? []).filter((correction) => correction.status === 'needs_review'));
  $effect(() => {
    const query = search.trim();
    const libraryRevision = snapshot?.meetings;
    const request = ++searchRequest;
    searchError = '';
    if (!query || !libraryRevision) { searchResults = []; searchLoading = false; return; }
    searchLoading = true;
    const timer = setTimeout(() => {
      void command<Meeting[]>('search_meetings', { query }).then((results) => {
        if (request === searchRequest) searchResults = results;
      }).catch((error) => {
        if (request === searchRequest) { searchError = message(error); searchResults = []; }
      }).finally(() => { if (request === searchRequest) searchLoading = false; });
    }, 250);
    return () => clearTimeout(timer);
  });
  const visibleSegments = $derived((detail?.segments ?? []).filter((segment) => (showHiddenBleed || !hiddenBleedIds.has(segment.id)) && (!transcriptSearch || segment.text.toLocaleLowerCase().includes(transcriptSearch.toLocaleLowerCase()) || segment.speaker.toLocaleLowerCase().includes(transcriptSearch.toLocaleLowerCase()))));
  const modelProgress = $derived(Math.round(Math.max(0, Math.min(1, snapshot?.models.progress ?? 0)) * 100));
  const totalTime = $derived((snapshot?.meetings ?? []).reduce((total, meeting) => total + meeting.duration_ms, 0));
  const currentTitle = $derived(isSelectedActive ? active?.title ?? 'Recording' : detail?.meeting.title ?? 'Meeting');

  function applyTheme() {
    const resolved = theme === 'system' ? (window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light') : theme;
    document.documentElement.dataset.theme = resolved;
    document.documentElement.dataset.bsTheme = resolved;
  }

  function chooseTheme(next: 'light' | 'dark' | 'system') {
    theme = next;
    try { localStorage.setItem('vantranscribe-theme', next); } catch { /* Appearance can remain session-local. */ }
    applyTheme();
  }

  function rememberInput() {
    try { localStorage.setItem('vantranscribe-input', selectedInput); } catch { /* Input choice can remain session-local. */ }
  }

  async function loadDevices() {
    try {
      inputDevices = await command<InputDevices>('input_devices');
      if (selectedInput && !inputDevices.devices.some((device) => device.id === selectedInput)) selectedInput = '';
      deviceError = '';
    } catch (error) { deviceError = message(error); }
  }

  function announce(text: string) {
    notice = text;
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => { notice = ''; }, 8000);
  }

  function applySnapshot(next: Snapshot, sequence: number) {
    if (sequence >= appliedSnapshot) {
      if (pageContextGeneration === undefined && next.meeting_detection) pageContextGeneration = next.meeting_detection.context_generation;
      snapshot = next;
      appliedSnapshot = sequence;
      connectionError = '';
    }
  }

  async function refresh() {
    const sequence = ++snapshotRequest;
    try {
      const next = await command<Snapshot>('app_snapshot');
      applySnapshot(next, sequence);
      if (view === 'meeting' && selectedId) await loadDetail(selectedId, false);
    } catch (error) {
      connectionError = message(error);
    } finally {
      loading = false;
    }
  }

  async function loadDetail(id: string, initial = true) {
    if (!initial && refreshingDetail) return;
    const request = ++detailRequest;
    refreshingDetail = true;
    if (initial) detailLoading = true;
    try {
      const run = selectedRun;
      const next = await command<MeetingDetail>('meeting_detail', { id, ...(run ? { run_id: run } : {}) });
      if (selectedId === id && selectedRun === run && request === detailRequest) {
        detail = next;
        storageRefreshRequired = false;
        detailError = '';
        if (followTranscript && isSelectedActive && detailTab === 'transcript') {
          await tick();
          transcriptPanel?.scrollTo({ top: transcriptPanel.scrollHeight, behavior: 'instant' });
        }
        return true;
      }
      return false;
    } catch (error) {
      if (selectedId === id) detailError = message(error);
      return false;
    } finally {
      if (request === detailRequest) detailLoading = false;
      refreshingDetail = false;
    }
  }

  function openMeeting(id: string) {
    selectedId = id;
    view = 'meeting';
    detail = null;
    selectedRun = '';
    showHiddenBleed = false;
    editedSegmentId = '';
    highlightedSegment = '';
    reviewTargets = {};
    detailTab = 'transcript';
    transcriptSearch = '';
    detailError = '';
    followTranscript = true;
    void loadDetail(id);
  }

  async function syncDetectionContext(nextMode: CaptureMode, setupOpen: boolean, contextGeneration = pageContextGeneration): Promise<boolean> {
    if (!snapshot?.meeting_detection) { mode = nextMode; return true; }
    if (contextGeneration === undefined) { actionError = 'Recording setup is waiting for this window’s detection context. Try again after it loads.'; return false; }
    const sequence = ++snapshotRequest;
    const next = await runAction<Snapshot>('set_detection_context', { mode: nextMode, setup_open: setupOpen, context_generation: contextGeneration });
    if (!next) { mode = snapshot.settings.capture_mode; return false; }
    detectionContextOpen = setupOpen;
    contextCloseFailed = false;
    mode = next.settings.capture_mode;
    applySnapshot(next, sequence);
    return true;
  }

  async function openSettings() {
    if (!snapshot || busyCommand) return;
    openingSetup = true;
    try { if (await syncDetectionContext(snapshot.settings.capture_mode, true)) view = 'settings'; }
    finally { openingSetup = false; }
  }

  async function openRecordDialog() { await prepareRecording(); }
  async function prepareRecording(prompt: DetectionPrompt | null = null) {
    if (!snapshot || busyCommand) return;
    if (active) { openMeeting(active.id); return; }
    if (sessionBusy) return;
    openingSetup = true;
    try {
      if (!await syncDetectionContext(snapshot.settings.capture_mode, true)) return;
      if (snapshot.recording && ['recording', 'paused', 'processing', 'stopping'].includes(snapshot.recording.status)) return;
      if (prompt && !snapshot.meeting_detection?.prompts.some(current => current.episode_id === prompt.episode_id && current.app.id === prompt.app.id)) { actionError = 'This microphone-use reminder is no longer current.'; return; }
    newTitle = '';
    mode = snapshot.settings.capture_mode;
    detectionIntent = prompt;
    consent = false;
    actionError = '';
    showConsent = true;
    void loadDevices();
    void tick().then(() => titleInput?.focus());
    } finally { openingSetup = false; }
  }

  async function closeRecordingSetup() {
    if (busyCommand || !await syncDetectionContext(mode, view === 'settings')) return;
    showConsent = false; consent = false; detectionIntent = null;
  }

  async function changeCaptureMode(next: CaptureMode) {
    if (busyCommand) return;
    consent = false;
    await syncDetectionContext(next, true);
  }

  async function dismissDetection(episode: string, suppress: boolean) {
    const sequence = ++snapshotRequest;
    const next = await runAction<Snapshot>('dismiss_detection', { episode_id: episode, suppress });
    if (next) applySnapshot(next, sequence);
  }
  async function dismissEnded(episode: string) {
    const sequence = ++snapshotRequest;
    const next = await runAction<Snapshot>('dismiss_detection_ended', { episode_id: episode });
    if (next) applySnapshot(next, sequence);
  }

  async function runAction<T>(name: string, args?: Record<string, unknown>): Promise<T | undefined> {
    if (busyCommand) return;
    busyCommand = name;
    actionError = '';
    try {
      return await command<T>(name, args);
    } catch (error) {
      actionError = message(error);
      return undefined;
    } finally {
      busyCommand = null;
    }
  }

  async function startRecording(event?: SubmitEvent) {
    event?.preventDefault();
    if (!consent || busyCommand || inputNeedsChoice || !inputDevices || detectionStale) return;
    const sequence = ++snapshotRequest;
    rememberInput();
    const title = newTitle.trim() || 'Untitled meeting';
    const next = detectionIntent
      ? await runAction<Snapshot>('start_detected_recording', { episode_id: detectionIntent.episode_id, title, consent, input_device: selectedInput || null })
      : await runAction<Snapshot>('start_recording', { title, mode, consent_confirmed: consent, ...(selectedInput ? { input_device: selectedInput } : {}) });
    if (next) {
      applySnapshot(next, sequence);
      showConsent = false;
      detectionIntent = null;
      if (next.recording) openMeeting(next.recording.id);
    }
  }

  async function control(name: 'pause_recording' | 'resume_recording' | 'stop_recording') {
    const id = active?.id;
    const sequence = ++snapshotRequest;
    const next = await runAction<Snapshot>(name);
    if (next) {
      applySnapshot(next, sequence);
      if (id && view === 'meeting') await loadDetail(id);
      if (name === 'stop_recording') announce('Recording saved to your library.');
    }
  }

  async function saveNote(bookmark = false) {
    const id = selectedId ?? active?.id;
    const text = bookmark ? '★ Bookmark' : noteText.trim();
    if (!id || !text || busyCommand) return;
    busyCommand = 'save_note';
    actionError = '';
    try {
      await command('save_note', { id, text });
      if (!bookmark) noteText = '';
      announce(bookmark ? 'Moment bookmarked.' : 'Note saved.');
      if (selectedId === id) await loadDetail(id, false);
    } catch (error) { actionError = message(error); }
    finally { busyCommand = null; }
  }

  async function saveTitle(event: SubmitEvent) {
    event.preventDefault();
    if (!selectedId || !editedTitle.trim() || busyCommand) return;
    busyCommand = 'rename_meeting';
    actionError = '';
    try {
      await command('rename_meeting', { id: selectedId, title: editedTitle.trim() });
      editingTitle = false;
      await refresh();
    } catch (error) { actionError = message(error); }
    finally { busyCommand = null; }
  }

  async function exportMeeting() {
    if (!selectedId || !isCurrentRun) return;
    const path = await runAction<string>('export_meeting', { id: selectedId, format: exportFormat });
    if (path) announce(`Export saved: ${path}`);
  }

  async function downloadModel() {
    await runAction('install_model');
    await refresh();
  }

  async function cancelDownload() {
    await runAction('cancel_model_download');
    await refresh();
  }

  async function transcribeMeeting() {
    if (!selectedId || busyCommand || audioUnavailable) return;
    busyCommand = 'transcribe_meeting';
    actionError = '';
    try {
      await command('transcribe_meeting', { id: selectedId });
      announce('Transcription started. You can keep using your library while it runs.');
      await refresh();
    } catch (error) { actionError = message(error); }
    finally { busyCommand = null; }
  }

  async function processMeeting(kind: 'refine' | 'summary') {
    if (!selectedId || busyCommand || (kind === 'refine' && audioUnavailable)) return;
    busyCommand = kind === 'refine' ? 'refine_meeting' : 'generate_summary';
    actionError = '';
    try {
      await command(busyCommand, { id: selectedId });
      announce(kind === 'refine' ? 'Refinement started. Your current transcript stays available.' : 'AI notes are being generated.');
      await refresh();
    } catch (error) { actionError = message(error); }
    finally { busyCommand = null; }
  }
  async function playFromSegment(position: number) {
    if (!selectedId || playbackBlocked || audioUnavailable) return;
    await runAction('play_meeting_audio', { id: selectedId, position_ms: position });
    await refresh();
  }
  async function playBleedExcerpt(track: 'mic' | 'system', position: number) {
    if (!selectedId || playbackBlocked || audioUnavailable || busyCommand) return;
    busyCommand = 'play_bleed_excerpt'; actionError = '';
    try {
      await command('set_playback_volumes', { id: selectedId, mic_volume: track === 'mic' ? 1 : 0, system_volume: track === 'system' ? 1 : 0 });
      await command('play_meeting_audio', { id: selectedId, position_ms: position });
      await refresh();
    } catch (cause) { actionError = message(cause); }
    finally { busyCommand = null; }
  }
  async function cancelProcessing() {
    if (snapshot?.processing?.kind === 'maintenance') return;
    await runAction('cancel_processing');
    await refresh();
  }
  async function selectRun(run: string) {
    selectedRun = run; showHiddenBleed = false; editedSegmentId = ''; highlightedSegment = ''; reviewTargets = {};
    if (selectedId) await loadDetail(selectedId);
  }
  async function saveCorrection() {
    if (!selectedId || !detail?.run_id || !editedSegmentId || !editedSegmentText.trim() || !canEditTranscript || busyCommand) return;
    busyCommand = 'correct_segment'; actionError = '';
    try {
      await command('correct_segment', { id: selectedId, run_id: detail.run_id, segment_id: editedSegmentId, replacement: editedSegmentText });
      editedSegmentId = '';
      await loadDetail(selectedId, false);
      announce('Text correction saved. The original transcript is preserved.');
    } catch (error) { actionError = message(error); }
    finally { busyCommand = null; }
  }
  async function resolveCorrection(correction: string, target?: string) {
    if (!selectedId || !canEditTranscript || busyCommand) return;
    busyCommand = 'resolve_correction'; actionError = '';
    try {
      await command('resolve_correction', { id: selectedId, correction_id: correction, ...(target ? { segment_id: target } : {}) });
      await loadDetail(selectedId, false);
      announce(target ? 'Correction applied to the selected segment.' : 'Correction dismissed.');
    } catch (error) { actionError = message(error); }
    finally { busyCommand = null; }
  }
  async function openCitation(run: string, segment: string) {
    selectedRun = run === detail?.active_run_id ? '' : run;
    detailTab = 'transcript'; transcriptSearch = ''; editedSegmentId = ''; showHiddenBleed = true;
    highlightedSegment = segment;
    if (!selectedId) return;
    await loadDetail(selectedId);
    await tick();
    const entry = document.getElementById(`segment-${segment}`);
    if (entry) { entry.scrollIntoView({ behavior: 'smooth', block: 'center' }); entry.focus({ preventScroll: true }); }
    else actionError = 'This cited segment could not be found in its source transcript.';
  }

  function transcriptScrolled() {
    if (transcriptPanel) followTranscript = transcriptPanel.scrollHeight - transcriptPanel.scrollTop - transcriptPanel.clientHeight < 90;
  }

  async function maintenanceChanged(report: MaintenanceReport, deletedId?: string) {
    storageRefreshRequired = true;
    try {
      const sequence = ++snapshotRequest;
      const next = await command<Snapshot>('app_snapshot');
      applySnapshot(next, sequence);
      if (deletedId === selectedId || (selectedId && !next.meetings.some((meeting) => meeting.id === selectedId))) {
        view = 'library'; selectedId = null; detail = null; selectedRun = '';
      } else if (view === 'meeting' && selectedId) {
        if (!await loadDetail(selectedId)) throw new Error(detailError || 'Meeting data could not be refreshed.');
      }
      storageRefreshRequired = false;
      announce(report.pending_cleanup.length || report.errors.length ? 'Storage maintenance needs attention. Review the maintenance report in Settings.' : 'Storage maintenance finished.');
    } catch (cause) {
      connectionError = `Storage changed. Refresh before using saved audio: ${message(cause)}`;
      throw cause;
    }
  }

  function handleKeys(event: KeyboardEvent) {
    if (document.querySelector('dialog[open]')) return;
    if (showConsent) {
      if (event.key === 'Escape' && !busyCommand) void closeRecordingSetup();
      if (event.key === 'Tab' && consentPanel) {
        const items = Array.from(consentPanel.querySelectorAll<HTMLElement>('button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled])'));
        const first = items[0];
        const last = items.at(-1);
        if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
        else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
      }
      return;
    }
    if ((event.metaKey || event.ctrlKey) && event.altKey && event.key.toLowerCase() === 'r') {
      event.preventDefault();
      openRecordDialog();
    } else if ((event.metaKey || event.ctrlKey) && event.altKey && event.key.toLowerCase() === 'b' && active) {
      event.preventDefault();
      if (selectedId !== active.id) openMeeting(active.id);
      void saveNote(true);
    } else if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'f') {
      event.preventDefault();
      if (view !== 'library') view = 'library';
      void tick().then(() => searchInput?.focus());
    }
  }

  onMount(() => {
    try {
      const storedTheme = localStorage.getItem('vantranscribe-theme');
      if (storedTheme === 'light' || storedTheme === 'dark' || storedTheme === 'system') theme = storedTheme;
      selectedInput = localStorage.getItem('vantranscribe-input') ?? '';
    } catch { /* Keep default session preferences when local storage is unavailable. */ }
    applyTheme();
    const appearance = window.matchMedia('(prefers-color-scheme: dark)');
    appearance.addEventListener('change', applyTheme);
    let alive = true;
    let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      await refresh();
      if (snapshot && !detectionContextReady && !busyCommand && await syncDetectionContext(snapshot.settings.capture_mode, showConsent || view === 'settings')) {
        detectionContextReady = true;
        try { await command<boolean>('frontend_ready'); } catch { /* The next snapshot carries backend readiness errors. */ }
      }
      if (alive) timer = setTimeout(poll, snapshot?.recording || snapshot?.processing || snapshot?.models.busy || snapshot?.refinement_models?.busy || snapshot?.playback?.status === 'playing' ? 250 : 2000);
    }
    void loadDevices();
    void poll().catch(() => { /* Snapshot errors provide the actionable connection message. */ });
    return () => { alive = false; clearTimeout(timer); clearTimeout(noticeTimer); appearance.removeEventListener('change', applyTheme); };
  });
</script>

<svelte:window onkeydown={handleKeys} />

<div class="desktop-shell" inert={showConsent}>
  <aside class="sidebar">
    <a class="brand" href="#library" onclick={(event) => { event.preventDefault(); view = 'library'; }} aria-label="VanTranscribe library">
      <span class="brand-symbol"><Icon name="waveform" size={25} /></span>
      <span>Van<span class="brand-light">Transcribe</span></span>
    </a>
    <div class="workspace-label">Your workspace</div>
    <nav class="main-nav" aria-label="Main navigation">
      <button class:current={view === 'library' || (view === 'meeting' && !isSelectedActive)} onclick={() => { view = 'library'; }}><Icon name="library" /><span>Meeting library</span>{#if snapshot?.meetings.length}<span class="nav-count">{snapshot.meetings.length}</span>{/if}</button>
      {#if active}<button class:current={isSelectedActive && view === 'meeting'} onclick={() => openMeeting(active.id)}><span class="record-dot" class:paused={active.status === 'paused'}></span><span>Current recording</span></button>{/if}
      <button class:current={view === 'settings'} onclick={() => void openSettings()}><Icon name="settings" /><span>Settings</span>{#if snapshot && !snapshot.models.installed}<span class="setup-dot" aria-label="Model setup needed"></span>{/if}</button>
    </nav>

    {#if active}
      <button class="sidebar-recording" onclick={() => openMeeting(active.id)}>
        <span class="eyebrow"><span class="record-dot" class:paused={active.status === 'paused'}></span>{active.status === 'paused' ? 'Recording paused' : 'Recording in progress'}</span>
        <strong>{time(active.elapsed_ms, true)}</strong>
        <span class="sidebar-recording-title">{active.title}</span>
        <span class="small">Open live view <Icon name="arrow" size={13} /></span>
      </button>
    {/if}

    <div class="sidebar-bottom">
      <div class="privacy-mark"><span class="privacy-icon"><Icon name="shield" size={17} /></span><div><strong>Local by default</strong><span>Recording stays on this Mac</span></div></div>
      <div class="app-version">VanTranscribe <span>0.1</span></div>
    </div>
  </aside>

  <main class="workspace">
    <header class="topbar">
      <div class="breadcrumb"><span>Workspace</span><Icon name="arrow" size={12} /><strong>{view === 'settings' ? 'Settings' : view === 'meeting' ? (isSelectedActive ? 'Live recording' : 'Meeting detail') : 'Meeting library'}</strong></div>
      <div class="topbar-actions">
        {#if snapshot?.models.installed}<span class="local-status"><span></span>On-device transcription</span>{/if}
        <button class="btn btn-record btn-sm" onclick={openRecordDialog} disabled={!snapshot || Boolean(busyCommand) || sessionBusy}><Icon name={active ? 'waveform' : 'plus'} size={16} />{active ? 'Live recording' : 'New recording'}<kbd>⌥⌘R</kbd></button>
      </div>
    </header>

    <div class="page-content">
      {#if connectionError}
        <div class="message-banner error-banner" role="alert"><Icon name="alert" /><div><strong>Connection unavailable</strong><p>{connectionError}</p></div><button class="btn btn-sm btn-outline-secondary" onclick={() => void refresh()} disabled={loading}>Retry</button></div>
      {/if}
      {#if failedRecording?.error}<div class="message-banner error-banner" role="alert"><Icon name="alert" /><div><strong>Recording stopped</strong><p>{failedRecording.error}</p></div><button class="btn btn-sm btn-outline-secondary" onclick={openRecordDialog} disabled={Boolean(busyCommand)}>New recording</button></div>{/if}
      {#if actionError && !showConsent}<div class="message-banner error-banner" role="alert"><Icon name="alert" /><p>{actionError}</p>{#if contextCloseFailed}<button class="btn btn-sm btn-outline-secondary" onclick={() => { contextCloseFailed = false; }}>Retry detection setup</button>{/if}<button class="icon-button" aria-label="Dismiss error" onclick={() => { actionError = ''; }}><Icon name="close" size={17} /></button></div>{/if}

      {#if snapshot?.processing}
        <div class="message-banner processing-banner" role="status"><div><strong>{snapshot.processing.kind === 'maintenance' ? 'Storage maintenance' : snapshot.processing.kind === 'refine' ? 'Refining transcript' : snapshot.processing.kind === 'summary' ? 'Generating AI notes' : 'Transcribing recording'}</strong><p>{snapshot.processing.stage} · {Math.round(Math.min(1, Math.max(0, snapshot.processing.progress)) * 100)}%</p>{#if snapshot.processing.error}<p>{snapshot.processing.error}</p>{/if}</div>{#if snapshot.processing.kind !== 'maintenance'}<button class="text-button" onclick={() => snapshot?.processing && openMeeting(snapshot.processing.meeting_id)}>View meeting</button><button class="btn btn-sm btn-outline-secondary" onclick={() => void cancelProcessing()} disabled={Boolean(busyCommand)}>Cancel processing</button>{:else}<span class="field-help">Finishing safely · cannot be cancelled</span>{/if}</div>
      {/if}

      <DetectionNotice {snapshot} blocked={Boolean(busyCommand) || Boolean(connectionError) || sessionBusy || openingSetup} onRecord={(prompt) => void prepareRecording(prompt)} onDismiss={(episode, suppress) => void dismissDetection(episode, suppress)} onDismissEnded={(episode) => void dismissEnded(episode)} onStop={() => void control('stop_recording')} />
      <ResourceStatus {snapshot} />
      {#if view === 'library'}
        <section class="page-intro">
          <div><div class="eyebrow section-eyebrow">Every conversation, captured</div><h1>Meeting library</h1><p>Make room for the conversation. Keep the details here.</p></div>
          <div class="library-totals"><span><strong>{snapshot?.meetings.length ?? 0}</strong> {(snapshot?.meetings.length ?? 0) === 1 ? 'meeting' : 'meetings'}</span><span class="totals-divider"></span><span><Icon name="clock" size={15} />{Math.floor(totalTime / 60000)} min recorded</span></div>
        </section>

        {#if snapshot && !snapshot.models.installed}
          <div class="setup-banner"><span class="setup-banner-icon"><Icon name="download" size={21} /></span><div><strong>{snapshot.models.busy ? 'Preparing on-device transcription' : 'Set up your local transcription model'}</strong><p>{snapshot.models.busy ? `Downloading model · ${modelProgress}%` : 'Download once to turn meeting audio into text, right on your Mac.'}</p></div><button class="btn btn-sm btn-outline-primary" onclick={() => void openSettings()}>{snapshot.models.busy ? 'View progress' : 'Set up model'}<Icon name="arrow" size={14} /></button></div>
        {/if}

        <div class="library-toolbar"><div class="search-field"><Icon name="search" size={18} /><input bind:this={searchInput} bind:value={search} placeholder="Search your meetings" aria-label="Search meetings" /><kbd>⌘F</kbd>{#if search}<button class="icon-button" aria-label="Clear search" onclick={() => { search = ''; }}><Icon name="close" size={15} /></button>{/if}</div><span class="sort-label">Most recent first</span></div>

        {#if searchError}<div class="inline-error" role="alert">{searchError}</div>{/if}
        {#if loading || searchLoading}
          <div class="loading-state" role="status"><span class="spinner-border spinner-border-sm"></span>{searchLoading ? 'Searching transcripts and notes…' : 'Opening your library…'}</div>
        {:else if !meetings.length}
          <section class="empty-library"><span class="empty-illustration"><span class="illustration-lines"></span><Icon name={search ? 'search' : 'waveform'} size={42} /></span><h2>{search ? 'No meetings found' : 'Your next conversation starts here'}</h2><p>{search ? 'Try transcript text, a note, or another title.' : 'Record a meeting, see the conversation take shape, and return to the moments that matter.'}</p>{#if search}<button class="btn btn-outline-secondary" onclick={() => { search = ''; }}>Clear search</button>{:else}<button class="btn btn-record" onclick={openRecordDialog} disabled={!snapshot || Boolean(busyCommand)}><Icon name="plus" size={17} />Record your first meeting</button><div class="empty-reassurance"><Icon name="shield" size={14} />No meeting bot. No audio uploaded.</div>{/if}</section>
        {:else}
          <div class="meeting-list" aria-label="Meetings"><div class="meeting-list-labels"><span>Meeting</span><span>Duration</span><span>Status</span><span></span></div>{#each meetings as meeting (meeting.id)}<button class="meeting-row" onclick={() => openMeeting(meeting.id)}><span class="meeting-row-main"><span class="meeting-glyph" class:live={meeting.status === 'recording'}><Icon name="waveform" size={23} /></span><span><strong>{meeting.title}</strong><span class="meeting-meta">{dateLabel(meeting.started_at)}<span>·</span>{meeting.segment_count ? `${meeting.segment_count} transcript lines` : 'Audio recording'}</span></span></span><span class="meeting-duration">{time(meeting.duration_ms)}</span><span><span class="status-badge" class:status-recording={meeting.status === 'recording'} class:status-ready={meeting.status === 'ready'} class:status-error={meeting.status === 'failed'}>{meeting.status === 'ready' ? 'Saved' : meeting.status.charAt(0).toUpperCase() + meeting.status.slice(1)}</span></span><Icon name="arrow" size={17} /></button>{/each}</div>
        {/if}
      {:else if view === 'settings'}
        <section class="page-intro"><div><div class="eyebrow section-eyebrow">Make it yours</div><h1>Settings</h1><p>A little setup. More attention for the conversation.</p></div></section>
        <div class="settings-layout">
          <LiveModelSettings {snapshot} {refresh} onDownload={() => void downloadModel()} onCancel={() => void cancelDownload()} busy={Boolean(busyCommand)} />
          <MeetingDetectionSettings {snapshot} {inputDevices} {refresh} pageGeneration={pageContextGeneration} blocked={Boolean(busyCommand) || sessionBusy || playbackBlocked} />
          <VoiceMemorySettings pageGeneration={pageContextGeneration} blocked={sessionBusy || playbackBlocked} onChanged={refresh} />
          <AudioSettings {snapshot} {refresh} />
          <ProcessingSettings {snapshot} {refresh} />
          <PrivacyPanel {snapshot} blocked={maintenanceBlocked} onChanged={maintenanceChanged} />
          <section class="settings-section"><div class="settings-section-heading"><span class="settings-icon"><Icon name="mic" size={22} /></span><div><h2>Microphone</h2><p>Choose an input without changing your meeting app’s audio.</p></div></div><label class="form-label settings-label" for="settings-input">Recording input</label><select id="settings-input" class="form-select input-device-select" bind:value={selectedInput} onchange={rememberInput}><option value="">System default microphone</option>{#each inputDevices?.devices ?? [] as device (device.id)}<option value={device.id}>{device.name}{device.bluetooth ? ' · Bluetooth' : ''}{device.is_default ? ' · Default' : ''}</option>{/each}</select>{#if inputNeedsChoice}<p class="field-help bluetooth-note">Your default microphone is Bluetooth. Choose it explicitly or select another input to protect call quality.</p>{/if}{#if deviceError}<div class="inline-error mt-3">{deviceError}<button class="text-button ms-2" onclick={() => void loadDevices()}>Retry</button></div>{/if}</section>
          <section class="settings-section"><h2>Appearance</h2><p class="field-help mb-3">Choose the workspace that feels comfortable.</p><div class="theme-options" role="group" aria-label="Color theme">{#each ['light', 'dark', 'system'] as choice}<button class:selected={theme === choice} aria-pressed={theme === choice} onclick={() => chooseTheme(choice as 'light' | 'dark' | 'system')}>{choice.charAt(0).toUpperCase() + choice.slice(1)}</button>{/each}</div></section>
          <section class="settings-section keyboard-section"><div><h2>Keyboard shortcuts</h2><p>Keep your hands on the conversation.</p></div><dl><div><dt>New recording / live view</dt><dd><kbd>⌥</kbd><kbd>⌘</kbd><kbd>R</kbd></dd></div><div><dt>Bookmark a moment</dt><dd><kbd>⌥</kbd><kbd>⌘</kbd><kbd>B</kbd></dd></div><div><dt>Search meetings</dt><dd><kbd>⌘</kbd><kbd>F</kbd></dd></div></dl></section>
        </div>
      {:else}
        <div class="meeting-heading"><button class="back-link" onclick={() => { view = 'library'; }}><Icon name="back" size={16} />Meeting library</button><div class="meeting-title-line">{#if editingTitle}<form class="rename-form" onsubmit={saveTitle}><input class="form-control" bind:value={editedTitle} aria-label="Meeting title" maxlength="200" /><button class="btn btn-sm btn-primary" disabled={Boolean(busyCommand) || !editedTitle.trim()}>Save</button><button type="button" class="btn btn-sm btn-outline-secondary" onclick={() => { editingTitle = false; }}>Cancel</button></form>{:else}<h1>{currentTitle}</h1><button class="icon-button rename-button" aria-label="Rename meeting" disabled={!detail || Boolean(busyCommand)} onclick={() => { editedTitle = currentTitle; editingTitle = true; }}><Icon name="edit" size={17} /></button>{/if}{#if isSelectedActive}<span class="status-badge" class:status-recording={active?.status === 'recording'}>{active?.status === 'paused' ? 'Paused' : 'Recording'}</span>{/if}</div><p class="meeting-subtitle">{dateLabel(detail?.meeting.started_at ?? '')}{#if detail?.meeting.started_at}<span>·</span>{/if}{isSelectedActive ? time(active?.elapsed_ms ?? 0, true) : time(detail?.meeting.duration_ms ?? 0)}<span>·</span><Icon name="shield" size={13} />Stored on this Mac</p></div>

        {#if isSelectedActive && active}
          <section class="recording-console" aria-label="Recording controls"><div class="recording-timer"><span class="record-indicator" class:paused={active.status === 'paused'}></span><div><span>{active.status === 'paused' ? 'Paused' : 'Recording'}</span><strong>{time(active.elapsed_ms, true)}</strong></div></div><div class="level-group"><div class="level-label"><Icon name="mic" size={15} />Microphone</div><div class="audio-meter" role="meter" aria-label="Microphone input level" aria-valuemin="0" aria-valuemax="100" aria-valuenow={Math.round(Math.min(1, Math.max(0, active.mic_level)) * 100)}><span style:width={`${Math.min(1, Math.max(0, active.mic_level)) * 100}%`}></span></div></div><div class="level-group"><div class="level-label"><Icon name="monitor" size={15} />System audio</div><div class="audio-meter system-meter" role="meter" aria-label="System audio input level" aria-valuemin="0" aria-valuemax="100" aria-valuenow={Math.round(Math.min(1, Math.max(0, active.system_level)) * 100)}><span style:width={`${Math.min(1, Math.max(0, active.system_level)) * 100}%`}></span></div></div><div class="recording-controls"><button class="btn btn-console" onclick={() => void control(active.status === 'paused' ? 'resume_recording' : 'pause_recording')} disabled={Boolean(busyCommand) || !['paused','recording'].includes(active.status)}><Icon name={active.status === 'paused' ? 'play' : 'pause'} size={17} />{active.status === 'paused' ? 'Resume' : 'Pause'}</button><button class="btn btn-stop" onclick={() => void control('stop_recording')} disabled={Boolean(busyCommand)}><Icon name="stop" size={15} />Stop</button></div></section>
          <p class="field-help mb-3">Live VAD: {snapshot?.live_processing?.vad_active ? 'On' : 'Off'} · Speaker labels: {snapshot?.live_processing?.diarization_active ? 'Provisional' : 'Channel labels'}</p>
          <p class="field-help mb-3" role="status">{echoRouteMessage(snapshot?.live_processing?.output_route, snapshot?.live_processing?.echo?.enabled ?? snapshot?.settings.echo_cancellation_enabled ?? true)}</p>
          {#if snapshot?.live_processing?.vad_warning}<p class="field-help mb-3" role="status">{snapshot.live_processing.vad_warning}</p>{/if}
          {#if snapshot?.live_processing?.diarization_warning}<p class="field-help mb-3" role="status">{snapshot.live_processing.diarization_warning}</p>{/if}
          {#if active.error}<div class="message-banner error-banner" role="alert"><Icon name="alert" /><p>{active.error}</p></div>{/if}
          {#if active.gaps > 0}<div class="message-banner caution-banner" role="status"><Icon name="alert" /><p>{active.gaps} audio {active.gaps === 1 ? 'gap has' : 'gaps have'} been marked in this recording.</p></div>{/if}
          {#if snapshot && !active.transcription_configured}<div class="message-banner caution-banner"><Icon name="note" /><p>{snapshot.models.installed ? 'Audio is being saved. Stop this recording, then choose Transcribe to process it with the model you just installed.' : 'Audio is being saved. Install the local model, then transcribe this recording after stopping.'}</p>{#if !snapshot.models.installed}<button class="text-button" onclick={() => void openSettings()}>Set up model</button>{/if}</div>{/if}
        {/if}

        {#if detail?.runs?.length}
          <div class="revision-toolbar"><label>Transcript version<select class="form-select form-select-sm" value={selectedRun} onchange={(event) => void selectRun(event.currentTarget.value)} disabled={detailLoading}><option value="">Current · {activeRun?.kind === 'refine' ? 'Refined' : activeRun?.kind ?? 'Transcript'}</option>{#each detail.runs.filter((run) => run.id !== detail?.active_run_id && run.status === 'complete') as run}<option value={run.id}>{run.kind === 'refine' ? 'Refined' : run.kind} · {dateLabel(run.created_at)} · {run.status}</option>{/each}</select></label>{#if detail.run_id && !isCurrentRun}<span class="history-notice">Historical transcript · read only</span>{/if}</div>
        {/if}
        {#if reviewCorrections.length}
          <section class="correction-review" aria-label="Corrections needing review"><h2>{reviewCorrections.length} {reviewCorrections.length === 1 ? 'correction needs' : 'corrections need'} review</h2><p>These edits no longer match one transcript segment. Choose where each belongs, or dismiss it explicitly.</p>{#each reviewCorrections as correction}<div class="correction-review-item"><p><time>{time(correction.start_ms)}</time> {correction.replacement_text}</p>{#if canEditTranscript}<label>Apply correction to<select class="form-select form-select-sm" bind:value={reviewTargets[correction.id]}><option value="">Choose a current segment</option>{#each detail?.segments.filter((segment) => !segment.id.startsWith('partial-')) ?? [] as segment}<option value={segment.id}>{time(segment.start_ms)} · {segment.text.slice(0, 100)}</option>{/each}</select></label><div class="review-actions"><button class="btn btn-sm btn-outline-primary" disabled={!reviewTargets[correction.id] || Boolean(busyCommand)} onclick={() => void resolveCorrection(correction.id, reviewTargets[correction.id])}>Apply correction</button><button class="text-button" disabled={Boolean(busyCommand)} onclick={() => void resolveCorrection(correction.id)}>Dismiss correction</button></div>{:else}<p class="field-help">Return to the current transcript after recording to resolve this edit.</p>{/if}</div>{/each}</section>
        {/if}
        {#if detail && !isSelectedActive}<InferenceHealth report={detail.inference_health} audioAvailable={!audioUnavailable} />{/if}
        {#if detail}<PrivacyPanel {snapshot} {detail} compact blocked={maintenanceBlocked} onChanged={maintenanceChanged} />{/if}
        {#if audioUnavailable}<div class="message-banner caution-banner" role="status"><Icon name="folder" /><p>Saved audio is unavailable or awaiting storage recovery. Transcripts, notes and citations remain available.</p></div>{/if}
        <div class="detail-toolbar"><div class="detail-tabs" role="tablist" aria-label="Meeting content"><button role="tab" aria-selected={detailTab === 'transcript'} class:chosen={detailTab === 'transcript'} onclick={() => { detailTab = 'transcript'; }}>Transcript{#if detail?.segments.length}<span>{detail.segments.length}</span>{/if}</button><button role="tab" aria-selected={detailTab === 'notes'} class:chosen={detailTab === 'notes'} onclick={() => { detailTab = 'notes'; }}>Notes{#if detail?.notes.length}<span>{detail.notes.length}</span>{/if}</button><button role="tab" aria-selected={detailTab === 'summary'} class:chosen={detailTab === 'summary'} onclick={() => { detailTab = 'summary'; }}>Summary</button><button role="tab" aria-selected={detailTab === 'audio'} class:chosen={detailTab === 'audio'} onclick={() => { detailTab = 'audio'; }}>Audio</button><button role="tab" aria-selected={detailTab === 'speakers'} class:chosen={detailTab === 'speakers'} onclick={() => { detailTab = 'speakers'; }}>Speakers</button></div><div class="export-controls">{#if !isSelectedActive && snapshot?.refinement_models?.installed && detail}<button class="btn btn-sm btn-outline-primary" onclick={() => void processMeeting('refine')} disabled={audioUnavailable || Boolean(busyCommand) || Boolean(active) || Boolean(snapshot?.processing)}>Refine transcript</button>{/if}{#if !isSelectedActive && snapshot?.models.installed && detail && (!detail.segments.length || ['failed', 'recovered'].includes(detail.meeting.status) || detail.meeting.error)}<button class="btn btn-sm btn-primary" onclick={() => void transcribeMeeting()} disabled={audioUnavailable || Boolean(busyCommand) || playbackBlocked || Boolean(snapshot?.processing) || detail.meeting.status === 'processing'}><Icon name="waveform" size={15} />{detail.meeting.status === 'processing' ? 'Transcribing…' : detail.segments.length ? 'Retry transcription' : 'Transcribe'}</button>{/if}<select class="form-select form-select-sm" bind:value={exportFormat} aria-label="Export format"><option value="markdown">Markdown</option><option value="txt">Text</option><option value="srt">SRT</option><option value="vtt">WebVTT</option><option value="json">JSON</option></select><button class="btn btn-sm btn-outline-secondary" onclick={() => void exportMeeting()} disabled={!detail || !isCurrentRun || Boolean(busyCommand)} title={!isCurrentRun ? 'Return to the current transcript to export' : 'Export current transcript'}><Icon name="download" size={15} />Export</button></div></div>
        {#if detail?.meeting.error && detail.meeting.id !== failedRecording?.id && !isSelectedActive}<div class="message-banner error-banner" role="alert"><Icon name="alert" /><p>{detail.meeting.error}</p></div>{/if}
        {#if detail?.summary_error && detailTab !== 'summary'}<div class="message-banner caution-banner" role="status"><Icon name="alert" /><p>AI notes could not be generated. Your transcript is preserved.</p><button class="text-button" onclick={() => { detailTab = 'summary'; }}>View details</button></div>{/if}
        {#if detailError}<div class="message-banner error-banner" role="alert"><Icon name="alert" /><p>{detailError}</p><button class="text-button" onclick={() => selectedId && void loadDetail(selectedId)}>Retry</button></div>{/if}

        <div class="meeting-body"><section class="transcript-column" aria-label={detailTab === 'notes' ? 'Meeting notes' : 'Meeting transcript'}>{#if detailLoading && !detail}<div class="loading-state" role="status"><span class="spinner-border spinner-border-sm"></span>Opening meeting…</div>{:else if detailTab === 'speakers'}<SpeakersPanel {detail} pageGeneration={pageContextGeneration} canEdit={canEditSpeakers} playbackBlocked={playbackBlocked || audioUnavailable} unavailable={audioUnavailable} {refresh} onPlay={(ms) => void playFromSegment(ms)} onSetup={() => { view = 'settings'; }} />{:else if detailTab === 'audio' && selectedId}{#if audioUnavailable}<div class="content-empty"><Icon name="folder" size={30} /><h2>Saved audio is unavailable</h2><p>Playback, waveforms and refinement require saved audio. Your transcript history remains available.</p></div>{:else}<AudioPanel id={selectedId} {snapshot} blocked={playbackBlocked} {refresh} turns={detail?.speaker_turns ?? []} echo={detail?.echo} />{/if}{:else if detailTab === 'summary'}<SummaryPanel {detail} configured={summaryConfigured} canGenerate={canGenerateSummary} generating={snapshot?.processing?.kind === 'summary' && snapshot.processing.meeting_id === selectedId} onGenerate={() => void processMeeting('summary')} onSetup={() => { view = 'settings'; }} onCitation={(run, segment) => void openCitation(run, segment)} />{:else if detailTab === 'notes'}<div class="notes-list">{#if !detail?.notes.length}<div class="content-empty"><Icon name="note" size={30} /><h3>A place for your own thoughts</h3><p>Add a note or bookmark a moment as you listen.</p></div>{:else}{#each detail.notes as note (note.id)}<article class="note-entry"><time>{note.timing_provenance === 'untimed_note' ? 'Untimed' : time(note.t_ms)}</time>{#if noteTimingLabel(note)}<span class="field-help">{noteTimingLabel(note)}</span>{/if}<p>{note.text}</p></article>{/each}{/if}</div>{:else}<BleedPanel {detail} canEdit={canEditSpeakers} playbackBlocked={playbackBlocked || audioUnavailable} unavailable={audioUnavailable} {refresh} onPlay={playBleedExcerpt} bind:showHidden={showHiddenBleed} /><div class="transcript-subbar"><span>{isSelectedActive ? 'Live transcript' : 'Transcript'}<span class="transcript-hint">{isSelectedActive ? 'Saved as the conversation unfolds' : 'Mic and system audio kept separate'}</span></span><label class="transcript-search"><Icon name="search" size={15} /><input bind:value={transcriptSearch} placeholder="Find in transcript" aria-label="Find in transcript" /></label></div><div class="transcript-scroll" bind:this={transcriptPanel} onscroll={transcriptScrolled}>{#if !visibleSegments.length}<div class="content-empty"><Icon name="waveform" size={32} /><h3>{transcriptSearch ? 'No matching lines' : hiddenBleedIds.size && !showHiddenBleed ? 'Confirmed bleed is hidden' : isSelectedActive && active?.transcription_active ? 'Listening for the conversation' : 'No transcript yet'}</h3><p>{transcriptSearch ? 'Try another word or speaker name.' : hiddenBleedIds.size && !showHiddenBleed ? 'Use Show hidden bleed above to review or restore the original lines.' : isSelectedActive && active?.transcription_active ? 'Speech will appear here as it is recognized.' : 'Recognized speech will appear here when transcription is available.'}</p></div>{:else}{#each visibleSegments as segment (segment.id)}<article class="transcript-entry" id={`segment-${segment.id}`} tabindex="-1" class:system-entry={segment.track === 'system'} class:bleed-entry={hiddenBleedIds.has(segment.id)} class:partial-entry={segment.id.startsWith('partial-')} class:cited-entry={highlightedSegment === segment.id}><span class="speaker-avatar">{segment.track === 'mic' ? 'M' : 'S'}</span><div><div class="segment-meta"><button class="speaker-label-button" title="View or name this meeting’s speakers" onclick={() => { detailTab = 'speakers'; }}>{segment.speaker || (segment.track === 'mic' ? 'Me' : 'Them')}</button><button class="segment-time-button" title="Play audio from this moment" aria-label={`Play from ${time(segment.start_ms)}`} disabled={audioUnavailable || playbackBlocked || Boolean(busyCommand)} onclick={() => void playFromSegment(segment.start_ms)}>{time(segment.start_ms)}</button><span>{segment.track === 'mic' ? 'Microphone' : 'System'}</span></div>{#if editedSegmentId === segment.id}<form class="segment-edit" onsubmit={(event) => { event.preventDefault(); void saveCorrection(); }}><label for={`edit-${segment.id}`}>Correct transcript text</label><textarea id={`edit-${segment.id}`} class="form-control" rows="3" maxlength="32000" bind:value={editedSegmentText}></textarea><div><button class="btn btn-sm btn-primary" disabled={!editedSegmentText.trim() || Boolean(busyCommand) || !canEditTranscript}>Save correction</button><button type="button" class="btn btn-sm btn-outline-secondary" onclick={() => { editedSegmentId = ''; }}>Cancel edit</button></div></form>{:else}{#if hiddenBleedIds.has(segment.id)}<span class="partial-label">Confirmed bleed · shown for review</span>{/if}<p><TranscriptWords {segment} assignment={detail?.speaker_assignments?.find((assignment) => assignment.segment_id === segment.id)} /></p>{#if segment.id.startsWith('partial-')}<span class="partial-label">In progress</span>{:else if canEditTranscript}<button class="text-button edit-segment" aria-label={`Edit transcript at ${time(segment.start_ms)}`} onclick={() => { editedSegmentId = segment.id; editedSegmentText = segment.text; }}>Edit text</button>{/if}{/if}</div></article>{/each}{/if}</div>{#if isSelectedActive && !followTranscript}<button class="follow-button" onclick={() => { followTranscript = true; transcriptPanel?.scrollTo({ top: transcriptPanel.scrollHeight, behavior: 'smooth' }); }}>Follow live transcript <span>↓</span></button>{/if}{/if}</section>
          <aside class="notes-panel"><div class="notes-panel-heading"><h2><Icon name="note" size={18} />Your notes</h2><button class="icon-button" onclick={() => void saveNote(true)} disabled={!selectedId || Boolean(busyCommand)} aria-label="Bookmark current moment" title="Bookmark current moment (⌥⌘B)"><Icon name="bookmark" size={18} /></button></div><p class="field-help">Capture a thought without leaving the conversation.</p><form onsubmit={(event) => { event.preventDefault(); void saveNote(); }}><label class="visually-hidden" for="meeting-note">Add a note</label><textarea id="meeting-note" class="form-control" bind:value={noteText} placeholder="Something worth remembering…" rows="4" maxlength="10000" disabled={!detail}></textarea><button class="btn btn-sm btn-primary save-note" disabled={!detail || !noteText.trim() || Boolean(busyCommand)}><Icon name="plus" size={14} />Add note</button></form><div class="sidebar-notes-list">{#each (detail?.notes ?? []).slice(-8).reverse() as note (note.id)}<article><time><Icon name={note.text.startsWith('★') ? 'bookmark' : 'clock'} size={12} />{note.timing_provenance === 'untimed_note' ? 'Untimed' : time(note.t_ms)}</time>{#if noteTimingLabel(note)}<span class="field-help">{noteTimingLabel(note)}</span>{/if}<p>{note.text}</p></article>{/each}</div><div class="notes-footer"><Icon name="shield" size={13} />Notes are saved locally</div></aside>
        </div>
      {/if}
    </div>
  </main>
</div>

{#if notice}<div class="toast-message" role="status"><Icon name="check" size={17} /><span>{notice}</span><button class="icon-button" aria-label="Dismiss notification" onclick={() => { notice = ''; }}><Icon name="close" size={15} /></button></div>{/if}

{#if showConsent}
  <div class="dialog-backdrop"><div class="consent-panel" role="dialog" aria-modal="true" aria-labelledby="record-dialog-title" aria-describedby="record-dialog-description" tabindex="-1" bind:this={consentPanel}><div class="dialog-heading"><span class="dialog-symbol"><Icon name="waveform" size={25} /></span><button class="icon-button" aria-label="Close recording setup" onclick={() => void closeRecordingSetup()} disabled={Boolean(busyCommand)}><Icon name="close" size={20} /></button></div><h2 id="record-dialog-title">A little less note-taking.<br />A little more listening.</h2><p id="record-dialog-description">Choose what to capture, let everyone know, and start your conversation.</p>{#if detectionIntent}<p class="field-help">Opened from {detectionIntent.app.name} microphone use. Confirm the conversation and participant notice before recording.</p>{/if}{#if detectionStale}<p class="inline-error" role="alert">This microphone-use reminder is no longer current. Close setup and wait for a fresh notice, or start a new recording yourself.</p>{/if}<form onsubmit={startRecording}><label class="form-label" for="record-title">Meeting title</label><input id="record-title" class="form-control" bind:this={titleInput} bind:value={newTitle} placeholder="What’s the conversation about?" maxlength="200" /><fieldset class="capture-options"><legend>Capture mode</legend><label class:selected={mode === 'remote'}><input type="radio" bind:group={mode} value="remote" disabled={Boolean(busyCommand)} onchange={() => void changeCaptureMode('remote')} /><span class="capture-option-icon"><Icon name="monitor" size={20} /></span><span><strong>Remote meeting</strong><small>Microphone + system audio</small></span><span class="radio-check"></span></label><label class:selected={mode === 'in_person'}><input type="radio" bind:group={mode} value="in_person" disabled={Boolean(busyCommand)} onchange={() => void changeCaptureMode('in_person')} /><span class="capture-option-icon"><Icon name="mic" size={20} /></span><span><strong>In person</strong><small>Microphone only</small></span><span class="radio-check"></span></label></fieldset><label class="form-label" for="record-input">Microphone</label><select id="record-input" class="form-select dialog-device-select" bind:value={selectedInput}><option value="">System default microphone</option>{#each inputDevices?.devices ?? [] as device (device.id)}<option value={device.id}>{device.name}{device.bluetooth ? ' · Bluetooth' : ''}{device.is_default ? ' · Default' : ''}</option>{/each}</select>{#if inputNeedsChoice}<p class="field-help bluetooth-note">Your default microphone is Bluetooth. Choose it explicitly above or select another input to avoid changing call quality.</p>{/if}{#if deviceError}<div class="inline-error mb-3">{deviceError}<button type="button" class="text-button ms-2" onclick={() => void loadDevices()}>Retry</button></div>{/if}<label class="consent-check"><input class="form-check-input" type="checkbox" bind:checked={consent} /><span>I’ve let participants know this conversation will be recorded.</span></label>{#if !snapshot?.models.installed}<p class="dialog-model-note"><Icon name="note" size={15} />Audio will be saved. Set up the local model to enable transcription.</p>{/if}{#if actionError}<div class="inline-error" role="alert">{actionError}</div>{/if}<div class="dialog-actions"><button type="button" class="btn btn-outline-secondary" onclick={() => void closeRecordingSetup()} disabled={Boolean(busyCommand)}>Cancel</button><button class="btn btn-record" disabled={!consent || Boolean(busyCommand) || inputNeedsChoice || !inputDevices || detectionStale}><span class="record-dot"></span>{['start_recording', 'start_detected_recording'].includes(busyCommand ?? '') ? 'Starting recording…' : 'Start recording'}</button></div></form><div class="dialog-privacy"><Icon name="shield" size={13} />Recorded locally. No bot joins your meeting.</div></div></div>
{/if}
