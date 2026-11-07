<script lang="ts">
  import { onMount } from 'svelte';
  import { command, message } from './api';
  import type { MeetingDetail, SpeakerInfo, VoiceMemoryStatus } from './api';
  let { detail, speaker, allowed, pageGeneration, onSaved, onCancel, onSetup }: { detail: MeetingDetail; speaker: SpeakerInfo; allowed: boolean; pageGeneration: number | undefined; onSaved: () => Promise<void>; onCancel: () => void; onSetup: () => void } = $props();
  let status = $state<VoiceMemoryStatus | null>(null);
  let personId = $state('');
  let name = $state('');
  let isSelf = $state(false);
  let consent = $state(false);
  let method = $state('');
  let attestedBy = $state('');
  let channel = $state<'' | 'remote_wideband' | 'remote_narrowband'>('');
  let busy = $state(false);
  let error = $state('');
  let alive = false;
  const existing = $derived(status?.people.find((person) => person.id === personId));
  const remote = $derived(detail.mode !== 'in_person');
  const canSave = $derived(pageGeneration !== undefined && allowed && status?.enabled && !busy && consent && (existing?.display_name ?? name).trim().length > 0 && attestedBy.trim().length > 0 && (isSelf || existing?.is_self || method.trim().length > 0) && (!remote || Boolean(channel)));
  onMount(() => {
    alive = true; name = speaker.user_name ?? ''; isSelf = false;
    void command<VoiceMemoryStatus>('voice_memory_status').then((next) => { if (alive) { status = next; const saved = next.people.find((person) => person.id === speaker.person_id); if (saved) { personId = saved.id; isSelf = saved.is_self; } } }).catch((cause) => { if (alive) error = message(cause); });
    return () => { alive = false; };
  });
  function choosePerson(value: string) { personId = value; consent = false; method = ''; attestedBy = ''; isSelf = status?.people.find((person) => person.id === value)?.is_self ?? false; }
  async function save() {
    if (!canSave || !detail.run_id || pageGeneration === undefined) return;
    const contextGeneration = pageGeneration;
    const request = { meeting_id: detail.meeting.id, run_id: detail.run_id, speaker_id: speaker.id, display_name: (existing?.display_name ?? name).trim(), person_id: personId || null, is_self: existing?.is_self ?? isSelf, written_consent: consent, channel_type: remote ? channel || null : 'in_person', consent_method: existing?.is_self || isSelf ? 'Own consent in app' : method.trim(), attested_by: attestedBy.trim() };
    busy = true; error = '';
    try { await command<VoiceMemoryStatus>('enroll_voice', { request, context_generation: contextGeneration }); if (alive) await onSaved(); }
    catch (cause) { if (alive) error = message(cause); }
    finally { if (alive) busy = false; }
  }
</script>

<section class="remember-voice" aria-label="Remember voice enrollment" aria-busy={busy}>
  <h3>Remember {speaker.user_name || speaker.label}’s voice</h3>
  <p>This saves an encrypted voiceprint from at least 20 seconds of clean, non-overlapping refined speech. It is separate from saving a meeting name. Automatic recognition remains unavailable until model matching is calibrated.</p>
  {#if error}<div class="message-banner error-banner" role="alert"><p>{error}</p></div>{/if}
  {#if !allowed && !busy}<p role="status">This speaker or transcript changed. Close this form and review the current speaker before enrolling.</p>
  {:else if status && !status.enabled}<p>Enable voice memory in People settings before saving a voice.</p><button class="btn btn-sm btn-outline-primary" onclick={onSetup}>Open People settings</button>
  {:else if status}
    <form onsubmit={(event) => { event.preventDefault(); void save(); }}>
      {#if status.people.length}<label>Save for<select class="form-select form-select-sm" aria-label="Person for voiceprint" value={personId} onchange={(event) => choosePerson(event.currentTarget.value)} disabled={busy}><option value="">New person</option>{#each status.people as person (person.id)}<option value={person.id}>{person.display_name}{person.is_self ? ' · Me' : ''}</option>{/each}</select></label>{/if}
      {#if !existing}<label>Person’s name<input class="form-control form-control-sm" aria-label="Voiceprint person name" bind:value={name} maxlength="120" disabled={busy} /></label><label class="check-row"><input type="checkbox" bind:checked={isSelf} onchange={() => { consent = false; method = ''; }} disabled={busy} /><span>This is my own voice</span></label>{/if}
      <label class="check-row consent"><input type="checkbox" bind:checked={consent} disabled={busy} /><span>{isSelf || existing?.is_self ? 'I consent to storing my own encrypted voiceprint on this computer under the published retention schedule.' : 'This person has separately consented in writing to storing their voiceprint on this computer under the published retention schedule.'}</span></label>
      {#if !isSelf && !existing?.is_self}<label>Written consent method<input class="form-control form-control-sm" aria-label="Written consent method" bind:value={method} placeholder="For example, signed form or email and date" maxlength="500" disabled={busy} /></label>{/if}
      {#if remote}<label>Source audio type<select class="form-select form-select-sm" aria-label="Voiceprint source audio type" bind:value={channel} disabled={busy}><option value="">Choose the source audio type</option><option value="remote_wideband">Meeting audio · wideband</option><option value="remote_narrowband">Phone or headset-call audio · narrowband</option></select></label><p>The system audio tap cannot determine the original meeting codec. Choose based on how this person joined. Phone or headset-call audio may still be saved at a high sample rate. If you do not know the source, do not enroll it.</p>{/if}
      <label>Attested by<input class="form-control form-control-sm" aria-label="Consent attested by" bind:value={attestedBy} placeholder="Your name or local account" maxlength="120" disabled={busy} /></label>
      <p>Saved voiceprints expire after {status.retention_months} {status.retention_months === 1 ? 'month' : 'months'} without a confirmed enrollment or match, or immediately when forgotten in People settings. Deleting this meeting does not delete a separately saved voiceprint.</p>
      <div class="actions"><button class="btn btn-sm btn-primary" disabled={!canSave}>{busy ? 'Saving voice…' : 'Save encrypted voiceprint'}</button><button class="btn btn-sm btn-outline-secondary" type="button" disabled={busy} onclick={onCancel}>Cancel enrollment</button></div>
    </form>
  {:else if !error}<p role="status">Checking voice memory preferences…</p>{/if}
  {#if !status || (!allowed && !busy) || !status.enabled}<button class="btn btn-sm btn-outline-secondary" disabled={busy} onclick={onCancel}>Close enrollment</button>{/if}
</section>

<style>
.remember-voice{border:1px solid var(--border);border-radius:6px;padding:18px;margin:16px 0;background:var(--soft);font-size:12px}.remember-voice h3{font-size:14px;margin:0 0 10px}.remember-voice p{line-height:1.8;max-width:720px}.remember-voice label{display:flex;flex-direction:column;gap:8px;margin:14px 0;max-width:580px;font-size:12px}.remember-voice .check-row{flex-direction:row;align-items:flex-start;gap:9px;line-height:1.8}.check-row input{margin-top:5px;flex-shrink:0}.consent{padding:12px;border:1px solid var(--border);border-radius:5px;background:var(--surface)}.remember-voice .form-control,.remember-voice .form-select{font-size:12px;color:var(--ink);background-color:var(--surface);border-color:var(--border)}.actions{display:flex;gap:10px;flex-wrap:wrap}
</style>
