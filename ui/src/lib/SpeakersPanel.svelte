<script lang="ts">
  import Icon from './Icon.svelte';
  import RememberVoice from './RememberVoice.svelte';
  import { command, message, time } from './api';
  import type { MeetingDetail, SpeakerInfo } from './api';

  type Speaker = SpeakerInfo;
  interface Turn { id: string; track: string; start_ms: number; end_ms: number; speaker_ids: string[]; label: string; provenance: string; score?: number | null; overlap: boolean; uncertain: boolean; provisional: boolean }
  interface SpeakerConstraint { id: string; kind: string; left: { track: string; start_ms: number; end_ms: number }; right: { track: string; start_ms: number; end_ms: number }; active: boolean }
  interface Review { constraint_id?: string | null; id: string; kind: string; previous_ids: string[]; previous_labels: string[]; current_ids: string[]; reason: string; status: string }
  let { detail, canEdit, playbackBlocked, unavailable = false, pageGeneration, refresh, onPlay, onSetup }: {
    detail: MeetingDetail | null; canEdit: boolean; playbackBlocked: boolean; unavailable?: boolean; pageGeneration: number | undefined;
    refresh: () => Promise<void>; onPlay: (ms: number) => void; onSetup: () => void;
  } = $props();

  const PAGE_SIZE = 64;
  let scope = $state('');
  let names = $state<Record<string, string>>({});
  let previousChoices = $state<Record<string, string>>({});
  let currentChoices = $state<Record<string, string>>({});
  let left = $state('');
  let right = $state('');
  let relationship = $state<'' | 'must_link' | 'cannot_link'>('');
  let page = $state(0);
  let excerptPage = $state(0);
  let constraintPage = $state(0);
  let busy = $state('');
  let error = $state('');
  let notice = $state('');
  let remembering = $state('');

  const speakers = $derived<Speaker[]>(detail?.speakers ?? []);
  const turns = $derived<Turn[]>(detail?.speaker_turns ?? []);
  const pendingReviews = $derived<Review[]>((detail?.speaker_reviews ?? []).filter((review) => !['resolved', 'dismissed'].includes(review.status)));
  const reviews = $derived(pendingReviews.filter((review) => review.kind !== 'constraint'));
  const constraintReviews = $derived(pendingReviews.filter((review) => review.kind === 'constraint'));
  const constraints = $derived<SpeakerConstraint[]>((detail?.speaker_constraints ?? []).filter((constraint) => constraint.active));
  const constraintPages = $derived(Math.max(1, Math.ceil(constraints.length / PAGE_SIZE)));
  const correctionPage = $derived(Math.min(constraintPage, constraintPages - 1));
  const visibleConstraints = $derived(constraints.slice(correctionPage * PAGE_SIZE, (correctionPage + 1) * PAGE_SIZE));
  const historical = $derived(Boolean(detail?.run_id && detail.run_id !== detail.active_run_id));
  const editable = $derived(canEdit && !historical && Boolean(detail?.run_id));
  const cleanTurns = $derived(turns.filter((turn) => !turn.overlap && !turn.uncertain && !turn.provisional && turn.end_ms - turn.start_ms >= 1000 && turn.speaker_ids.length > 0));
  const totalPages = $derived(Math.max(1, Math.ceil(turns.length / PAGE_SIZE)));
  const currentPage = $derived(Math.min(page, totalPages - 1));
  const visibleTurns = $derived(turns.slice(currentPage * PAGE_SIZE, (currentPage + 1) * PAGE_SIZE));
  const excerptPages = $derived(Math.max(1, Math.ceil(cleanTurns.length / PAGE_SIZE)));
  const choicePage = $derived(Math.min(excerptPage, excerptPages - 1));
  const excerptChoices = $derived(cleanTurns.slice(choicePage * PAGE_SIZE, (choicePage + 1) * PAGE_SIZE));
  const leftTurn = $derived(cleanTurns.find((turn) => turn.id === left));
  const rightTurn = $derived(cleanTurns.find((turn) => turn.id === right));
  const validPair = $derived(Boolean(leftTurn && rightTurn && left !== right && leftTurn.track === rightTurn.track && relationship));
  const provisional = $derived(speakers.some((speaker) => speaker.provisional) || turns.some((turn) => turn.provisional));

  $effect(() => {
    const next = detail ? `${detail.meeting.id}|${detail.run_id ?? ''}` : '';
    if (next !== scope) {
      scope = next; names = {}; previousChoices = {}; currentChoices = {};
      left = ''; right = ''; relationship = ''; remembering = ''; page = 0; excerptPage = 0; constraintPage = 0; error = ''; notice = '';
    }
  });

  function trackName(track: string) { return track === 'mic' ? 'Microphone' : track === 'system' ? 'System audio' : track; }
  function speakerName(id: string) { const speaker = speakers.find((item) => item.id === id); return speaker?.user_name || speaker?.label || 'Unknown speaker'; }
  function turnName(turn: Turn) {
    if (turn.overlap) return turn.speaker_ids.length ? `Overlap · ${turn.speaker_ids.map(speakerName).join(' + ')}` : 'Overlapping voices';
    if (turn.uncertain || turn.provisional) return 'Unknown speaker';
    return turn.speaker_ids.length ? turn.speaker_ids.map(speakerName).join(' + ') : turn.label || 'Unknown speaker';
  }
  function excerptName(turn: Turn) { return `${time(turn.start_ms)}–${time(turn.end_ms)} · ${trackName(turn.track)} · ${turnName(turn)}`; }
  function chooseLeft(id: string) { left = id; const first = cleanTurns.find((turn) => turn.id === id); const second = cleanTurns.find((turn) => turn.id === right); if (right === id || (second && first && second.track !== first.track)) right = ''; }
  function play(ms: number) { if (!playbackBlocked && detail) onPlay(ms); }
  function reviewTargets(review: Review) { return speakers.filter((speaker) => review.current_ids.includes(speaker.id)); }
  async function mutate(key: string, name: string, args: Record<string, unknown>, success: string, done?: () => void) {
    if (!editable || busy || !detail?.run_id) return;
    const meeting = detail.meeting.id; const run = detail.run_id;
    busy = key; error = ''; notice = '';
    try {
      await command(name, { id: meeting, ...args });
      await refresh();
      if (detail?.meeting.id === meeting && detail?.run_id === run) { done?.(); notice = success; }
    } catch (cause) { if (detail?.meeting.id === meeting && detail?.run_id === run) error = message(cause); }
    finally { busy = ''; }
  }
  const refinedContext = $derived(detail?.diarization?.segmentation_context);
  const refined = $derived(detail?.runs.find((run) => run.id === detail.run_id)?.kind === 'refine' && detail?.runs.find((run) => run.id === detail.run_id)?.status === 'complete' && refinedContext?.source_window_ms === 10_000 && refinedContext.ownership_stride_ms === 2_500 && refinedContext.native_window_ms === 10_000);
  const rememberedSpeaker = $derived(speakers.find((speaker) => speaker.id === remembering));
  function cleanSpeech(speaker: Speaker) {
    const ranges = turns.filter((turn) => !turn.overlap && !turn.uncertain && !turn.provisional && turn.speaker_ids.length === 1 && turn.speaker_ids[0] === speaker.id && turn.track === speaker.track && turn.end_ms > turn.start_ms).map((turn) => [turn.start_ms, turn.end_ms]).sort((a,b) => a[0] - b[0]);
    let total = 0; let end = 0;
    for (const range of ranges) { total += Math.max(0, range[1] - Math.max(end, range[0])); end = Math.max(end, range[1]); }
    return total;
  }
  function canRemember(speaker: Speaker) { return editable && refined && !unavailable && !playbackBlocked && !speaker.provisional && !speaker.shared_room && cleanSpeech(speaker) >= 20_000; }
  function memoryReason(speaker: Speaker) { return speaker.shared_room ? 'Shared-room voices cannot be remembered.' : !refined ? 'Refine with speaker models enabled before remembering this voice.' : cleanSpeech(speaker) < 20_000 ? 'At least 20 seconds of clean, non-overlapping speech is needed.' : unavailable ? 'Saved audio is required to enroll a voice.' : 'Review the speaker and obtain separate consent before enrolling.'; }
  async function enrolled() { remembering = ''; await refresh(); notice = 'Encrypted voiceprint saved with consent. Automatic recognition remains unavailable.'; }
  function saveName(speaker: Speaker) {
    const name = (names[speaker.id] ?? speaker.user_name ?? '').trim();
    void mutate(`name:${speaker.id}`, 'rename_speaker', { speaker_id: speaker.id, name }, name ? 'Speaker name saved for this meeting.' : 'Automatic speaker label restored.', () => { delete names[speaker.id]; });
  }
  function saveConstraint() {
    if (!validPair || !detail?.run_id) return;
    void mutate('constraint', 'add_speaker_constraint', { run_id: detail.run_id, left_turn_id: left, right_turn_id: right, kind: relationship }, 'Speaker correction saved. Refine the transcript to apply it.', () => { left = ''; right = ''; relationship = ''; });
  }
  function removeReviewedConstraint(review: Review) {
    if (!review.constraint_id) return;
    void mutate(`remove:${review.id}`, 'revoke_speaker_constraint', { constraint_id: review.constraint_id }, 'The unresolved speaker correction was removed. You can choose new excerpts.');
  }
  function resolveReview(review: Review, dismiss = false) {
    if (!detail?.run_id) return;
    const previous = previousChoices[review.id]; const target = currentChoices[review.id];
    if (!dismiss && (!review.previous_ids.includes(previous) || !reviewTargets(review).some((speaker) => speaker.id === target))) return;
    void mutate(`review:${review.id}`, 'resolve_speaker_review', { run_id: detail.run_id, review_id: review.id, ...(dismiss ? {} : { previous_id: previous, target_id: target }) }, dismiss ? 'Speaker review dismissed.' : 'Selected name applied to this speaker.');
  }
</script>

<section class="speakers-panel" aria-label="Meeting speakers" aria-busy={Boolean(busy)}>
  <div class="speakers-heading"><div><h2>Speakers</h2><p class="field-help">Review voices and keep names consistent within this meeting.</p></div>{#if !turns.length}<button class="btn btn-sm btn-outline-primary" onclick={onSetup}>Set up speaker labels</button>{/if}</div>
  <p class="privacy-note"><Icon name="shield" size={14} />Meeting names stay in this meeting. Saving a voiceprint requires a separate, explicit enrollment with consent.</p>
  {#if historical}<div class="message-banner caution-banner" role="status"><p>This historical transcript is read only. Return to the current version to edit speakers.</p></div>{:else if detail && !editable}<p class="field-help">Speaker edits become available when the current transcript finishes processing.</p>{/if}
  {#if playbackBlocked}<div class="message-banner caution-banner" role="status"><p>{unavailable ? 'Saved audio excerpts are unavailable. Meeting labels and text remain available.' : 'Audio excerpts are disabled while any recording is active, including while paused or stopping.'}</p></div>{/if}
  {#if detail?.diarization && !detail.diarization.calibrated}<div class="message-banner caution-banner" role="status"><p><strong>Experimental speaker labels</strong> · Automatic labels may need correction. Listen to the audio before naming or combining voices.{#if provisional} Some voices do not yet have enough clean speech for a stable label.{/if}</p></div>{:else if provisional}<p class="field-help">Some speaker labels are provisional and may change after refinement.</p>{/if}
  {#if error}<div class="message-banner error-banner" role="alert"><p>{error}</p></div>{/if}
  {#if notice}<p class="save-notice" role="status">{notice}</p>{/if}

    {#if constraints.length}<section class="saved-speaker-corrections" aria-label="Saved speaker corrections"><div class="turn-heading"><h3>Saved speaker corrections</h3><span>{constraints.length} active</span></div><p class="field-help">Remove an incorrect relationship before refining again. Earlier processing history is preserved.</p>
      <ul class="constraint-list">{#each visibleConstraints as constraint (constraint.id)}<li data-constraint-id={constraint.id}><div><strong>{constraint.kind === 'must_link' ? 'Same person' : constraint.kind === 'cannot_link' ? 'Different people' : 'Speaker correction'}</strong><div class="constraint-excerpts"><button type="button" class="turn-play" disabled={playbackBlocked} onclick={() => play(constraint.left.start_ms)} aria-label={`Play first saved excerpt at ${time(constraint.left.start_ms)}`}><Icon name="play" size={13} />{trackName(constraint.left.track)} · {time(constraint.left.start_ms)}–{time(constraint.left.end_ms)}</button><span aria-hidden="true">↔</span><button type="button" class="turn-play" disabled={playbackBlocked} onclick={() => play(constraint.right.start_ms)} aria-label={`Play second saved excerpt at ${time(constraint.right.start_ms)}`}><Icon name="play" size={13} />{trackName(constraint.right.track)} · {time(constraint.right.start_ms)}–{time(constraint.right.end_ms)}</button></div></div><button type="button" class="btn btn-sm btn-outline-secondary" disabled={!editable || Boolean(busy)} onclick={() => void mutate(`remove:${constraint.id}`, 'revoke_speaker_constraint', { constraint_id: constraint.id }, 'Speaker correction removed. Refine the transcript to update labels.')}>{busy === `remove:${constraint.id}` ? 'Removing…' : 'Remove speaker correction'}</button></li>{/each}</ul>
      {#if constraintPages > 1}<nav class="turn-pagination" aria-label="Saved speaker correction pages"><button class="btn btn-sm btn-outline-secondary" disabled={correctionPage === 0} onclick={() => { constraintPage = correctionPage - 1; }}>Previous corrections</button><span>Page {correctionPage + 1} of {constraintPages}</span><button class="btn btn-sm btn-outline-secondary" disabled={correctionPage + 1 >= constraintPages} onclick={() => { constraintPage = correctionPage + 1; }}>Next corrections</button></nav>{/if}
    </section>{/if}


  {#if constraintReviews.length}<section class="speaker-reviews" aria-label="Speaker corrections needing review"><h3>Speaker corrections need new excerpts</h3><p class="field-help">The transcript was refined. These speaker relationships could not be aligned safely. Remove the affected correction, refine again, then choose new clean excerpts if needed.</p>{#each constraintReviews as review (review.id)}<article class="speaker-review" aria-label={`Speaker correction review ${review.id}`}><p>{review.reason}</p>{#if review.constraint_id}<button type="button" class="btn btn-sm btn-outline-secondary" disabled={!editable || Boolean(busy)} onclick={() => removeReviewedConstraint(review)}>Remove affected speaker correction</button>{:else}<p class="field-help">Review the saved speaker corrections above. Remove the affected relationship and refine again before choosing new excerpts.</p>{/if}</article>{/each}</section>{/if}

  {#if !detail}<div class="content-empty"><h3>Select a meeting</h3><p>Speaker turns and meeting names will appear here.</p></div>
  {:else if !turns.length}<div class="content-empty"><Icon name="waveform" size={30} /><h3>Find the voices in this meeting</h3><p>Set up local speaker models in Settings, then choose <strong>Refine transcript</strong> to create speaker turns. Existing transcript text stays available.</p></div>
  {:else}
    {#if speakers.length}<section class="speaker-roster" aria-label="Speaker roster"><h3>Voices in this meeting</h3><div class="roster-grid">
      {#each speakers as speaker (speaker.id)}
        <article class="speaker-card" aria-label={speaker.user_name || speaker.label}>
          <div class="speaker-card-heading"><strong>{speaker.user_name || speaker.label}</strong>{#if speaker.provisional}<span class="speaker-tag">Provisional</span>{/if}</div>
          <p class="speaker-meta">{trackName(speaker.track)} · {time(speaker.speech_ms)} of speech</p>
          <form onsubmit={(event) => { event.preventDefault(); saveName(speaker); }}>
            <label class="speaker-name-label">Meeting name<input class="form-control form-control-sm" aria-label={`Name for ${speaker.label}`} placeholder={speaker.label} maxlength="120" value={names[speaker.id] ?? speaker.user_name ?? ''} oninput={(event) => { names[speaker.id] = event.currentTarget.value; }} disabled={!editable || Boolean(busy)} /></label>
            <button class="btn btn-sm btn-outline-primary" type="submit" disabled={!editable || Boolean(busy) || (names[speaker.id] ?? speaker.user_name ?? '').trim() === (speaker.user_name ?? '')}>{busy === `name:${speaker.id}` ? 'Saving…' : 'Save name'}</button>
          </form><p class="field-help">Clear the name to restore the automatic label.</p>
          <label class="shared-room-choice"><input type="checkbox" checked={speaker.shared_room ?? false} disabled={!editable || Boolean(busy)} onchange={(event) => void mutate(`shared:${speaker.id}`, 'set_speaker_shared_room', { speaker_id: speaker.id, shared_room: event.currentTarget.checked }, 'Shared-room preference saved.')} /><span>Shared room with multiple voices</span></label>
          <button class="btn btn-sm btn-outline-secondary remember-button" disabled={!canRemember(speaker) || Boolean(busy)} onclick={() => { remembering = speaker.id; error = ''; notice = ''; }}>Remember this voice</button><p class="field-help">{memoryReason(speaker)}</p>
        </article>
      {/each}
    </div></section>{/if}

    {#if detail && rememberedSpeaker}{#key `${detail.meeting.id}|${detail.run_id}|${rememberedSpeaker.id}`}<RememberVoice {detail} speaker={rememberedSpeaker} {pageGeneration} allowed={canRemember(rememberedSpeaker)} onSaved={enrolled} onCancel={() => { remembering = ''; }} {onSetup} />{/key}{/if}

    {#if reviews.length}<section class="speaker-reviews" aria-label="Speaker name reviews"><h3>Names needing review <span>({reviews.length})</span></h3><p class="field-help">A voice changed during refinement. Choose where an earlier name belongs, or leave it unassigned.</p>
      {#each reviews as review (review.id)}
        <article class="speaker-review" aria-label={`Speaker review ${review.id}`}>
          <h4>{review.kind.replaceAll('_', ' ')}</h4><p>{review.reason}</p>
          <div class="review-choices"><label>Earlier name<select class="form-select form-select-sm" aria-label={`Previous name for review ${review.id}`} value={previousChoices[review.id] ?? ''} onchange={(event) => { previousChoices[review.id] = event.currentTarget.value; }} disabled={!editable || Boolean(busy)}><option value="">Choose an earlier name</option>{#each review.previous_ids as previous, index}<option value={previous}>{review.previous_labels?.[index] || `Earlier speaker ${index + 1}`}</option>{/each}</select></label>
          <label>Current voice<select class="form-select form-select-sm" aria-label={`Current speaker for review ${review.id}`} value={currentChoices[review.id] ?? ''} onchange={(event) => { currentChoices[review.id] = event.currentTarget.value; }} disabled={!editable || Boolean(busy)}><option value="">Choose a current voice</option>{#each reviewTargets(review) as target}<option value={target.id}>{target.user_name || target.label} · {trackName(target.track)}</option>{/each}</select></label></div>
          {#if !reviewTargets(review).length}<p class="field-help">No current voice can be matched to this review. Keep it for later review or dismiss it.</p>{/if}
          <div class="speaker-actions"><button type="button" class="btn btn-sm btn-outline-primary" disabled={!editable || Boolean(busy) || !review.previous_ids.includes(previousChoices[review.id]) || !reviewTargets(review).some((speaker) => speaker.id === currentChoices[review.id])} onclick={() => resolveReview(review)}>Keep selected name</button><button type="button" class="btn btn-sm btn-outline-secondary" disabled={!editable || Boolean(busy)} onclick={() => resolveReview(review, true)}>Dismiss review</button></div>
        </article>
      {/each}
    </section>{/if}

    <section class="speaker-corrections" aria-label="Speaker corrections"><h3>Correct a speaker relationship</h3><p class="field-help">Choose two clean excerpts from the same audio track. Your correction will guide the next refinement.</p>
      {#if cleanTurns.length < 2}<p class="field-help">Two clear excerpts of at least one second are needed. Short, overlapping and uncertain turns are excluded.</p>{/if}
      <form onsubmit={(event) => { event.preventDefault(); saveConstraint(); }}><div class="constraint-grid">
        <label>First speaker excerpt<select class="form-select form-select-sm" value={left} onchange={(event) => chooseLeft(event.currentTarget.value)} disabled={!editable || Boolean(busy) || !cleanTurns.length}><option value="">Choose the first excerpt</option>{#if leftTurn && !excerptChoices.some((turn) => turn.id === leftTurn?.id)}<option value={leftTurn.id}>{excerptName(leftTurn)}</option>{/if}{#each excerptChoices as turn}<option value={turn.id}>{excerptName(turn)}</option>{/each}</select></label>
        <label>Second speaker excerpt<select class="form-select form-select-sm" bind:value={right} disabled={!editable || Boolean(busy) || !leftTurn}><option value="">Choose another excerpt on the same track</option>{#if rightTurn && !excerptChoices.some((turn) => turn.id === rightTurn?.id)}<option value={rightTurn.id}>{excerptName(rightTurn)}</option>{/if}{#each excerptChoices.filter((turn) => turn.id !== left && (!leftTurn || turn.track === leftTurn.track)) as turn}<option value={turn.id}>{excerptName(turn)}</option>{/each}</select></label>
        <label>Speaker relationship<select class="form-select form-select-sm" bind:value={relationship} disabled={!editable || Boolean(busy)}><option value="">Choose a relationship</option><option value="must_link">Same person</option><option value="cannot_link">Different people</option></select></label>
      </div><div class="speaker-actions"><button class="btn btn-sm btn-outline-primary" type="submit" disabled={!editable || Boolean(busy) || !validPair}>{busy === 'constraint' ? 'Saving…' : 'Save speaker correction'}</button>{#if leftTurn}<button type="button" class="btn btn-sm btn-outline-secondary" disabled={playbackBlocked} onclick={() => play(leftTurn.start_ms)}>Listen to first excerpt</button>{/if}{#if rightTurn}<button type="button" class="btn btn-sm btn-outline-secondary" disabled={playbackBlocked} onclick={() => play(rightTurn.start_ms)}>Listen to second excerpt</button>{/if}</div></form>
      {#if excerptPages > 1}<nav class="turn-pagination" aria-label="Excerpt choices"><button class="btn btn-sm btn-outline-secondary" disabled={choicePage === 0} onclick={() => { excerptPage = choicePage - 1; }}>Previous excerpt choices</button><span>{choicePage + 1} / {excerptPages}</span><button class="btn btn-sm btn-outline-secondary" disabled={choicePage + 1 >= excerptPages} onclick={() => { excerptPage = choicePage + 1; }}>Next excerpt choices</button></nav>{/if}
    </section>

    <section class="speaker-turns" aria-label="Speaker turns"><div class="turn-heading"><h3>Speaker turns</h3><span>{turns.length} total</span></div><p class="field-help">Short and unclear turns remain unknown. Overlapping voices are shown separately from clean speech.</p>
      <ol class="turn-list">{#each visibleTurns as turn (turn.id)}<li class:overlap-turn={turn.overlap}><button class="turn-play" type="button" aria-label={`Play speaker excerpt at ${time(turn.start_ms)}`} title={playbackBlocked ? 'Playback is disabled during recording' : `Play ${trackName(turn.track)} excerpt`} disabled={playbackBlocked} onclick={() => play(turn.start_ms)}><Icon name="play" size={14} /><span>{time(turn.start_ms)}–{time(turn.end_ms)}</span></button><div class="turn-description"><strong>{turnName(turn)}</strong><span>{trackName(turn.track)}{#if turn.provisional} · Provisional{:else if turn.uncertain && !turn.overlap} · Uncertain{/if}</span></div>{#if turn.overlap}<span class="speaker-tag">Overlap</span>{/if}</li>{/each}</ol>
      {#if totalPages > 1}<nav class="turn-pagination" aria-label="Speaker turn pages"><button class="btn btn-sm btn-outline-secondary" disabled={currentPage === 0} onclick={() => { page = currentPage - 1; }}>Previous turns</button><span>Page {currentPage + 1} of {totalPages}</span><button class="btn btn-sm btn-outline-secondary" disabled={currentPage + 1 >= totalPages} onclick={() => { page = currentPage + 1; }}>Next turns</button></nav>{/if}
    </section>
  {/if}
  {#if detail?.diarization?.warnings.length}<details class="speaker-processing-details"><summary>Processing notes</summary>{#each detail.diarization.warnings as warning}<p>{warning}</p>{/each}</details>{/if}
</section>

<style>
  .shared-room-choice{flex-direction:row!important;align-items:flex-start;line-height:1.8;margin:14px 0}.shared-room-choice input{margin-top:4px;flex-shrink:0}.remember-button{margin-top:4px}.speakers-panel{padding:24px 0;color:var(--ink);font-size:12px}.speakers-heading{display:flex;justify-content:space-between;align-items:flex-start;gap:18px}.speakers-heading h2{font-size:16px;margin:0 0 6px}.speakers-heading>.btn{flex-shrink:0}.privacy-note{display:flex;align-items:center;gap:7px;color:var(--muted);font-size:11px;margin:16px 0;line-height:1.7}.speaker-roster,.speaker-reviews,.speaker-corrections,.saved-speaker-corrections,.speaker-turns{margin-top:28px}.speakers-panel h3{font-size:13px;font-weight:600;margin:0 0 12px}.speakers-panel h4{font-size:12px;text-transform:capitalize;margin:0 0 8px}.roster-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(240px,1fr));gap:12px}.speaker-card{padding:16px;border:1px solid var(--border);background:var(--surface);border-radius:7px;min-width:0}.speaker-card-heading{display:flex;gap:10px;align-items:center;justify-content:space-between}.speaker-card-heading strong{overflow-wrap:anywhere}.speaker-meta{color:var(--muted);font-size:11px;margin:7px 0 13px}.speaker-card form{display:flex;gap:8px;align-items:flex-end}.speaker-name-label{flex:1;min-width:0}.speaker-card form>.btn{flex-shrink:0}.speakers-panel label{font-size:11px;display:flex;flex-direction:column;gap:7px}.speakers-panel .form-control,.speakers-panel .form-select{font-size:12px;color:var(--ink);background-color:var(--surface);border-color:var(--border);min-height:34px}.speaker-card>.field-help{margin:9px 0 0}.speaker-tag{display:inline-flex;align-items:center;border:1px solid var(--border);border-radius:4px;padding:2px 6px;font-size:10px;white-space:nowrap;color:var(--muted);background:var(--soft)}.speaker-review{margin-top:12px;border:1px solid var(--border);border-radius:7px;padding:16px;background:var(--soft)}.speaker-review>p{line-height:1.8;overflow-wrap:anywhere}.review-choices{display:grid;grid-template-columns:1fr 1fr;gap:14px}.speaker-actions{display:flex;gap:9px;align-items:center;flex-wrap:wrap;margin-top:14px}.constraint-grid{display:grid;grid-template-columns:minmax(0,1fr) minmax(0,1fr);gap:14px}.constraint-grid>label:last-child{max-width:260px}.speaker-corrections{padding-top:23px;border-top:1px solid var(--border)}.constraint-list{list-style:none;padding:0;margin:12px 0}.constraint-list>li{display:flex;justify-content:space-between;gap:15px;align-items:center;padding:14px 0;border-bottom:1px solid var(--border)}.constraint-list>li>div{min-width:0}.constraint-list strong{font-size:12px;font-weight:500}.constraint-list>li>.btn{flex-shrink:0}.constraint-excerpts{display:flex;gap:10px;align-items:center;flex-wrap:wrap;margin-top:6px}.constraint-excerpts>span{color:var(--muted)}.turn-heading{display:flex;justify-content:space-between;align-items:baseline;gap:12px}.turn-heading>span{font-size:11px;color:var(--muted)}.turn-list{list-style:none;padding:0;margin:14px 0}.turn-list>li{display:flex;align-items:center;gap:16px;border-bottom:1px solid var(--border);padding:12px 0}.turn-play{display:inline-flex;gap:7px;align-items:center;flex-shrink:0;border:0;background:none;color:var(--blue);font-size:11px;font-variant-numeric:tabular-nums;padding:4px 0}.turn-play:hover:not(:disabled){text-decoration:underline}.turn-play:disabled{opacity:.55;color:var(--muted)}.turn-description{display:flex;flex-direction:column;gap:4px;flex:1;min-width:0}.turn-description strong{font-size:12px;font-weight:500;overflow-wrap:anywhere}.turn-description>span{font-size:10px;color:var(--muted)}.turn-pagination{display:flex;gap:12px;align-items:center;justify-content:flex-end;margin-top:16px}.turn-pagination>span{font-size:11px;color:var(--muted);font-variant-numeric:tabular-nums}.speaker-processing-details{margin-top:24px;color:var(--muted);font-size:11px}.speaker-processing-details summary{cursor:pointer}.speaker-processing-details p{margin:12px 0 0;line-height:1.8;overflow-wrap:anywhere}.save-notice{color:var(--blue);font-size:11px;margin:14px 0}.content-empty :global(svg){color:var(--blue);margin-bottom:10px}.content-empty p{line-height:1.9;max-width:490px;margin-inline:auto}@media(max-width:850px){.constraint-grid,.review-choices{grid-template-columns:minmax(0,1fr)}.speaker-card form{flex-wrap:wrap}.constraint-list>li{align-items:flex-start;flex-direction:column}.turn-list>li{gap:10px}.turn-play{font-size:10px}.speakers-heading{flex-wrap:wrap}.turn-pagination{justify-content:space-between;gap:7px}}
</style>
