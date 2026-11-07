<script lang="ts">
  import { command, message, time } from './api';
  import type { MeetingDetail } from './api';
  let { detail, canEdit, playbackBlocked, unavailable = false, refresh, onPlay, showHidden = $bindable(false) }: {
    detail: MeetingDetail | null; canEdit: boolean; playbackBlocked: boolean; unavailable?: boolean; refresh: () => Promise<void>;
    onPlay: (track: 'mic' | 'system', start: number) => Promise<void>; showHidden?: boolean;
  } = $props();
  let busy = $state(false);
  let error = $state('');
  let page = $state(0);
  let hiddenPage = $state(0);
  let genuinePage = $state(0);
  let pendingPage = $state(0);
  const candidates = $derived(detail?.bleed_candidates ?? []);
  const hidden = $derived((detail?.bleed_decisions ?? []).filter((d) => d.status === 'applied' && d.decision === 'bleed'));
  const genuine = $derived((detail?.bleed_decisions ?? []).filter((d) => d.status === 'applied' && d.decision === 'genuine'));
  const pending = $derived((detail?.bleed_decisions ?? []).filter((d) => d.status === 'needs_review'));
  const pageCount = $derived(Math.max(1, Math.ceil(candidates.length / 8)));
  const currentPage = $derived(Math.min(page, pageCount - 1));
  const visibleCandidates = $derived(candidates.slice(currentPage * 8, currentPage * 8 + 8));
  function pages(length: number) { return Math.max(1, Math.ceil(length / 8)); }
  function boundedPage(index: number, length: number) { return Math.min(index, pages(length) - 1); }
  function pageItems<T>(items: T[], index: number): T[] { const start = boundedPage(index, items.length) * 8; return items.slice(start, start + 8); }
  function segment(id: string | null) { return detail?.segments.find((s) => s.id === id); }
  async function decide(id: string, decision: 'bleed' | 'genuine') {
    if (!canEdit || busy || !detail?.run_id) return;
    busy = true; error = '';
    try { await command('set_bleed_decision', { id: detail.meeting.id, run_id: detail.run_id, segment_id: id, decision }); await refresh(); }
    catch (cause) { error = message(cause); }
    finally { busy = false; }
  }
  async function dismiss(id: string) {
    if (!canEdit || busy || !detail?.run_id) return;
    busy = true; error = '';
    try { await command('resolve_bleed_review', { id: detail.meeting.id, run_id: detail.run_id, decision_id: id }); await refresh(); }
    catch (cause) { error = message(cause); }
    finally { busy = false; }
  }
</script>

{#if candidates.length || hidden.length || pending.length || genuine.length}
<section class="bleed-review" aria-label="Microphone echo review">
  {#if unavailable}<p class="field-help">Saved audio excerpts are unavailable. Transcript choices remain reversible.</p>{/if}
  {#if error}<p role="alert" class="text-danger">{error}</p>{/if}
  {#if candidates.length}
    <h3>Possible microphone echo <span>({candidates.length})</span></h3>
    <p class="field-help">Similar microphone and system text needs a listening check. Speech stays visible until you confirm it as bleed.</p>
    {#each visibleCandidates as candidate (candidate.mic_segment_id)}
      {@const mic = segment(candidate.mic_segment_id)}{@const system = segment(candidate.system_segment_id)}
      {#if mic && system}<article class="bleed-review-card" data-bleed-segment={mic.id}>
        <p><strong>Microphone · {time(mic.start_ms)}</strong> {mic.text}</p><p><strong>System · {time(system.start_ms)}</strong> {system.text}</p>
        <div class="bleed-actions"><button class="btn btn-sm btn-outline-secondary" disabled={playbackBlocked || busy} onclick={() => void onPlay('mic', mic.start_ms)}>Play microphone from here</button><button class="btn btn-sm btn-outline-secondary" disabled={playbackBlocked || busy} onclick={() => void onPlay('system', system.start_ms)}>Play system from here</button><button class="btn btn-sm btn-outline-primary" disabled={!canEdit || busy} onclick={() => void decide(mic.id, 'bleed')}>Hide as probable bleed</button><button class="btn btn-sm btn-outline-secondary" disabled={!canEdit || busy} onclick={() => void decide(mic.id, 'genuine')}>Keep as genuine speech</button></div>
      </article>{/if}
    {/each}
    {#if pageCount > 1}<nav class="turn-pagination" aria-label="Microphone echo review pages"><button class="btn btn-sm btn-outline-secondary" disabled={currentPage === 0} onclick={() => { page = currentPage - 1; }}>Previous echo candidates</button><span>Page {currentPage + 1} of {pageCount}</span><button class="btn btn-sm btn-outline-secondary" disabled={currentPage + 1 >= pageCount} onclick={() => { page = currentPage + 1; }}>Next echo candidates</button></nav>{/if}
  {/if}
  {#if hidden.length}<button class="btn btn-sm btn-outline-secondary" aria-expanded={showHidden} onclick={() => { showHidden = !showHidden; }}>{showHidden ? 'Hide confirmed bleed' : 'Show hidden bleed'} ({hidden.length})</button><p class="field-help">Confirmed bleed is excluded from search, AI notes and text exports. Raw transcript and JSON retain the original text. Bleed choices do not change stored audio.</p>
    {#if showHidden}{#each pageItems(hidden, hiddenPage) as decision (decision.id)}{@const entry = segment(decision.segment_id)}{#if entry}<article class="bleed-review-card" data-hidden-bleed={entry.id}><p><strong>Hidden microphone · {time(entry.start_ms)}</strong> {entry.text}</p><button class="btn btn-sm btn-outline-primary" disabled={!canEdit || busy} onclick={() => void decide(entry.id, 'genuine')}>Restore speech</button></article>{/if}{/each}{#if hidden.length > 8}<nav class="turn-pagination" aria-label="Hidden bleed pages"><button class="btn btn-sm btn-outline-secondary" disabled={boundedPage(hiddenPage, hidden.length) === 0} onclick={() => { hiddenPage = boundedPage(hiddenPage, hidden.length) - 1; }}>Previous hidden lines</button><span>Page {boundedPage(hiddenPage, hidden.length) + 1} of {pages(hidden.length)}</span><button class="btn btn-sm btn-outline-secondary" disabled={boundedPage(hiddenPage, hidden.length) + 1 >= pages(hidden.length)} onclick={() => { hiddenPage = boundedPage(hiddenPage, hidden.length) + 1; }}>Next hidden lines</button></nav>{/if}{/if}
  {/if}
  {#if genuine.length}<details class="mt-2"><summary>Kept as genuine speech ({genuine.length})</summary>{#each pageItems(genuine, genuinePage) as decision (decision.id)}{@const entry = segment(decision.segment_id)}{#if entry}<article class="bleed-review-card" data-genuine-bleed={entry.id}><p>{time(entry.start_ms)} · {entry.text}</p><button class="btn btn-sm btn-outline-secondary" disabled={!canEdit || busy} onclick={() => void decide(entry.id, 'bleed')}>Mark as probable bleed</button></article>{/if}{/each}{#if genuine.length > 8}<nav class="turn-pagination" aria-label="Genuine speech review pages"><button class="btn btn-sm btn-outline-secondary" disabled={boundedPage(genuinePage, genuine.length) === 0} onclick={() => { genuinePage = boundedPage(genuinePage, genuine.length) - 1; }}>Previous genuine lines</button><span>Page {boundedPage(genuinePage, genuine.length) + 1} of {pages(genuine.length)}</span><button class="btn btn-sm btn-outline-secondary" disabled={boundedPage(genuinePage, genuine.length) + 1 >= pages(genuine.length)} onclick={() => { genuinePage = boundedPage(genuinePage, genuine.length) + 1; }}>Next genuine lines</button></nav>{/if}</details>{/if}
  {#if pending.length}<h3>Earlier bleed decisions need review</h3><p class="field-help">These earlier lines could not be matched uniquely. Current speech remains visible.</p>{#each pageItems(pending, pendingPage) as decision (decision.id)}<article class="bleed-review-card" data-bleed-review={decision.id}><p>{decision.reason}</p><button class="btn btn-sm btn-outline-secondary" disabled={!canEdit || busy} onclick={() => void dismiss(decision.id)}>Keep this speech visible</button></article>{/each}{#if pending.length > 8}<nav class="turn-pagination" aria-label="Earlier bleed review pages"><button class="btn btn-sm btn-outline-secondary" disabled={boundedPage(pendingPage, pending.length) === 0} onclick={() => { pendingPage = boundedPage(pendingPage, pending.length) - 1; }}>Previous earlier decisions</button><span>Page {boundedPage(pendingPage, pending.length) + 1} of {pages(pending.length)}</span><button class="btn btn-sm btn-outline-secondary" disabled={boundedPage(pendingPage, pending.length) + 1 >= pages(pending.length)} onclick={() => { pendingPage = boundedPage(pendingPage, pending.length) + 1; }}>Next earlier decisions</button></nav>{/if}{/if}
</section>
{/if}
