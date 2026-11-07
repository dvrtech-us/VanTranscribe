<script lang="ts">
  import { bytesLabel, command, message } from './api';
  import type { Snapshot, LiveModel, LiveModelCatalog } from './api';
  import Icon from './Icon.svelte';
  let { snapshot, refresh, onDownload, onCancel, busy }: { snapshot: Snapshot | null; refresh: () => Promise<void>; onDownload: () => void; onCancel: () => void; busy: boolean } = $props();
  let catalog = $state<LiveModelCatalog | null>(null);
  let loading = $state(false);
  let changing = $state(false);
  let error = $state('');
  let revision = 0;
  let signature = '';
  const selected = $derived(catalog?.choices.find(choice => choice.model === catalog?.selected));
  const progress = $derived(Math.round(Math.max(0, Math.min(1, snapshot?.models.progress ?? 0)) * 100));
  const activeWork = $derived(Boolean(snapshot?.processing || snapshot?.recording && ['recording', 'paused', 'processing'].includes(snapshot.recording.status)));
  const blocked = $derived(busy || changing || activeWork || Boolean(snapshot?.models.busy) || !snapshot);
  async function load() {
    const request = ++revision;
    loading = true;
    try { const result = await command<LiveModelCatalog>('live_model_catalog'); if (request === revision) { catalog = result; error = ''; } }
    catch (cause) { if (request === revision) error = message(cause); }
    finally { if (request === revision) loading = false; }
  }
  $effect(() => {
    if (!snapshot) return;
    const next = `${snapshot.models.busy}:${snapshot.models.installed}`;
    if (signature !== next) { signature = next; void load(); }
  });
  function select(event: Event) {
    const control = event.target as HTMLSelectElement;
    return setModel(control.value as LiveModel, control);
  }
  async function setModel(model: LiveModel, control?: HTMLSelectElement) {
    if (blocked || !catalog || model === catalog.selected && !catalog.selection_warning) return;
    changing = true; error = ''; ++revision;
    try { await command<Snapshot>('select_live_model', { model }); await refresh(); await load(); }
    catch (cause) { const failure = message(cause); await load(); error = failure; }
    finally { if (control && catalog) control.value = catalog.selected; changing = false; }
  }

</script>
<section class="settings-section">
  <div class="settings-section-heading"><span class="settings-icon"><Icon name="waveform" size={22} /></span><div><h2>On-device transcription</h2><p>Speech recognition that runs locally on your Mac.</p></div></div>
  <div class="model-panel">
    <div class="model-name"><span class="model-monogram">Z</span><div><strong>Streaming Zipformer</strong><p>English · Live transcription · CPU</p></div><span class="status-badge" class:status-ready={snapshot?.models.installed}>{catalog?.selection_warning ? 'Choice required' : snapshot?.models.busy ? 'Preparing' : snapshot?.models.installed ? 'Installed' : 'Not installed'}</span></div>
    <label class="model-choice">Live English model<select class="form-select" value={catalog?.selected ?? ''} disabled={blocked || loading || !catalog} onchange={(event) => void select(event)}>{#if !catalog}<option value="">Loading model choices…</option>{:else}{#each catalog.choices as choice}<option value={choice.model}>{choice.title}{choice.recommended ? ' · Recommended' : ''}</option>{/each}{/if}</select></label>
    {#if selected}<p class="field-help">{bytesLabel(selected.download_bytes)} download · {bytesLabel(selected.installed_bytes)} installed · {selected.license}</p><details class="model-attribution"><summary>Model source and license</summary><p>{selected.model_id}. Official k2-fsa/sherpa-onnx release, with pinned archive and model-file checksums. Public download with no account or API key. Model weights are downloaded separately.</p><code>{selected.source}</code></details>{/if}
    <p class="model-explanation">The model downloads once. Audio and transcripts stay on this device during transcription. Standard is recommended from the public meeting comparison; quality still depends on your audio. Switching models keeps existing downloads and applies to the next recording or saved transcription.</p>
    {#if activeWork}<p class="field-help">Finish recording and processing before switching the live model.</p>{/if}
    {#if catalog?.selection_warning}<div class="inline-error mt-3" role="alert">{catalog.selection_warning}</div><button class="btn btn-primary mt-3" disabled={blocked || loading} onclick={() => { if (catalog) void setModel(catalog.selected); }}>Confirm this model choice</button>
    {:else if snapshot?.models.busy}<div class="download-progress"><div class="progress" role="progressbar" aria-label="Model download progress" aria-valuenow={progress} aria-valuemin="0" aria-valuemax="100"><div class="progress-bar" style:width={`${progress}%`}></div></div><span>{progress}%</span></div><button class="btn btn-sm btn-outline-secondary" onclick={onCancel} disabled={busy}>Cancel download</button>
    {:else if snapshot?.models.installed}<div class="model-ready"><Icon name="check" size={18} />Ready for your next meeting</div><button class="btn btn-sm btn-outline-secondary mt-3" disabled={blocked} onclick={onDownload}>Verify or repair model</button>
    {:else}<button class="btn btn-primary" onclick={onDownload} disabled={!snapshot || busy || changing}><Icon name="download" size={17} />Download transcription model</button><p class="field-help mt-2 mb-0">You can record audio before the model is installed.</p>{/if}
    {#if snapshot?.models.error && snapshot.models.error !== catalog?.selection_warning}<div class="inline-error mt-3" role="alert">{snapshot.models.error}</div>{/if}
    {#if error}<div class="inline-error mt-3" role="alert">{error}</div><button class="text-button" disabled={loading || changing} onclick={() => void load()}>Retry model choices</button>{/if}
  </div>
</section>
<style>.model-choice{display:block;margin:18px 0 8px;font-size:12px}.model-choice select{margin-top:8px;background:var(--surface);color:var(--ink);border-color:var(--border);font-size:12px}.model-attribution code{display:block;overflow-wrap:anywhere;font-size:10px}</style>
