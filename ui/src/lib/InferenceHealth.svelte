<script lang="ts">
  import type { BacklogReport } from './api';
  let { report, audioAvailable }: { report?: BacklogReport | null; audioAvailable: boolean } = $props();
  const hasNotices = $derived(report && (report.stages.length || report.gaps.length || report.warnings.length));
</script>
{#if hasNotices && report}
  <section class="inference-health" aria-label="Transcript processing history">
    <strong>Live processing notices for this version</strong>
    {#each report.warnings as warning}<p>{warning}</p>{/each}
    {#if report.gaps.length}<p>{report.gaps.length} inference {report.gaps.length === 1 ? 'gap was' : 'gaps were'} recorded separately from the original audio.</p>{/if}
    {#if report.truncated}<p>The event history reached its display limit; additional inference events may have occurred.</p>{/if}
    <p>{audioAvailable ? 'Saved audio can be reprocessed to create a new transcript version.' : 'Source audio is unavailable for reprocessing. Existing transcript history remains available.'}</p>
  </section>
{/if}
<style>.inference-health{margin:14px 0;padding:14px 16px;border:1px solid var(--border);border-left:3px solid var(--red);border-radius:5px;color:var(--ink);background:var(--surface);font-size:11px;line-height:1.7}.inference-health p{margin:7px 0 0;color:var(--muted)}</style>
