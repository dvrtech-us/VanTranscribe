<script lang="ts">
  import { onDestroy } from 'svelte';
  import { command, message } from './api';
  import type { Snapshot, InputDevices, MeetingDetectionSettings, MeetingDetectionRule } from './api';
  let { snapshot, inputDevices, refresh, pageGeneration, blocked = false }: { snapshot: Snapshot | null; inputDevices: InputDevices | null; refresh: () => Promise<void>; pageGeneration: number | undefined; blocked?: boolean } = $props();
  const copy = (value: MeetingDetectionSettings): MeetingDetectionSettings => JSON.parse(JSON.stringify(value));
  const empty = (): MeetingDetectionSettings => ({ enabled: false, rules: [] });
  let draft = $state<MeetingDetectionSettings>(empty());
  let dirty = $state(false);
  let busy = $state(false);
  let error = $state('');
  let saved = $state('');
  let generation = 0;
  let observedSettings = '';
  const apps = $derived(snapshot?.meeting_detection?.catalog ?? []);
  $effect(() => {
    const next = snapshot?.settings.meeting_detection ?? empty();
    const signature = JSON.stringify(next);
    if (signature !== observedSettings && !dirty && !busy) { observedSettings = signature; draft = copy(next); }
  });
  onDestroy(() => { ++generation; });
  function rule(id: string): MeetingDetectionRule {
    return draft.rules.find(rule => rule.app_id === id) ?? { app_id: id, policy: 'prompt', participant_notice_attested: false, input: null };
  }
  function change(id: string, patch: Partial<MeetingDetectionRule>) {
    const next = { ...rule(id), ...patch };
    draft = { ...draft, rules: [...draft.rules.filter(rule => rule.app_id !== id), next] };
    dirty = true; saved = '';
  }
  function inputValue(id: string) { const input = rule(id).input; return input?.kind === 'default' ? '__default__' : input?.kind === 'device' ? input.id : ''; }
  function invalid(): boolean {
    if (!draft.enabled) return false;
    return draft.rules.some(rule => {
      if (rule.policy !== 'always_record') return false;
      const input = rule.input;
      return !rule.participant_notice_attested || !input || !inputDevices || input.kind === 'device' && !inputDevices.devices.some(device => device.id === input.id);
    });
  }
  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (busy || blocked || invalid()) return;
    const contextGeneration = pageGeneration;
    if (contextGeneration === undefined) { error = 'Meeting detection is waiting for this window’s setup context. Try again after it loads.'; return; }
    const request = ++generation;
    const previous = copy(snapshot?.settings.meeting_detection ?? empty());
    busy = true; error = ''; saved = '';
    try {
      const next = await command<Snapshot>('update_meeting_detection', { settings: copy(draft), context_generation: contextGeneration });
      await refresh();
      if (request === generation) { draft = copy(next.settings.meeting_detection ?? previous); dirty = false; saved = 'Meeting detection preferences saved.'; }
    } catch (cause) {
      if (request === generation) { draft = previous; dirty = false; error = message(cause); }
    } finally { if (request === generation) busy = false; }
  }
</script>

<section class="settings-section" aria-labelledby="detection-settings-title">
  <h2 id="detection-settings-title">Meeting detection</h2>
  <p class="field-help">Optional reminders when another app uses the microphone. Detection does not listen to audio or prove that a meeting is happening. In-person mode never prompts or starts automatically.</p>
  <p class="field-help">In-app reminders remain available. Background alerts are controlled by macOS notification settings.</p>
  <form onsubmit={save}>
    <fieldset disabled={busy || blocked || !snapshot?.meeting_detection}>
      <label class="setting-check"><input type="checkbox" checked={draft.enabled} onchange={(event) => { draft.enabled = event.currentTarget.checked; dirty = true; saved = ''; }} /> Enable meeting reminders</label>
      <p class="field-help">Off by default. Prompt opens recording setup; it does not start capture. Recording never stops automatically when an app releases the microphone.</p>
      {#if snapshot?.settings.capture_mode === 'in_person'}<p class="field-help">Detection is paused for in-person mode. Choose Remote meeting in recording setup to use it.</p>{/if}
      <div class="detection-rules">
        {#each apps as app (app.id)}
          {@const current = rule(app.id)}
          <div class="detection-rule">
            <label class="app-policy">{app.name}{app.kind === 'browser' ? ' · Browser' : ''}<select class="form-select" aria-label={`${app.name} detection rule`} value={current.policy} onchange={(event) => change(app.id, { policy: event.currentTarget.value as MeetingDetectionRule['policy'], participant_notice_attested: false, input: null })}><option value="prompt">Prompt</option><option value="suppress">Suppress</option><option value="always_record">Always record</option></select></label>
            {#if app.kind === 'browser'}<p class="field-help">{app.name} microphone use can be dictation, a voice memo or a call. It is not evidence of a meeting.</p>{/if}
            {#if current.policy === 'always_record'}
              <div class="always-record-options">
                <p>Always record starts a remote recording automatically when {app.name} uses the microphone{app.kind === 'browser' ? ', including any browser microphone use outside meetings' : ''}. This applies to future microphone-use episodes after setup is closed.</p>
                <label class="setting-check"><input type="checkbox" checked={current.participant_notice_attested} onchange={(event) => change(app.id, { participant_notice_attested: event.currentTarget.checked })} /> I have informed participants about automatic recording for {app.name}.</label>
                <label>Automatic recording microphone<select class="form-select" aria-label={`${app.name} automatic recording microphone`} value={inputValue(app.id)} onchange={(event) => change(app.id, { input: event.currentTarget.value === '__default__' ? { kind: 'default' } : event.currentTarget.value ? { kind: 'device', id: event.currentTarget.value } : null })}><option value="">Choose an input explicitly</option><option value="__default__">System default microphone · may change</option>{#each inputDevices?.devices ?? [] as device (device.id)}<option value={device.id}>{device.name}{device.bluetooth ? ' · Bluetooth' : ''}</option>{/each}</select></label>
                <p class="field-help">Choose Bluetooth explicitly to allow it. If a saved input disappears, choose another input before enabling automatic recording; recording will not silently switch to another device.</p>
              </div>
            {/if}
          </div>
        {/each}
      </div>
      <button class="btn btn-sm btn-primary" disabled={invalid() || !apps.length}>Save meeting detection preferences</button>
    </fieldset>
  </form>
  {#if !snapshot?.meeting_detection}<p class="field-help">Meeting detection is unavailable in this connection.</p>{/if}
  {#if snapshot?.meeting_detection?.error}<p class="field-help">{snapshot.meeting_detection.error}</p>{/if}
  {#if error}<p class="inline-error" role="alert">{error}</p>{/if}
  {#if saved}<p class="save-result" role="status">{saved}</p>{/if}
</section>
<style>
  fieldset{min-width:0}.detection-rules{margin:18px 0}.detection-rule{border-top:1px solid var(--border);padding:14px 0}.app-policy{display:flex;align-items:center;justify-content:space-between;gap:20px;font-size:12px}.app-policy select{max-width:160px}.detection-rule .field-help{margin:8px 0}.always-record-options{padding:12px;background:var(--soft);border-radius:5px;margin-top:12px;font-size:12px}.always-record-options label:not(.setting-check){display:block;margin:12px 0}.form-select{background-color:var(--surface);color:var(--ink);border-color:var(--border);font-size:12px}.always-record-options p{line-height:1.7}
</style>
