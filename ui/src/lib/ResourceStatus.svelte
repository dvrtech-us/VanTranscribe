<script lang="ts">
  import { bytesLabel, time } from './api';
  import type { Snapshot } from './api';
  let { snapshot }: { snapshot: Snapshot | null } = $props();
  const health = $derived(snapshot?.health);
  const backlog = $derived(snapshot?.live_processing?.backlog);
  const overloaded = $derived(Boolean(backlog?.warning));
  const clock = $derived(snapshot?.recording ? snapshot.source_timing : null);
</script>
{#if health || clock || overloaded}
  <section class="resource-status" aria-label="Recording health">
    {#if health}<span><strong>Recording disk</strong> · {health.free_bytes == null ? 'Space not verified' : `${bytesLabel(health.free_bytes)} available`}</span>{/if}
    {#if clock}<span><strong>Source clock</strong> · {clock.synchronized ? `Synchronized · ${time(clock.elapsed_ms)}` : 'Approximate or unavailable'}</span>{/if}
    {#if health?.warning}<p class:caution={health.disk_state !== 'healthy'} role="status">{health.warning}</p>{/if}
    {#if overloaded}<p class="caution" role="status"><strong>{backlog?.stage === 'normal' ? 'Live processing notice' : 'Live processing reduced'}</strong> · {backlog?.warning ?? 'Inference is behind the source audio.'} {snapshot?.recording?.status === 'recording' ? 'Original audio continues to be recorded.' : 'Original audio is retained.'}</p>{/if}
    {#if clock?.warning}<p role="status">{clock.warning}</p>{/if}
  </section>
{/if}
<style>.resource-status{display:flex;align-items:center;gap:10px 22px;flex-wrap:wrap;border-bottom:1px solid var(--border);padding:0 0 12px;margin-bottom:16px;color:var(--muted);font-size:11px}.resource-status strong{font-weight:500;color:var(--ink)}.resource-status p{width:100%;margin:0;line-height:1.6}.caution{color:var(--ink);border-left:3px solid var(--red);padding-left:10px}</style>
