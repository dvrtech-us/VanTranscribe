<script lang="ts">
  import type { DetectionPrompt, Snapshot } from './api';
  let { snapshot, blocked, onRecord, onDismiss, onDismissEnded, onStop }: { snapshot: Snapshot | null; blocked: boolean; onRecord: (prompt: DetectionPrompt) => void; onDismiss: (episode: string, suppress: boolean) => void; onDismissEnded: (episode: string) => void; onStop: () => void } = $props();
  const detection = $derived(snapshot?.meeting_detection);
  const canPrompt = $derived(detection?.enabled && detection.status === 'watching' && snapshot?.settings.capture_mode === 'remote' && !snapshot?.recording && !snapshot?.processing);
</script>
{#if detection?.enabled && detection.error && snapshot?.settings.capture_mode === 'remote'}
  <section class="message-banner caution-banner" role="status" aria-label="Meeting detection status"><div><strong>Meeting detection needs attention</strong><p>{detection.error} You can still start a recording yourself.</p></div></section>
{/if}
{#if canPrompt}
  {#each detection?.prompts ?? [] as prompt (prompt.episode_id)}
    <section class="message-banner detection-notice" role="status" aria-label={`${prompt.app.name} microphone reminder`}>
      <div><strong>{prompt.app.kind === 'browser' ? `${prompt.app.name} is using the microphone` : `Meeting detected in ${prompt.app.name}`}</strong><p>{prompt.app.kind === 'browser' ? 'This could be a call, dictation or a voice memo. Microphone use does not confirm a meeting.' : 'Would you like to record this conversation locally?'}</p></div>
      <div class="notice-actions"><button class="btn btn-sm btn-outline-primary" disabled={blocked} onclick={() => onRecord(prompt)}>Review recording setup</button><button class="text-button" disabled={blocked} onclick={() => onDismiss(prompt.episode_id, false)}>Not now</button><button class="text-button" disabled={blocked} onclick={() => onDismiss(prompt.episode_id, true)}>Suppress {prompt.app.name}</button></div>
    </section>
  {/each}
{/if}
{#if detection?.enabled && detection.ended && snapshot?.recording?.id === detection.ended.meeting_id && ['recording', 'paused'].includes(snapshot.recording.status) && snapshot.settings.capture_mode === 'remote'}
  <section class="message-banner detection-notice" role="status" aria-label="Meeting may have ended">
    <div><strong>Meeting may have ended</strong><p>The source app has released the microphone for more than 60 seconds. Recording continues until you stop it.</p></div>
    <div class="notice-actions"><button class="btn btn-sm btn-outline-primary" disabled={blocked} onclick={onStop}>Stop recording</button><button class="text-button" disabled={blocked} onclick={() => { if (detection?.ended) onDismissEnded(detection.ended.episode_id); }}>Keep recording</button></div>
  </section>
{/if}
<style>.detection-notice{background:var(--soft);border:1px solid var(--border);align-items:center}.detection-notice p{margin:5px 0 0;line-height:1.6}.notice-actions{display:flex;align-items:center;gap:12px;flex-wrap:wrap;justify-content:flex-end;flex-shrink:0}@media(max-width:900px){.detection-notice{flex-wrap:wrap}.notice-actions{justify-content:flex-start}}</style>
