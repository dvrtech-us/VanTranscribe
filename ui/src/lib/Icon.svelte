<script lang="ts">
  let { name, size = 20 }: { name: string; size?: number } = $props();
  const paths: Record<string, string> = {
    library: 'M4 4h5v16H4zM11 4h4v16h-4zM17 5l3-1 4 15-3 1z',
    settings: 'M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8zm0-5 1 3 3 1 3-1 2 3-2 3 1 3-2 3-3-1-3 1-1 3H9l-1-3-3-1-2-3 1-3-1-3 2-3 3 1 1-3z',
    mic: 'M9 5a3 3 0 0 1 6 0v7a3 3 0 0 1-6 0zM5 10v2a7 7 0 0 0 14 0v-2M12 19v3m-4 0h8',
    monitor: 'M3 4h18v13H3zM8 22h8m-4-5v5',
    plus: 'M12 5v14M5 12h14',
    search: 'M10.5 3a7.5 7.5 0 1 0 0 15 7.5 7.5 0 0 0 0-15M16 16l5 5',
    arrow: 'M9 5l7 7-7 7',
    back: 'M15 5l-7 7 7 7',
    pause: 'M8 5v14M16 5v14',
    play: 'M7 4l13 8-13 8z',
    stop: 'M6 6h12v12H6z',
    note: 'M5 3h14v18H5zM8 8h8m-8 4h8m-8 4h5',
    download: 'M12 3v12m-5-5 5 5 5-5M4 17v4h16v-4',
    check: 'M5 12l4 4L20 5',
    close: 'M6 6l12 12M6 18 18 6',
    folder: 'M3 7V4h7l2 3h9v13H3z',
    shield: 'M12 2l8 4v6c0 5-8 10-8 10S4 17 4 12V6zM8 12l3 3 5-6',
    bookmark: 'M6 3h12v19l-6-5-6 5z',
    edit: 'M4 16 16 4l4 4L8 20H4zM14 6l4 4',
    sound: 'M3 9h4l5-5v16l-5-5H3zM16 8a6 6 0 0 1 0 8m3-11a10 10 0 0 1 0 14',
    clock: 'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18m0 4v6l4 2',
    alert: 'M12 3 2 21h20zM12 9v5m0 3v1',
    external: 'M14 3h7v7m0-7L10 14M10 5H3v16h16v-7',
    waveform: 'M3 10v4m4-8v12m5-16v20m5-16v12m4-8v4',
  };
</script>

<svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d={paths[name] ?? paths.note} /></svg>
