<script lang="ts">
  import { onMount } from 'svelte';
  import { command, defaultSummaryConfig, message } from './api';
  import type { Snapshot, SummaryCapabilities, SummaryConfig } from './api';
  let { snapshot, refresh }: { snapshot: Snapshot | null; refresh: () => Promise<void> } = $props();
  let config = $state<SummaryConfig>({ ...defaultSummaryConfig });
  let autoRefine = $state(true);
  let autoSummary = $state(true);
  let initialized = $state(false);
  let apiKey = $state('');
  let clearKey = $state(false);
  let busy = $state(false);
  let error = $state('');
  let saved = $state('');
  let capabilities = $state<SummaryCapabilities | null>(null);
  let capabilitiesError = $state('');
  const cloud = $derived(['open_ai', 'anthropic', 'compatible'].includes(config.provider));
  const refinementProgress = $derived(Math.round(Math.min(1, Math.max(0, snapshot?.refinement_models?.progress ?? 0)) * 100));
  const models = $derived(capabilities?.local_servers.filter((server) => server.endpoint === config.endpoint).flatMap((server) => server.models) ?? []);
  $effect(() => {
    if (snapshot && !initialized) {
      config = { ...defaultSummaryConfig, ...snapshot.settings.summary_config };
      autoRefine = snapshot.settings.auto_refine ?? true;
      autoSummary = snapshot.settings.auto_summarize ?? true;
      initialized = true;
    }
  });
  function changedOrigin() { config.cloud_consent = false; apiKey = ''; clearKey = false; saved = ''; }
  async function discover() {
    capabilitiesError = '';
    try { capabilities = await command<SummaryCapabilities>('summary_capabilities'); }
    catch (cause) { capabilitiesError = message(cause); }
  }
  async function modelAction(name: string) {
    busy = true; error = '';
    try { await command(name); await refresh(); }
    catch (cause) { error = message(cause); }
    finally { busy = false; }
  }
  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (busy || (cloud && !config.cloud_consent)) return;
    busy = true; error = ''; saved = '';
    // Key material exists only in this component and the explicit IPC request.
    try {
      await command<Snapshot>('update_preferences', { auto_refine: autoRefine, auto_summarize: autoSummary, summary_config: { ...config, endpoint: config.endpoint?.trim() || null, model: config.model?.trim() || null }, ...(apiKey ? { api_key: apiKey } : {}), ...(clearKey ? { clear_key: true } : {}) });
      apiKey = ''; clearKey = false;
      saved = 'Preferences saved.';
      await refresh();
    } catch (cause) { error = message(cause); }
    finally { busy = false; }
  }
  onMount(() => { void discover(); return () => { apiKey = ''; }; });
</script>

<section class="settings-section">
  <div class="settings-section-heading"><div><h2>Post-meeting refinement</h2><p>A second pass with word timestamps, using audio saved on this Mac.</p></div></div>
  <div class="model-panel">
    <div class="model-name"><span class="model-monogram">P</span><div><strong>Parakeet v3 · INT8</strong><p>Multilingual · Silero voice activity detection · CPU</p></div><span class="status-badge" class:status-ready={snapshot?.refinement_models?.installed}>{snapshot?.refinement_models?.installed ? 'Installed' : snapshot?.refinement_models?.busy ? 'Downloading' : 'Not installed'}</span></div>
    <p class="model-explanation">About 465 MiB to download and 640 MiB installed. Refinement publishes a new transcript while preserving the previous version.</p>
    {#if snapshot?.refinement_models?.busy}
      <div class="download-progress"><div class="progress" role="progressbar" aria-label="Refinement model download progress" aria-valuenow={refinementProgress} aria-valuemin="0" aria-valuemax="100"><div class="progress-bar" style:width={`${refinementProgress}%`}></div></div><span>{refinementProgress}%</span></div>
      <button class="btn btn-sm btn-outline-secondary" onclick={() => void modelAction('cancel_refinement_model_download')} disabled={busy}>Cancel refinement download</button>
    {:else if snapshot?.refinement_models?.installed}<div class="model-ready">Ready to refine saved meetings</div>
    {:else}<button class="btn btn-primary" onclick={() => void modelAction('install_refinement_model')} disabled={!snapshot || busy}>Download refinement model</button>{/if}
    {#if snapshot?.refinement_models?.error}<div class="inline-error mt-3" role="alert">{snapshot.refinement_models.error}</div>{/if}
    <details class="model-attribution"><summary>Model license and attribution</summary><p>NVIDIA Corporation: parakeet-tdt-0.6b-v3, CC BY 4.0. Source: https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3. ONNX export and INT8 quantization: Fangjun Kuang / sherpa-onnx contributors. Silero VAD: Silero Team, MIT. License notices are included with the installed pack.</p></details>
  </div>
</section>
<section class="settings-section" id="summary-settings">
  <h2>AI notes</h2><p class="field-help">Choose where transcript text is processed. No provider is enabled automatically.</p>
  <form class="summary-settings" onsubmit={save}>
    <label class="setting-check"><input type="checkbox" bind:checked={autoRefine} /> Refine automatically after recording</label>
    <label class="setting-check"><input type="checkbox" bind:checked={autoSummary} /> Generate AI notes after processing when a provider is configured</label>
    <div class="preference-grid">
      <label>Summary provider<select class="form-select" bind:value={config.provider} onchange={changedOrigin}><option value="none">Off</option><option value="apple">Apple Foundation Models · on device</option><option value="local">Local server · on device</option><option value="open_ai">OpenAI · cloud</option><option value="anthropic">Anthropic · cloud</option><option value="compatible">Compatible server · external</option></select></label>
      <label>Notes template<select class="form-select" bind:value={config.template}><option value="general">General meeting</option><option value="one_on_one">One-on-one</option><option value="standup">Standup</option><option value="client_call">Client call</option><option value="interview">Interview</option></select></label>
      {#if config.provider !== 'none' && config.provider !== 'apple'}
        <label>Model<input class="form-control" bind:value={config.model} list="summary-models" placeholder="Enter a model name" autocomplete="off" /><datalist id="summary-models">{#each models as model}<option value={model}></option>{/each}</datalist></label>
        <label>Server endpoint<input class="form-control" bind:value={config.endpoint} onchange={changedOrigin} placeholder={config.provider === 'local' ? 'http://127.0.0.1:11434/v1' : 'Provider default or https://…'} autocomplete="off" /></label>
      {/if}
      {#if config.provider !== 'none'}<label>Context tokens<input class="form-control" type="number" min="3072" max="131072" step="1" bind:value={config.context_tokens} required /></label>{/if}
    </div>
    {#if config.provider === 'apple'}<p class="provider-availability" role="status">{capabilities ? capabilities.apple.available ? `Apple Foundation Models is available · ${capabilities.apple.context_tokens.toLocaleString()} context tokens.` : `Apple Foundation Models is unavailable. ${capabilities.apple.reason ?? ''}` : 'Checking Apple Foundation Models…'}</p>{/if}
    {#if config.provider === 'local'}
      <div class="local-server-discovery"><p class="field-help">Detected local servers (selecting one does not enable AI notes):</p>{#each capabilities?.local_servers ?? [] as server}<button type="button" class="btn btn-sm btn-outline-secondary" disabled={!server.available} onclick={() => { config.endpoint = server.endpoint; config.model = server.models[0] ?? ''; }}>{server.endpoint} · {server.available ? `${server.models.length} models` : 'Unavailable'}</button>{#if !server.available && server.reason}<p class="field-help">{server.reason}</p>{/if}{/each}<button type="button" class="text-button" onclick={() => void discover()}>Check local servers again</button></div>
    {/if}
    {#if cloud}
      <div class="cloud-settings"><label class="setting-check"><input type="checkbox" bind:checked={config.cloud_consent} /><span>I allow transcript text and notes to be sent to this provider for AI notes.</span></label><p class="field-help">Audio stays on this Mac. The selected provider’s data handling applies to text sent for summaries.</p>
      <label>API key<input class="form-control" type="password" bind:value={apiKey} autocomplete="new-password" placeholder="Leave blank to keep the stored key" disabled={clearKey} /></label>
      <label class="setting-check"><input type="checkbox" bind:checked={clearKey} onchange={() => { if (clearKey) apiKey = ''; }} /> Remove the stored key for this provider and endpoint</label><p class="field-help">Keys are stored in macOS Keychain and are never displayed here.</p></div>
    {/if}
    {#if capabilitiesError}<p class="inline-error">{capabilitiesError}<button type="button" class="text-button ms-2" onclick={() => void discover()}>Retry provider check</button></p>{/if}
    {#if error}<div class="inline-error" role="alert">{error}</div>{/if}
    {#if saved}<p class="save-result" role="status">{saved}</p>{/if}
    <button class="btn btn-primary" disabled={!snapshot || busy || (cloud && !config.cloud_consent)}>{busy ? 'Saving…' : 'Save processing preferences'}</button>
  </form>
</section>
