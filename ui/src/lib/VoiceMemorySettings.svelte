<script lang="ts">
  import { onMount } from 'svelte';
  import { command, message } from './api';
  import type { VoiceMemoryStatus } from './api';
  let { blocked = false, pageGeneration, onChanged = async () => {} }: { blocked?: boolean; pageGeneration: number | undefined; onChanged?: () => Promise<void> } = $props();
  let status = $state<VoiceMemoryStatus | null>(null);
  let enabled = $state(false);
  let retention = $state(12);
  let acknowledged = $state(false);
  let busy = $state('');
  let error = $state('');
  let notice = $state('');
  let confirmation = $state<{ personId: string | null; label: string; count: number } | null>(null);
  let removeNames = $state(false);
  function defaultConsent(months: number) { return `I would like to save an encrypted voiceprint of your voice in VanTranscribe on this computer to help identify you in future meetings. This is separate from recording consent. It will not be shared, and will be deleted after ${months} ${months === 1 ? 'month' : 'months'} without a confirmed enrollment or match or immediately on your request. You can decline and still participate. Please reply in writing if you agree.`; }
  let consentRequest = $state(defaultConsent(12));
  let consentEdited = $state(false);
  $effect(() => { if (!consentEdited) consentRequest = defaultConsent(retention); });
  let alive = false;
  let request = 0;
  function apply(next: VoiceMemoryStatus) { status = next; enabled = next.enabled; retention = next.retention_months; acknowledged = false; }
  async function load() {
    const current = ++request; error = '';
    try { const next = await command<VoiceMemoryStatus>('voice_memory_status'); if (alive && current === request) apply(next); }
    catch (cause) { if (alive && current === request) error = message(cause); }
  }
  onMount(() => { alive = true; void load(); return () => { alive = false; ++request; }; });
  async function save() {
    if (!status || busy || (enabled && (!acknowledged && !status.enabled || blocked)) || !Number.isInteger(retention) || retention < 1 || retention > 24) return;
    const contextGeneration = pageGeneration;
    if (contextGeneration === undefined) { error = 'This window has not connected its privacy context. Retry after reconnecting.'; return; }
    const current = ++request; busy = 'save'; error = ''; notice = '';
    try {
      const next = await command<VoiceMemoryStatus>('update_voice_memory', { enabled, retention_months: retention, explanation_acknowledged: acknowledged, context_generation: contextGeneration });
      if (alive && current === request) { apply(next); notice = 'Voice memory preferences saved.'; }
    } catch (cause) { if (alive && current === request) error = message(cause); }
    finally { if (alive && current === request) busy = ''; }
  }
  function confirm(personId: string | null) {
    const person = status?.people.find((item) => item.id === personId);
    confirmation = person ? { personId: person.id, label: person.display_name, count: person.voiceprints.length } : { personId: null, label: 'all saved people', count: status?.people.reduce((sum, item) => sum + item.voiceprints.length, 0) ?? 0 };
    removeNames = false; error = ''; notice = '';
  }
  async function forget() {
    if (!confirmation || busy || blocked) return;
    const target = confirmation; const current = ++request; busy = 'delete'; error = ''; notice = '';
    try {
      const next = await command<VoiceMemoryStatus>(target.personId ? 'forget_person' : 'delete_all_voiceprints', { ...(target.personId ? { person_id: target.personId } : {}), remove_names: removeNames });
      if (alive && current === request) { apply(next); confirmation = null; notice = 'Saved voiceprints, consent records and person links removed.'; await onChanged(); }
    } catch (cause) { if (alive && current === request) error = message(cause); }
    finally { if (alive && current === request) busy = ''; }
  }
  async function copyConsent() {
    try { await navigator.clipboard.writeText(consentRequest); if (alive) notice = 'Consent request copied. Review it before sharing.'; }
    catch { if (alive) error = 'Copy is unavailable. Select the request text and copy it manually.'; }
  }
  function channelName(channel: string) { return ({ remote_wideband: 'Remote wideband', remote_narrowband: 'Remote phone/headset', in_person: 'In person' } as Record<string, string>)[channel] ?? channel; }
  function date(value: string) { const parsed = new Date(value); return Number.isNaN(parsed.valueOf()) ? 'Unknown date' : parsed.toLocaleDateString(); }
</script>

<section class="settings-section voice-memory" aria-label="People and voiceprints" aria-busy={Boolean(busy)}>
  <h2>People and voiceprints</h2>
  <p class="field-help">Voiceprints are biometric data. Remembering a voice requires separate consent from that person, even when everyone agreed to recording. Saved voiceprints stay on this computer and are encrypted with a separate key in macOS Keychain.</p>
  <p class="field-help">Automatic recognition is unavailable until matching has been calibrated for the installed speaker model. Saving a voice does not verify a person’s identity or change other meetings’ labels.</p>
  {#if error}<div class="message-banner error-banner" role="alert"><p>{error}</p>{#if !status}<button class="btn btn-sm btn-outline-secondary" onclick={() => void load()}>Retry People</button>{/if}</div>{/if}
  {#if notice}<p class="save-notice" role="status">{notice}</p>{/if}
  {#if !status}{#if !error}<p class="field-help" role="status">Loading voice memory preferences…</p>{/if}
  {:else}
    <form onsubmit={(event) => { event.preventDefault(); void save(); }}>
      <label class="check-row"><input type="checkbox" bind:checked={enabled} disabled={Boolean(busy)} /><span>Enable voice memory</span></label>
      {#if enabled && !status.enabled}<label class="check-row consent-ack"><input type="checkbox" bind:checked={acknowledged} disabled={Boolean(busy)} /><span>I understand that voiceprints are sensitive biometric data and that I must obtain separate written consent before saving another person’s voice.</span></label>{/if}
      <label class="retention-label">Delete voiceprints after no confirmed enrollment or match for<select class="form-select form-select-sm" bind:value={retention} disabled={Boolean(busy)}>{#each Array.from({ length: 24 }, (_, i) => i + 1) as months}<option value={months}>{months} {months === 1 ? 'month' : 'months'}{months === 12 ? ' · default' : ''}</option>{/each}</select></label>
      <p class="field-help retention-schedule"><strong>Retention schedule:</strong> Voiceprints expire after the selected number of months without a confirmed enrollment or match (12 by default), or are deleted immediately when you forget a person. Turning voice memory off prevents new enrollment and use; it keeps saved entries until expiry or deletion. Backups and exported files remain outside this deletion.</p>
      <button class="btn btn-sm btn-outline-primary" type="submit" disabled={Boolean(busy) || enabled && (blocked || !status.enabled && !acknowledged)}>{busy === 'save' ? 'Saving…' : 'Save voice memory preferences'}</button>
    </form>
    {#if status.key_status === 'protected'}<p class="field-help key-status">The encryption key is protected by macOS Keychain. Access is checked when you save a voice.</p>{:else if status.key_status === 'unavailable_platform'}<p class="field-help key-status">Voiceprint encryption is unavailable on this platform. Existing entries can still be deleted.</p>{:else if status.key_status !== 'available' && status.key_status !== 'not_created'}<p class="field-help key-status">Key access: {status.key_status.replaceAll('_', ' ')}. Enrollment requires the encryption key; deleting saved entries does not.</p>{/if}
    <div class="people-heading"><h3>Saved people <span>({status.people.length})</span></h3>{#if status.people.length}<button class="btn btn-sm btn-outline-danger" disabled={Boolean(busy) || blocked} onclick={() => confirm(null)}>Delete all voiceprints</button>{/if}</div>
    {#if !status.people.length}<p class="field-help">No voices are saved. Open a completed refined meeting’s Speakers tab to remember a voice with consent.</p>{:else}<ul class="people-list">{#each status.people as person (person.id)}<li><div><strong>{person.display_name}{person.is_self ? ' · Me' : ''}</strong><p class="field-help">Consent attested {date(person.consent.attested_at)} · {person.consent.method} · by {person.consent.attested_by}</p>{#each person.voiceprints as voice (voice.id)}<p class="field-help">{channelName(voice.channel_type)}{voice.channel_provenance === 'user_declared' ? ' · Source declared by user' : ''} · {voice.n_exemplars} {voice.n_exemplars === 1 ? 'sample' : 'samples'} · expires {date(voice.expires_at)}{voice.stale ? ' · Model version not verified' : ''}</p>{/each}</div><button class="btn btn-sm btn-outline-danger" disabled={Boolean(busy) || blocked} onclick={() => confirm(person.id)}>Forget this person</button></li>{/each}</ul>{/if}
    {#if confirmation}<section class="forget-confirmation" aria-label="Confirm voiceprint deletion"><h3>Forget {confirmation.label}?</h3><p>This removes {confirmation.count} saved {confirmation.count === 1 ? 'voiceprint' : 'voiceprints'}, consent records and person links. Saved speaker-name labels remain unless you remove the linked labels below.</p><label class="check-row"><input type="checkbox" bind:checked={removeNames} disabled={Boolean(busy)} /><span>Remove linked speaker-name labels from past meetings</span></label><p class="field-help">Spoken transcript text, personal notes, stored summaries, source history and exports are kept.</p><div class="actions"><button class="btn btn-sm btn-danger" disabled={Boolean(busy) || blocked} onclick={() => void forget()}>{busy === 'delete' ? 'Deleting…' : 'Confirm deletion'}</button><button class="btn btn-sm btn-outline-secondary" disabled={Boolean(busy)} onclick={() => { confirmation = null; }}>Cancel deletion</button></div></section>{/if}
    <details class="consent-request"><summary>Prepare a consent request</summary><p class="field-help">Edit this request for the person and your retention setting. Nothing is sent automatically.</p><label>Consent request<textarea class="form-control" rows="6" maxlength="4000" bind:value={consentRequest} oninput={() => { consentEdited = true; }}></textarea></label><button class="btn btn-sm btn-outline-secondary" onclick={() => void copyConsent()} disabled={!consentRequest.trim()}>Copy consent request</button></details>
  {/if}
</section>

<style>
.voice-memory>h2{margin-bottom:12px}.voice-memory>.field-help{line-height:1.8}.voice-memory form{margin-top:20px}.check-row{display:flex;gap:9px;align-items:flex-start;font-size:12px;line-height:1.8;margin:13px 0}.check-row input{flex-shrink:0;margin-top:5px}.consent-ack{padding:12px;background:var(--soft);border:1px solid var(--border);border-radius:6px}.retention-label{display:flex;flex-direction:column;gap:9px;font-size:12px;margin:18px 0 10px}.retention-label select{max-width:230px}.voice-memory .form-select,.voice-memory .form-control{color:var(--ink);background-color:var(--surface);border-color:var(--border);font-size:12px}.retention-schedule{line-height:1.8;margin:12px 0 16px}.people-heading{margin-top:28px;display:flex;align-items:center;justify-content:space-between;gap:12px;flex-wrap:wrap}.people-heading h3{font-size:13px;margin:0}.people-heading span{color:var(--muted)}.people-list{list-style:none;padding:0;margin:12px 0}.people-list li{display:flex;justify-content:space-between;align-items:flex-start;gap:14px;border-bottom:1px solid var(--border);padding:15px 0;overflow-wrap:anywhere}.people-list li>div{min-width:0}.people-list .btn{flex-shrink:0}.people-list strong{font-size:13px}.people-list p{margin:7px 0 0}.forget-confirmation{padding:16px;border:1px solid var(--border);background:var(--soft);border-radius:6px;margin-top:16px}.forget-confirmation h3{font-size:14px;overflow-wrap:anywhere}.forget-confirmation p{font-size:12px;line-height:1.8}.actions{display:flex;flex-wrap:wrap;gap:9px;margin-top:14px}.consent-request{margin-top:24px;font-size:12px}.consent-request summary{cursor:pointer}.consent-request label{display:flex;flex-direction:column;gap:8px;margin:12px 0}.save-notice{font-size:12px;color:var(--blue)}.key-status{margin-top:16px}@media(max-width:850px){.people-list li{flex-direction:column}}
</style>
