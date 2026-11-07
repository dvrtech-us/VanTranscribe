<script lang="ts">
  import { dateLabel } from './api';
  import type { MeetingDetail, SummaryEnvelope, SummaryInfo } from './api';
  let { detail, configured, canGenerate, generating, onGenerate, onSetup, onCitation }: { detail: MeetingDetail | null; configured: boolean; canGenerate: boolean; generating: boolean; onGenerate: () => void; onSetup: () => void; onCitation: (run: string, segment: string) => void } = $props();
  let selected = $state('');
  const summaries = $derived(detail?.summaries ?? []);
  const summary = $derived(summaries.find((s) => s.id === selected) ?? summaries.find((s) => !s.is_stale && s.run_id === detail?.run_id) ?? summaries.find((s) => s.run_id === detail?.run_id) ?? summaries[0]);
  const envelope = $derived(parse(summary));
  function parse(s: SummaryInfo | undefined): SummaryEnvelope | null {
    if (!s) return null;
    try {
      const raw = JSON.parse(s.content_json);
      if (!raw?.content || typeof raw.content !== 'object') return null;
      const item = (x: unknown): x is { text: string; source_ids: string[] } => Boolean(x && typeof x === 'object' && typeof (x as any).text === 'string' && Array.isArray((x as any).source_ids) && (x as any).source_ids.every((id: unknown) => typeof id === 'string'));
      const content = raw.content;
      for (const key of ['tldr', 'key_points', 'decisions', 'open_questions']) if (!Array.isArray(content[key]) || !content[key].every(item)) return null;
      if (!Array.isArray(content.action_items) || !content.action_items.every((x: any) => x && typeof x.task === 'string' && Array.isArray(x.source_ids) && x.source_ids.every((id: unknown) => typeof id === 'string') && (x.owner == null || typeof x.owner === 'string') && (x.due == null || typeof x.due === 'string'))) return null;
      return { content, partial: raw.partial === true, errors: Array.isArray(raw.errors) ? raw.errors.filter((x: unknown) => typeof x === 'string') : [] };
    } catch { return null; }
  }
</script>

{#snippet citations(ids: string[])}
  {#if summary}<span class="source-citations">{#each ids as source, i}<button class="citation-button" title={`Open source segment ${source}`} aria-label={`Open source ${i + 1} for ${source}`} onclick={() => onCitation(summary.run_id, source)}>[{i + 1}]</button>{/each}</span>{/if}
{/snippet}
<div class="ai-notes-content">
  <div class="summary-heading"><div><h2>AI notes</h2><p class="field-help">Return to the transcript behind each point.</p></div>{#if configured}<button class="btn btn-sm btn-primary" onclick={onGenerate} disabled={!canGenerate || generating}>{generating ? 'Generating…' : summaries.length ? 'Regenerate AI notes' : 'Generate AI notes'}</button>{:else}<button class="btn btn-sm btn-outline-primary" onclick={onSetup}>Set up AI notes</button>{/if}</div>
  {#if detail?.summary_error}<div class="message-banner error-banner" role="alert"><p>AI notes could not be generated: {detail.summary_error}</p></div>{/if}
  {#if !configured}<p class="field-help">Choose an on-device or explicitly approved external provider in Settings.</p>{:else if !canGenerate && !generating}<p class="field-help">AI notes need a completed, active transcript. Stop recording or return to the current version.</p>{/if}
  {#if summaries.length > 1}<label class="summary-version-label">AI notes version<select class="form-select form-select-sm" bind:value={selected}><option value="">Latest available</option>{#each summaries as version}<option value={version.id}>{dateLabel(version.created_at)} · {version.model} {version.is_stale ? '· Stale' : ''}</option>{/each}</select></label>{/if}
  {#if summary}
    <div class="summary-provenance"><span>{summary.provider} · {summary.model}</span><time>{dateLabel(summary.created_at)}</time><span>{summary.template.replaceAll('_', ' ')}</span></div>
    {#if summary.is_stale}<div class="message-banner caution-banner stale-summary" role="status"><p><strong>Stale AI notes</strong> · Based on an earlier transcript revision. Review the cited sources or regenerate.</p></div>{/if}
    {#if envelope?.partial}<div class="message-banner caution-banner" role="status"><p><strong>Partial AI notes</strong> · Some transcript portions could not be summarized.</p></div>{/if}
    {#if envelope?.errors.length}<details class="summary-errors"><summary>Processing gaps ({envelope.errors.length})</summary>{#each envelope.errors as error}<p>{error}</p>{/each}</details>{/if}
    {#if envelope}
      {#each [{ key: 'tldr' as const, title: 'At a glance' }, { key: 'key_points' as const, title: 'Key points' }, { key: 'decisions' as const, title: 'Decisions' }] as section}
        {#if envelope.content[section.key].length}<section class="summary-section"><h3>{section.title}</h3><ul>{#each envelope.content[section.key] as point}<li>{point.text} {@render citations(point.source_ids)}</li>{/each}</ul></section>{/if}
      {/each}
      {#if envelope.content.action_items.length}<section class="summary-section"><h3>Action items</h3><ul class="action-items">{#each envelope.content.action_items as action}<li><p>{action.task} {@render citations(action.source_ids)}</p><span>Owner: {action.owner ?? 'Not stated'} · Due: {action.due ?? 'Not stated'}</span></li>{/each}</ul></section>{/if}
      {#if envelope.content.open_questions.length}<section class="summary-section"><h3>Open questions</h3><ul>{#each envelope.content.open_questions as point}<li>{point.text} {@render citations(point.source_ids)}</li>{/each}</ul></section>{/if}
      {#if !Object.values(envelope.content).some((items) => items.length)}<p class="field-help">No supported points were extracted from this transcript.</p>{/if}
    {:else}<div class="summary-content">{summary.content_md}</div>{/if}
  {:else if detail?.summary}<div class="summary-content">{detail.summary}</div>
  {:else}<div class="content-empty"><h3>A useful record of the conversation</h3><p>Generate key points, decisions, action items and open questions with transcript sources.</p></div>{/if}
</div>
