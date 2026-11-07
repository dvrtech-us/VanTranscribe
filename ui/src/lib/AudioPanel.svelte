<script lang="ts">
  import { untrack } from 'svelte';
  import Icon from './Icon.svelte';
  import { command, message, time } from './api';
  import type { AudioWaveform, Snapshot, SpeakerTurn, EchoInfo } from './api';
  let { id, snapshot, blocked, refresh, turns = [], echo = null }: { id: string; snapshot: Snapshot | null; blocked: boolean; refresh: () => Promise<void>; turns?: SpeakerTurn[]; echo?: EchoInfo | null } = $props();
  let waveform = $state<AudioWaveform | null>(null);
  let loading = $state(false);
  let error = $state('');
  let busy = $state(false);
  let pendingPosition = $state<number | null>(null);
  let mic = $state(1);
  let system = $state(1);
  const current = $derived(snapshot?.playback?.meeting_id === id ? snapshot.playback : null);
  const duration = $derived(current?.duration_ms ?? waveform?.duration_ms ?? 0);
  const position = $derived(pendingPosition ?? current?.position_ms ?? 0);
  const playing = $derived(current?.status === 'playing');
  const percent = $derived(duration ? Math.min(100, position / duration * 100) : 0);
  $effect(() => {
    const meeting = id;
    const existing = untrack(() => snapshot?.playback);
    pendingPosition = null;
    mic = existing?.meeting_id === meeting ? existing.mic_volume : 1;
    system = existing?.meeting_id === meeting ? existing.system_volume : 1;
  });
  $effect(() => {
    const meeting = id;
    if (blocked) return;
    let alive = true;
    loading = true; error = ''; waveform = null;
    void command<AudioWaveform>('meeting_waveform', { id: meeting, bins: 512 }).then((value) => { if (alive) waveform = value; }).catch((cause) => { if (alive) error = message(cause); }).finally(() => { if (alive) loading = false; });
    return () => { alive = false; };
  });
  async function action(name: string, args?: Record<string, unknown>) {
    if (busy || blocked) return;
    busy = true; error = '';
    try { await command(name, args); pendingPosition = null; await refresh(); }
    catch (cause) { error = message(cause); }
    finally { busy = false; }
  }
  function seek(event: Event) {
    const value = Number((event.currentTarget as HTMLInputElement).value);
    void action('seek_meeting_audio', { id, position_ms: Math.round(value) });
  }
  function peaks(values: number[]) {
    if (!values.length) return '';
    return values.map((value, index) => { const x = index / Math.max(1, values.length - 1) * 1000; const height = Math.max(0, Math.min(1, value)) * 35; return `M${x.toFixed(2)},${(42-height).toFixed(2)}V${(42+height).toFixed(2)}`; }).join('');
  }
  function volumes() { void action('set_playback_volumes', { id, mic_volume: mic, system_volume: system }); }
  function seekWaveform(event: MouseEvent) {
    const bounds = event.currentTarget instanceof HTMLElement ? event.currentTarget.getBoundingClientRect() : null;
    if (!bounds || !duration) return;
    const target = event.detail === 0 ? position : Math.max(0, Math.min(1, (event.clientX - bounds.left) / bounds.width)) * duration;
    void action('seek_meeting_audio', { id, position_ms: Math.round(target) });
  }
  function speakerLane(track: string) {
    if (!duration) return [];
    const bins: { label: string; color: string; uncertain: boolean }[] = Array.from({ length: 512 }, () => ({ label: 'No assigned speech', color: 'transparent', uncertain: true }));
    const overlap = new Float64Array(512);
    for (const turn of turns) {
      if (turn.track !== track) continue;
      const first = Math.max(0, Math.floor(turn.start_ms / duration * bins.length));
      const last = Math.min(bins.length - 1, Math.ceil(turn.end_ms / duration * bins.length));
      const label = turn.overlap ? 'Overlap' : turn.uncertain || turn.provisional ? 'Unknown' : turn.label;
      const identity = turn.speaker_ids.join('|') || label;
      let hash = 0; for (let i = 0; i < identity.length; i++) hash = (hash * 31 + identity.charCodeAt(i)) >>> 0;
      const color = label === 'Unknown' || label === 'Overlap' ? 'var(--muted)' : `hsl(${180 + hash % 130} 27% 55%)`;
      for (let i = first; i <= last; i++) {
        const covered = Math.min(turn.end_ms, (i+1) * duration / bins.length) - Math.max(turn.start_ms, i * duration / bins.length);
        if (covered > overlap[i]) { overlap[i] = covered; bins[i] = { label, color, uncertain: turn.uncertain || turn.provisional || turn.overlap }; }
      }
    }
    return bins;
  }
</script>

<div class="saved-audio-panel">
  <div class="audio-heading"><div><h2>Meeting audio</h2><p class="field-help">Listen to the original microphone and system tracks.</p></div><span class="field-help">{time(duration, true)}</span></div>
  {#if blocked}<div class="message-banner caution-banner" role="status"><Icon name="shield" /><p>Playback is disabled while any recording is active, including while paused or stopping.</p></div>{/if}
  {#if loading}<p class="field-help" role="status">Reading audio waveforms…</p>{/if}
  {#if error || current?.error}<div class="message-banner error-banner" role="alert"><p>{error || current?.error}</p></div>{/if}
  <div class="playback-controls"><button class="btn btn-primary" disabled={blocked || busy || !duration} onclick={() => void action(playing ? 'pause_meeting_audio' : 'play_meeting_audio', playing ? {} : { id, position_ms: Math.round(position) })}><Icon name={playing ? 'pause' : 'play'} size={17} />{playing ? 'Pause playback' : 'Play audio'}</button><button class="btn btn-outline-secondary" disabled={blocked || busy || !current} onclick={() => void action('stop_meeting_audio')}>Stop playback</button><span class="playback-clock" aria-live="off">{time(position, true)} / {time(duration, true)}</span></div>
  <div class="audio-waveforms">
    {#each [{ key: 'mic' as const, title: 'Microphone', icon: 'mic', samples: waveform?.mic ?? [] }, { key: 'system' as const, title: 'System audio', icon: 'monitor', samples: waveform?.system ?? [] }] as track}
      <div class="audio-track" class:system-track={track.key === 'system'}><div class="audio-track-label"><Icon name={track.icon} size={15} /><strong>{track.title}</strong></div><button type="button" class="waveform-drawing" aria-label={`Seek in ${track.title.toLowerCase()} waveform`} disabled={blocked || busy || !duration} onclick={seekWaveform}><svg viewBox="0 0 1000 84" preserveAspectRatio="none" role="img" aria-label={`${track.title} waveform`}><path d="M0,42H1000" class="waveform-axis" /><path d={peaks(track.samples)} class="waveform-peaks" /></svg><span class="playback-head" style:left={`${percent}%`}></span></button>{#if turns.some((turn) => turn.track === track.key)}<svg class="speaker-audio-lane" viewBox="0 0 512 16" preserveAspectRatio="none" role="img" aria-label={`${track.title} speaker lane`}>{#each speakerLane(track.key) as bin, index}<rect x={index} y="2" width="1.1" height="12" fill={bin.color} opacity={bin.uncertain ? 0.45 : 0.85}><title>{bin.label} · {time(index/512*duration)}</title></rect>{/each}</svg><p class="field-help">Speaker lane · uncertain or overlapping speech is muted. Open Speakers for exact excerpts.</p>{/if}</div>
    {/each}
    <label class="audio-seek-label">Playback position<input class="form-range" type="range" min="0" max={duration} step="100" value={position} oninput={(event) => { pendingPosition = Number(event.currentTarget.value); }} onchange={seek} disabled={blocked || busy || !duration} aria-label="Playback position" aria-valuetext={time(position, true)} /></label>
  </div>
  <div class="track-volumes"><label><span>Microphone volume · {Math.round(mic*100)}%</span><input class="form-range" type="range" min="0" max="1" step="0.05" bind:value={mic} onchange={volumes} disabled={blocked || busy || !duration} /></label><label><span>System volume · {Math.round(system*100)}%</span><input class="form-range" type="range" min="0" max="1" step="0.05" bind:value={system} onchange={volumes} disabled={blocked || busy || !duration} /></label></div>
  {#if echo}<section class="mt-3" aria-label="Transcript echo processing"><h3>Echo processing for this transcript</h3><p class="field-help">Applied to {time(echo.applied_mic_ms)} of microphone audio; skipped for {time(echo.bypassed_mic_ms)}. Playback uses the original recording.</p>{#each echo.warnings.slice(0, 3) as warning}<p class="field-help">{warning}</p>{/each}</section>{/if}
  <p class="field-help">A volume of zero mutes that track. Recorded gaps remain silent. Playback does not change the stored audio.</p>
</div>
