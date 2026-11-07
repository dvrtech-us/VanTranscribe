<script lang="ts">
  import { onMount } from 'svelte';
  import DeletionDialog from './DeletionDialog.svelte';
  import Icon from './Icon.svelte';
  import { bytesLabel, command, maintenance, message } from './api';
  import type { Snapshot, MeetingDetail, MaintenancePolicy, MaintenanceReport, DeletionKind, DeletionPreview, PolicyPreview } from './api';
  let { snapshot, detail = null, compact = false, blocked = false, onChanged }: {
    snapshot: Snapshot | null; detail?: MeetingDetail | null; compact?: boolean; blocked?: boolean;
    onChanged: (report: MaintenanceReport, deletedId?: string) => Promise<void>;
  } = $props();
  let ready = $state(false);
  let busy = $state(false);
  let keepAudio = $state(true);
  let audioDays = $state<number | undefined>(30);
  let cleanupVersions = $state(true);
  let versionDays = $state<number | undefined>(7);
  let error = $state('');
  let notice = $state('');
  let report = $state<MaintenanceReport | null>(null);
  let savedPolicy = $state<MaintenancePolicy | null>(null);
  let deletion = $state<DeletionPreview | null>(null);
  let cleanup = $state<PolicyPreview | null>(null);
  const unavailable = $derived(detail?.audio_available === false);
  const disabled = $derived(blocked || busy || !snapshot);
  const unsaved = $derived(ready && (savedPolicy?.audio_keep_days !== (keepAudio ? null : audioDays) || savedPolicy?.obsolete_run_days !== versionDays || savedPolicy?.cleanup_obsolete_runs !== cleanupVersions));
  const validDays = $derived((keepAudio || valid(audioDays)) && valid(versionDays));
  function valid(days: number | undefined) { return days !== undefined && Number.isSafeInteger(days) && days > 0 && days <= 4_294_967_295; }
  async function load() {
    busy = true; error = '';
    const [policyResult, reportResult] = await Promise.allSettled([maintenance.policy(), maintenance.lastReport()]);
    if (policyResult.status === 'fulfilled') {
      keepAudio = policyResult.value.audio_keep_days == null;
      audioDays = policyResult.value.audio_keep_days ?? 30;
      cleanupVersions = policyResult.value.cleanup_obsolete_runs ?? true;
      versionDays = policyResult.value.obsolete_run_days;
      savedPolicy = { audio_keep_days: policyResult.value.audio_keep_days, obsolete_run_days: policyResult.value.obsolete_run_days, cleanup_obsolete_runs: cleanupVersions };
      ready = true;
    } else error = message(policyResult.reason);
    if (reportResult.status === 'fulfilled') report = reportResult.value;
    else error = [error, `Maintenance status: ${message(reportResult.reason)}`].filter(Boolean).join(' ');
    busy = false;
  }
  onMount(() => { if (!compact) void load(); });
  async function save(event: SubmitEvent) {
    event.preventDefault(); if (disabled || !ready || !validDays) return;
    busy = true; error = ''; notice = ''; cleanup = null;
    const policy: MaintenancePolicy = { audio_keep_days: keepAudio ? null : audioDays!, obsolete_run_days: versionDays!, cleanup_obsolete_runs: cleanupVersions };
    try { savedPolicy = await maintenance.savePolicy(policy); notice = 'Retention preferences saved. Automatic cleanup runs only while the app is idle.'; }
    catch (cause) { error = message(cause); }
    finally { busy = false; }
  }
  async function preview(kind?: DeletionKind) {
    if (disabled || (!kind && (!ready || unsaved)) || (kind && !detail)) return;
    busy = true; error = ''; notice = '';
    try {
      if (kind && detail) deletion = await maintenance.previewDeletion(detail.meeting.id, kind);
      else cleanup = await maintenance.previewCleanup();
    } catch (cause) { error = message(cause); }
    finally { busy = false; }
  }
  async function completed(next: MaintenanceReport, deletedId?: string) {
    report = next;
    await onChanged(next, deletedId);
  }
  async function openFolder() {
    try { await command('open_data_folder'); } catch (cause) { error = message(cause); }
  }
</script>

<section class:settings-section={!compact} class:meeting-privacy={compact} aria-label={compact ? 'Meeting storage controls' : 'Privacy and retention'}>
  {#if compact}
    <details><summary>Stored meeting data</summary><p class="field-help">Review exactly what will be removed before confirming. Exported copies and backups remain where saved.</p>
      <div class="privacy-actions"><button class="btn btn-sm btn-outline-secondary" disabled={disabled || unavailable} onclick={() => void preview('audio')}>Review audio removal</button><button class="btn btn-sm btn-outline-secondary" disabled={disabled} onclick={() => void preview('meeting')}>Review meeting deletion</button></div>
      {#if blocked}<p class="field-help">Finish any recording and processing before changing stored data.</p>{/if}
    </details>
  {:else}
    <div class="settings-section-heading"><span class="settings-icon"><Icon name="shield" size={22} /></span><div><h2>Privacy and retention</h2><p>Choose how long to keep local audio and transcript versions.</p></div></div>
    <div class="storage-path">{snapshot?.settings.data_dir ?? 'Connecting to local storage…'}</div><button class="btn btn-outline-secondary" disabled={!snapshot || busy} onclick={() => void openFolder()}><Icon name="folder" size={17} />Open data folder</button>
    <p class="field-help mt-3">Local files are unencrypted. Use your Mac’s disk encryption to protect them. Exported copies and backups remain where saved.</p>
    {#if !ready && busy}<p class="field-help" role="status">Loading retention preferences…</p>{/if}
    <form class="retention-form" onsubmit={save}>
      <fieldset disabled={disabled || !ready}><legend>Saved audio</legend><label class="setting-check"><input type="radio" name="audio-retention" checked={keepAudio} onchange={() => { keepAudio = true; }} /> Keep all audio</label><label class="setting-check"><input type="radio" name="audio-retention" checked={!keepAudio} onchange={() => { keepAudio = false; }} /> Remove audio after a set number of days</label><label class="retention-days">Audio retention days<input class="form-control form-control-sm" type="number" min="1" step="1" bind:value={audioDays} disabled={keepAudio || disabled || !ready} /></label><p class="field-help">Audio removal keeps transcripts, notes, summaries and citations. Playback and refinement become unavailable.</p></fieldset>
      <fieldset disabled={disabled || !ready}><legend>Transcript versions</legend><label class="setting-check"><input type="radio" name="version-retention" checked={cleanupVersions} onchange={() => { cleanupVersions = true; }} /> Clean up unreferenced old versions</label><label class="setting-check"><input type="radio" name="version-retention" checked={!cleanupVersions} onchange={() => { cleanupVersions = false; if (!valid(versionDays)) versionDays = 7; }} /> Keep all transcript versions</label><label class="retention-days">Version retention days<input class="form-control form-control-sm" type="number" min="1" step="1" bind:value={versionDays} disabled={!cleanupVersions || disabled || !ready} /></label><p class="field-help">The default is 7 days. Current versions and sources referenced by summaries, text corrections or bleed decisions are protected.</p></fieldset>
      <p class="field-help">Saved preferences authorize automatic cleanup while idle. Cleanup waits during recording, processing or saved-audio playback. Removed data cannot be restored from this app.</p>
      <button class="btn btn-sm btn-primary" disabled={disabled || !ready || !validDays}>Save retention preferences</button>
    </form>
    {#if unsaved}<p class="field-help">Save retention preferences before previewing cleanup.</p>{/if}
    <div class="privacy-actions"><button class="btn btn-sm btn-outline-secondary" disabled={disabled || !ready || unsaved} onclick={() => void preview()}>Preview eligible cleanup</button>{#if !ready && !busy}<button class="text-button" onclick={() => void load()}>Retry retention setup</button>{/if}</div>
    {#if blocked}<p class="field-help">Finish any recording and processing before changing retention preferences or removing data.</p>{/if}
  {/if}
  {#if error}<div class="inline-error mt-3" role="alert">{error}</div>{/if}
  {#if notice}<p class="save-result" role="status">{notice}</p>{/if}
  {#if report && (report.completed || report.errors.length || report.pending_cleanup.length)}<div class="maintenance-result" role="status" aria-label="Storage maintenance result"><strong>Last storage maintenance</strong><p>{report.meetings_deleted} meetings deleted · {report.audio_purged} audio recordings removed · {report.runs_deleted} transcript versions removed · {bytesLabel(report.bytes_reclaimed)} file space reclaimed</p>{#if report.pending_cleanup.length}<p class="inline-error">{report.pending_cleanup.length} {report.pending_cleanup.length === 1 ? 'operation needs' : 'operations need'} cleanup. Affected audio stays unavailable until recovery finishes. Reopen the app to retry confirmed recovery.</p>{/if}{#each report.errors as issue}<p class="inline-error">{issue}</p>{/each}</div>{/if}
</section>
{#if deletion || cleanup}<DeletionDialog {deletion} {cleanup} title={deletion ? detail?.meeting.title : ''} {blocked} onClose={() => { deletion = null; cleanup = null; }} onComplete={completed} />{/if}

<style>
  .meeting-privacy{border-bottom:1px solid var(--border);padding:8px 0 15px;margin:0 0 12px;font-size:11px}.meeting-privacy summary{cursor:pointer;color:var(--muted);width:fit-content}.meeting-privacy details[open] summary{color:var(--ink);margin-bottom:9px}.privacy-actions{display:flex;gap:10px;flex-wrap:wrap;margin-top:16px}.retention-form{margin:22px 0 18px;font-size:12px}.retention-form fieldset{border:0;border-top:1px solid var(--border);padding:18px 0 8px;min-width:0}.retention-form legend{float:none;font-size:13px;font-weight:600;padding:0 8px 0 0;width:auto}.retention-days{display:flex;align-items:center;gap:14px;margin:14px 0;font-size:11px}.retention-days input{max-width:100px;background:var(--surface);color:var(--ink);border-color:var(--border)}.maintenance-result{margin-top:20px;padding-top:14px;border-top:1px solid var(--border);font-size:11px;line-height:1.7;overflow-wrap:anywhere}.maintenance-result p{margin:7px 0}.meeting-privacy .field-help{margin-top:10px}
</style>
