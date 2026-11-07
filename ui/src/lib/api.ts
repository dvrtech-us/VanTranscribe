import { invoke, isTauri } from '@tauri-apps/api/core';

export type MeetingStatus = 'recording' | 'paused' | 'processing' | 'ready' | 'recovered' | 'failed';
export type CaptureMode = 'remote' | 'in_person';
export interface MeetingApp { id: string; name: string; kind: 'meeting' | 'browser' }
export type DetectionInput = { kind: 'default' } | { kind: 'device'; id: string };
export interface MeetingDetectionRule { app_id: string; policy: 'prompt' | 'suppress' | 'always_record'; participant_notice_attested: boolean; input: DetectionInput | null }
export interface MeetingDetectionSettings { enabled: boolean; rules: MeetingDetectionRule[] }
export interface DetectionPrompt { episode_id: string; app: MeetingApp; message: string }
export interface MeetingDetectionSnapshot { context_generation: number; enabled: boolean; status: 'disabled' | 'watching' | 'unknown' | 'blocked'; error: string | null; catalog: MeetingApp[]; prompts: DetectionPrompt[]; ended: { episode_id: string; meeting_id: string; app_id: string; message: string } | null }
export type ExportFormat = 'markdown' | 'txt' | 'json' | 'srt' | 'vtt';

export interface Meeting {
  id: string;
  title: string;
  status: MeetingStatus;
  started_at: string;
  duration_ms: number;
  segment_count: number;
  error?: string | null;
}

export interface ActiveRecording {
  id: string;
  title: string;
  status: MeetingStatus | 'stopping';
  elapsed_ms: number;
  mic_level: number;
  system_level: number;
  gaps: number;
  transcription_configured?: boolean;
  transcription_active?: boolean;
  error?: string | null;
}

export interface EchoInfo { enabled: boolean; applied_mic_ms: number; bypassed_mic_ms: number; engine: string; warnings: string[] }
export interface OutputRoute { kind: 'built_in_speakers' | 'headphones' | 'external' | 'unknown'; reason: string; transport: string }

export interface Snapshot {
  meeting_detection?: MeetingDetectionSnapshot;
  health?: HealthSnapshot;
  source_timing?: SourceTiming | null;
  recording: ActiveRecording | null;
  meetings: Meeting[];
  models: ModelStatus;
  refinement_models: ModelStatus;
  vad_models?: ModelStatus;
  diarization_models?: ModelStatus;
  playback?: PlaybackSnapshot | null;
  live_processing?: { backlog?: BacklogSnapshot; vad_active: boolean; diarization_active: boolean; vad_warning?: string | null; diarization_warning?: string | null; output_route?: OutputRoute | null; echo?: EchoInfo | null } | null;
  processing: { meeting_id: string; kind: 'refine' | 'replay' | 'summary' | 'maintenance'; stage: string; progress: number; error?: string | null } | null;
  settings: { meeting_detection?: MeetingDetectionSettings; capture_mode: CaptureMode; data_dir: string; auto_refine: boolean; auto_summarize: boolean; summary_config: SummaryConfig; live_vad_enabled?: boolean; diarization_enabled?: boolean; echo_cancellation_enabled?: boolean };
}

export interface PlaybackSnapshot { meeting_id: string; status: string; position_ms: number; duration_ms: number; mic_volume: number; system_volume: number; error?: string | null }
export interface AudioWaveform { meeting_id: string; duration_ms: number; mic: number[]; system: number[] }

export interface Segment {
  id: string;
  track: 'mic' | 'system';
  start_ms: number;
  end_ms: number;
  text: string;
  speaker: string;
  words: { text: string; start_ms: number; end_ms: number }[];
}

export interface MeetingDetail {
  mode?: CaptureMode;
  audio_available?: boolean;
  inference_health?: BacklogReport | null;
  meeting: Meeting;
  segments: Segment[];
  notes: { id: string; t_ms: number; text: string; timing_provenance?: TimingProvenance | null; timing_synchronized?: boolean | null }[];
  summary?: string | null;
  summary_error?: string | null;
  active_run_id: string | null;
  run_id: string | null;
  runs: RunInfo[];
  corrections: Correction[];
  summaries: SummaryInfo[];
  correction_version: number;
  speakers?: SpeakerInfo[];
  speaker_turns?: SpeakerTurn[];
  speaker_reviews?: SpeakerReview[];
  speaker_assignments?: SpeakerAssignment[];
  speaker_constraints?: SpeakerConstraint[];
  echo?: EchoInfo | null;
  bleed_decisions?: BleedDecision[];
  bleed_candidates?: BleedCandidate[];
  diarization?: { embedding_version: string; calibrated: boolean; warnings: string[]; segmentation_context?: { source_window_ms: number; ownership_stride_ms: number; native_window_ms: number } | null } | null;
}
export interface VoiceprintInfo { id: string; channel_type: string; channel_provenance?: string; embedding_version: string; n_exemplars: number; last_matched_at: string; expires_at: string; stale: boolean }
export interface PersonInfo { id: string; display_name: string; is_self: boolean; voiceprints: VoiceprintInfo[]; consent: { attested_at: string; method: string; attested_by: string } }
export interface VoiceMemoryStatus { enabled: boolean; retention_months: number; matching_available: boolean; key_status: string; people: PersonInfo[] }
export interface SpeakerInfo { id: string; track: string; label: string; user_name: string | null; speech_ms: number; provisional: boolean; shared_room?: boolean; person_id?: string | null }
export interface SpeakerTurn { id: string; track: string; start_ms: number; end_ms: number; speaker_ids: string[]; label: string; provenance: string; score: number | null; overlap: boolean; uncertain: boolean; provisional: boolean }
export interface SpeakerReview { constraint_id?: string | null; id: string; kind: string; previous_ids: string[]; previous_labels: string[]; current_ids: string[]; reason: string; status: string }
export interface SpeakerAssignment { segment_id: string; speaker_ids: string[]; provenance: string; overlap: boolean; uncertain: boolean; words: { index: number; speaker_ids: string[]; label: string; overlap: boolean; uncertain: boolean }[] }
export interface ModelStatus { installed: boolean; busy: boolean; progress: number; error?: string | null }
export interface RunInfo { id: string; meeting_id: string; kind: string; status: string; engines_json: string; constraints_version: number; created_at: string }
export interface Correction { id: string; track: string; start_ms: number; end_ms: number; original_text_hash: string; replacement_text: string; status: 'applied' | 'needs_review' | 'dismissed'; segment_id: string | null }
export interface SummaryInfo { id: string; meeting_id: string; run_id: string; correction_version: number; provider: string; model: string; template: string; content_json: string; content_md: string; is_stale: boolean; created_at: string; source_segment_ids: string[] }
export interface SummaryConfig { provider: 'none' | 'apple' | 'local' | 'open_ai' | 'anthropic' | 'compatible'; endpoint?: string | null; model?: string | null; context_tokens: number; template: string; cloud_consent: boolean }
export interface SummaryCapabilities { apple: { available: boolean; reason?: string | null; context_tokens: number }; local_servers: { endpoint: string; available: boolean; models: string[]; reason?: string | null }[] }
export interface CitedItem { text: string; source_ids: string[] }
export interface SummaryEnvelope { content: { tldr: CitedItem[]; key_points: CitedItem[]; decisions: CitedItem[]; action_items: { owner: string | null; task: string; due: string | null; source_ids: string[] }[]; open_questions: CitedItem[] }; partial: boolean; errors: string[] }
export const defaultSummaryConfig: SummaryConfig = { provider: 'none', endpoint: null, model: null, context_tokens: 4096, template: 'general', cloud_consent: false };

export interface InputDevices {
  devices: { id: string; name: string; is_default: boolean; bluetooth: boolean }[];
  system_supported: boolean;
}

export function command<T>(name: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) {
    return Promise.reject(new Error('Open VanTranscribe as a desktop app to connect to your local recordings.'));
  }
  return invoke<T>(name, args);
}

export function message(error: unknown): string {
  if (error instanceof Error) return error.message;
  return typeof error === 'string' ? error : 'Something went wrong. Please try again.';
}

export function time(milliseconds: number, hours = false): string {
  const seconds = Math.max(0, Math.floor(milliseconds / 1000));
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  return h || hours
    ? `${h.toString().padStart(2, '0')}:${m.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`
    : `${m.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`;
}

export function dateLabel(date: string): string {
  const value = new Date(date);
  return Number.isNaN(value.getTime()) ? '' : value.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });
}

export interface SpeakerConstraint { id: string; kind: string; left: { track: string; start_ms: number; end_ms: number }; right: { track: string; start_ms: number; end_ms: number }; active: boolean }

export function echoRouteMessage(route: OutputRoute | null | undefined, enabled = true): string {
  if (!enabled) return 'Echo cancellation is off in audio processing preferences.';
  if (!route) return 'Waiting for output route evidence. Echo cancellation is skipped until the route can be verified.';
  const reasons: Record<string, string> = {
    eligible: 'The built-in speaker route is verified. Echo cancellation can use the System track as its reference.',
    headphones: 'Headphones detected. Echo cancellation is skipped.',
    external_output: 'External output detected. Use headphones to reduce microphone echo.',
    mixed_process_outputs: 'Audio is playing through different output devices. Use headphones to reduce microphone echo.',
    no_active_output: 'No active system output is available as an echo reference.',
    no_reference: 'System audio is unavailable as an echo reference.',
  };
  return reasons[route.reason] ?? 'The output route could not be verified. Use headphones to reduce microphone echo.';
}

export interface BleedDecision { id: string; segment_id: string | null; decision: 'bleed' | 'genuine'; reason: string; status: 'applied' | 'needs_review' | 'dismissed' }
export interface BleedCandidate { mic_segment_id: string; system_segment_id: string; similarity: number }

export interface MaintenancePolicy { audio_keep_days: number | null; obsolete_run_days: number; cleanup_obsolete_runs: boolean }
export type DeletionKind = 'meeting' | 'audio';
export interface DeletionPreview { token: string; meeting_id: string; kind: 'meeting' | 'audio' | 'runs'; files: number; bytes: number; runs: number; fingerprint: string; protected_runs: number; protected_active: number; protected_references: number; foreign_key_tables: string[] }
export interface PolicyPreview { policy: MaintenancePolicy; actions: DeletionPreview[]; protected_runs: number; skipped_busy: string[] }
export interface MaintenanceReport { completed: number; meetings_deleted: number; audio_purged: number; runs_deleted: number; bytes_reclaimed: number; pending_cleanup: string[]; errors: string[] }
export interface HealthSnapshot { free_bytes: number | null; checked_at: string | null; disk_state: 'healthy' | 'warning' | 'inference_suspended' | 'critical' | 'unknown'; inference_suspended: boolean; automatic_processing_allowed: boolean; warning: string | null }
export type TimingProvenance = 'uptime_raw' | 'latest_audio_observation' | 'saved_playback_position' | 'untimed_note' | 'unavailable';
export interface SourceTiming { synchronized: boolean; provenance: TimingProvenance; elapsed_ms: number; latest_audio_ms: number | null; warning: string | null }
export function bytesLabel(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value) || value < 0) return 'Unknown';
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB']; let size = value; let unit = 0;
  while (size >= 1024 && unit < units.length - 1) { size /= 1024; unit++; }
  return `${size.toLocaleString(undefined, { maximumFractionDigits: unit ? 1 : 0 })} ${units[unit]}`;
}
export const maintenance = {
  policy: () => command<MaintenancePolicy>('maintenance_policy'),
  savePolicy: (policy: MaintenancePolicy) => command<MaintenancePolicy>('save_maintenance_policy', { policy }),
  previewDeletion: (id: string, kind: DeletionKind) => command<DeletionPreview>('preview_deletion', { id, kind }),
  commitDeletion: (token: string) => command<MaintenanceReport>('commit_deletion', { token }),
  previewCleanup: () => command<PolicyPreview>('preview_cleanup'),
  applyCleanup: (preview: PolicyPreview) => command<MaintenanceReport>('apply_cleanup', { preview }),
  lastReport: () => command<MaintenanceReport>('last_maintenance_report'),
};
export function noteTimingLabel(note: { timing_provenance?: TimingProvenance | null; timing_synchronized?: boolean | null }): string {
  if (note.timing_provenance === 'untimed_note') return 'Untimed note';
  if (note.timing_provenance === 'saved_playback_position') return 'Playback position';
  if (note.timing_synchronized === true) return 'Source synchronized';
  if (note.timing_provenance === 'latest_audio_observation') return 'Approximate audio position';
  if (note.timing_provenance === 'unavailable' || note.timing_synchronized === false) return 'Timing unavailable';
  return 'Timing unverified';
}

export type LiveModel = 'english_small' | 'english_standard';
export interface LiveModelChoice { model: LiveModel; title: string; recommended?: boolean; model_id: string; installed: boolean; download_bytes: number; installed_bytes: number; source: string; license: string }
export interface LiveModelCatalog { selected: LiveModel; choices: LiveModelChoice[]; selection_warning?: string | null }

export type BacklogStage = 'normal' | 'partials_disabled' | 'speakers_disabled' | 'stt_disabled';
export interface BacklogSnapshot { stage: BacklogStage; lag_ms: number; partials_enabled: boolean; speakers_enabled: boolean; stt_enabled: boolean; warning: string | null }
export interface BacklogReport { complete?: boolean; stages: { stage: BacklogStage; source_ms: number; lag_ms: number }[]; gaps: { worker: string; track: string; generation: number; start_ms: number; end_ms: number; reason: string }[]; warnings: string[]; truncated: boolean; max_lag_ms: number }
