<script lang="ts">
  import { onMount } from 'svelte';
  import { bytesLabel, maintenance, message } from './api';
  import type { DeletionPreview, PolicyPreview, MaintenanceReport } from './api';
  let { deletion = null, cleanup = null, title = '', blocked, onClose, onComplete }: {
    deletion?: DeletionPreview | null; cleanup?: PolicyPreview | null; title?: string; blocked: boolean;
    onClose: () => void; onComplete: (report: MaintenanceReport, deletedId?: string) => Promise<void>;
  } = $props();
  let dialog: HTMLDialogElement;
  let cancelButton: HTMLButtonElement;
  let acknowledged = $state(false);
  let busy = $state(false);
  let error = $state('');
  let settled = $state(false);
  let failedAttempt = $state(false);
  let result = $state<MaintenanceReport | null>(null);
  const actions = $derived(deletion ? [deletion] : cleanup?.actions ?? []);
  const totals = $derived(actions.reduce((sum, action) => ({ files: sum.files + action.files, bytes: sum.bytes + action.bytes, runs: sum.runs + action.runs }), { files: 0, bytes: 0, runs: 0 }));
  const protectedRuns = $derived(cleanup?.protected_runs ?? deletion?.protected_runs ?? 0);
  const heading = $derived(settled ? 'Storage operation finished' : deletion?.kind === 'meeting' ? 'Delete this meeting?' : deletion?.kind === 'audio' ? 'Remove saved audio?' : 'Apply storage cleanup?');
  const confirmText = $derived(deletion?.kind === 'meeting' ? 'Delete meeting' : deletion?.kind === 'audio' ? 'Remove audio' : 'Apply cleanup');
  onMount(() => { dialog.showModal(); cancelButton.focus(); });
  async function confirm() {
    if (busy || blocked || settled || failedAttempt || !acknowledged || !actions.length) return;
    busy = true; error = '';
    try {
      result = deletion ? await maintenance.commitDeletion(deletion.token) : await maintenance.applyCleanup(cleanup!);
    } catch (cause) {
      error = message(cause); acknowledged = false; failedAttempt = true; busy = false; return;
    }
    settled = true;
    try {
      await onComplete(result, deletion?.kind === 'meeting' && result.meetings_deleted > 0 ? deletion.meeting_id : undefined);
      onClose();
    } catch (cause) { error = `The operation returned, but the view could not be refreshed: ${message(cause)}`; }
    finally { busy = false; }
  }
</script>

<dialog bind:this={dialog} class="retention-dialog" aria-labelledby="deletion-heading" oncancel={(event) => { if (busy && !settled) event.preventDefault(); else onClose(); }}>
  <h2 id="deletion-heading">{heading}</h2>
  {#if title}<p class="deletion-title">{title}</p>{/if}
  {#if !settled}
  {#if deletion?.kind === 'meeting'}<p>This removes the meeting’s audio, transcript versions, notes, summaries and annotations from this recordings folder.</p>
  {:else if deletion?.kind === 'audio'}<p>This removes the saved audio. Transcripts, notes, summaries and citations stay available. Playback, waveform viewing and audio processing will no longer be available.</p>
  {:else}<p>Only data eligible under your saved retention preferences will be removed. Referenced transcript history stays protected.</p>{/if}
  <dl class="deletion-counts"><div><dt>Files to remove</dt><dd>{totals.files}</dd></div><div><dt>File space</dt><dd>{bytesLabel(totals.bytes)}</dd></div><div><dt>Transcript versions selected</dt><dd>{totals.runs}</dd></div>{#if protectedRuns}<div><dt>Transcript versions protected</dt><dd>{protectedRuns}</dd></div>{/if}</dl>
  {#if cleanup?.skipped_busy.length}<p class="field-help">{cleanup.skipped_busy.length} busy {cleanup.skipped_busy.length === 1 ? 'meeting was' : 'meetings were'} skipped.</p>{/if}
  {/if}
  <p class="field-help">Exported copies and backups remain where saved. Deletion does not guarantee secure erasure from disks or backups.</p>
  {#if !actions.length}<p role="status">No eligible data to remove.</p>{/if}
  {#if blocked && !busy && !settled}<p class="inline-error" role="status">Finish recording and processing before confirming.</p>{/if}
  {#if error}<div class="inline-error" role="alert">{error}<p class="field-help">{settled ? 'Close this message and refresh the app. This confirmed token will not be submitted again.' : 'Close this preview and review the current data before trying again.'}</p></div>{/if}
  {#if actions.length && !settled}<label class="setting-check"><input type="checkbox" bind:checked={acknowledged} disabled={busy || blocked || failedAttempt} /> I understand that this removal cannot be undone.</label>{/if}
  <div class="deletion-actions"><button class="btn btn-outline-secondary" disabled={busy && !settled} onclick={onClose} bind:this={cancelButton}>{settled ? 'Close' : actions.length ? 'Cancel' : 'Close preview'}</button>{#if actions.length && !settled}<button class="btn btn-record" disabled={!acknowledged || blocked || busy || failedAttempt} onclick={() => void confirm()}>{busy ? 'Removing selected data…' : confirmText}</button>{/if}</div>
  {#if settled && result}<p class="field-help" role="status">{result.meetings_deleted} meetings deleted · {result.audio_purged} audio recordings removed · {result.runs_deleted} versions removed.{#if result.pending_cleanup.length} Some confirmed cleanup remains pending.{/if}</p>{#each result.errors as issue}<p class="inline-error">{issue}</p>{/each}{/if}
  {#if busy && !settled}<p class="field-help" role="status">Finishing the confirmed operation safely. It cannot be cancelled.</p>{/if}
</dialog>

<style>
  .retention-dialog{width:min(540px,calc(100vw - 40px));max-height:calc(100vh - 64px);overflow:auto;border:1px solid var(--border);border-radius:9px;background:var(--surface);color:var(--ink);padding:28px;font-size:12px;line-height:1.7}.retention-dialog::backdrop{background:rgb(14 28 40 / .55)}h2{font-size:20px;margin:0 0 12px}.deletion-title{font-weight:600;overflow-wrap:anywhere}.deletion-counts{border-block:1px solid var(--border);padding:10px 0;margin:18px 0}.deletion-counts>div{display:flex;justify-content:space-between;gap:20px;padding:4px 0}.deletion-counts dt{font-weight:400}.deletion-counts dd{margin:0;font-variant-numeric:tabular-nums}.deletion-actions{display:flex;justify-content:flex-end;gap:10px;margin-top:20px}.field-help{font-size:11px}.inline-error{margin:12px 0}.retention-dialog :global(input){accent-color:var(--red)}
</style>
