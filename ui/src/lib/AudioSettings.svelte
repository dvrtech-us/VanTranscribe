<script lang="ts">
  import { command, message } from './api';
  import type { Snapshot } from './api';
  let { snapshot, refresh }: { snapshot: Snapshot | null; refresh: () => Promise<void> } = $props();
  let initialized = $state(false);
  let vad = $state(true);
  let speakers = $state(false);
  let echo = $state(true);
  let acknowledged = $state(false);
  let busy = $state(false);
  let error = $state('');
  let saved = $state('');
  $effect(() => { if (snapshot && !initialized) { vad = snapshot.settings.live_vad_enabled ?? true; speakers = snapshot.settings.diarization_enabled ?? false; echo = snapshot.settings.echo_cancellation_enabled ?? true; initialized = true; } });
  async function action(name: string, args?: Record<string, unknown>) {
    busy = true; error = ''; saved = '';
    try { await command(name, args); await refresh(); }
    catch (cause) { error = message(cause); }
    finally { busy = false; }
  }
  async function save(event: SubmitEvent) {
    event.preventDefault();
    await action('update_audio_preferences', { live_vad_enabled: vad, diarization_enabled: speakers, echo_cancellation_enabled: echo });
    if (!error) saved = 'Audio processing preferences saved. Changes apply to the next recording or refinement.';
  }
</script>

<section class="settings-section">
  <h2>Speech and speaker models</h2><p class="field-help">Speech processing runs on this Mac. Storage retention preferences are separate.</p>
  <div class="model-panel mt-3"><div class="model-name"><span class="model-monogram">V</span><div><strong>Silero voice activity detection</strong><p>About 629 KiB · MIT · local CPU</p></div><span class="status-badge" class:status-ready={snapshot?.vad_models?.installed}>{snapshot?.vad_models?.installed ? 'Installed' : 'Not installed'}</span></div>
    <p class="model-explanation">Skips sustained silence in live recognition while preserving speech onset and decoder context. Also available with the Parakeet pack.</p>
    {#if snapshot?.vad_models?.busy}<progress max="1" value={snapshot.vad_models.progress} aria-label="Voice activity model download progress"></progress><button class="btn btn-sm btn-outline-secondary" disabled={busy} onclick={() => void action('cancel_vad_model_download')}>Cancel voice activity download</button>{:else if !snapshot?.vad_models?.installed}<button class="btn btn-sm btn-primary" disabled={busy || !snapshot} onclick={() => void action('install_vad_model')}>Download voice activity model</button>{/if}
    {#if snapshot?.vad_models?.error}<p class="inline-error" role="alert">{snapshot.vad_models.error}</p>{/if}
  </div>
  <div class="model-panel mt-3"><div class="model-name"><span class="model-monogram">S</span><div><strong>Provisional speaker labels</strong><p>pyannote segmentation + 3D-Speaker ERes2Net · local CPU</p></div><span class="status-badge" class:status-ready={snapshot?.diarization_models?.installed}>{snapshot?.diarization_models?.installed ? 'Installed' : 'Not installed'}</span></div>
    <p class="model-explanation">About 45 MiB to download. Labels are experimental and can change after refinement. Short or overlapping speech remains unknown or overlap. This does not remember voices across meetings.</p>
    <details class="model-attribution"><summary>Speaker model sources and license notice</summary><p>pyannote segmentation 3.0: MIT, © 2022 CNRS. ERes2Net: 3D-Speaker / ModelScope, Apache-2.0. Converted files are downloaded from official k2-fsa/sherpa-onnx releases, with pinned hashes and source notices. The original pyannote Hugging Face acquisition has its own publisher conditions; this app does not log in or accept them. The ERes2Net source model is Chinese; English and meeting-channel thresholds are not calibrated. No weights are bundled with this app.</p></details>
    {#if snapshot?.diarization_models?.busy}<progress max="1" value={snapshot.diarization_models.progress} aria-label="Speaker model download progress"></progress><button class="btn btn-sm btn-outline-secondary" disabled={busy} onclick={() => void action('cancel_diarization_model_download')}>Cancel speaker model download</button>{:else if !snapshot?.diarization_models?.installed}<label class="setting-check"><input type="checkbox" bind:checked={acknowledged} /> I have read the speaker model source and license notice.</label><button class="btn btn-sm btn-primary" disabled={busy || !snapshot || !acknowledged} onclick={() => void action('install_diarization_model', { acknowledge_terms: acknowledged })}>Download speaker models</button>{/if}
    {#if snapshot?.diarization_models?.error}<p class="inline-error" role="alert">{snapshot.diarization_models.error}</p>{/if}
  </div>
  <form class="summary-settings mt-3" onsubmit={save}><label class="setting-check"><input type="checkbox" bind:checked={vad} /> Use live voice activity detection when its model is installed</label><label class="setting-check"><input type="checkbox" bind:checked={speakers} /> Use provisional speaker labels when the speaker models are installed</label><label class="setting-check"><input type="checkbox" bind:checked={echo} /> Reduce microphone echo when the built-in speaker route is verified</label><p class="field-help">Headphones and unverified routes skip echo cancellation. Processing leaves original audio unchanged. Storage retention preferences are separate.</p><p class="field-help">Speaker labels are suggestions, not confirmed identities. Rename a label to identify a participant. Cross-meeting voice memory remains off.</p><button class="btn btn-sm btn-primary" disabled={busy || !snapshot}>Save audio processing preferences</button></form>
  {#if error}<div class="inline-error mt-3" role="alert">{error}</div>{/if}{#if saved}<p class="save-result" role="status">{saved}</p>{/if}
</section>
