use super::*;
use vt_playback::Playback;

fn ensure_idle(service: &AppService, runtime: &Runtime) -> Result<()> {
    if service.shutting_down.load(Ordering::Relaxed) {
        bail!("VanTranscribe is closing.");
    }
    AppService::ensure_not_maintenance(runtime)?;
    if runtime.current.as_ref().is_some_and(|current| {
        matches!(
            current.view.status.as_str(),
            "recording" | "paused" | "processing"
        )
    }) {
        bail!("Playback is disabled while any recording is active, paused, or stopping.");
    }
    Ok(())
}

fn with_audio(
    service: &AppService,
    id: &str,
    action: impl FnOnce(&mut Playback) -> Result<()>,
) -> Result<()> {
    // Keep runtime reserved until the output command completes. Recording start
    // takes these same locks in this order, then drops playback before capture.
    let runtime = service
        .runtime
        .lock()
        .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
    ensure_idle(service, &runtime)?;
    service.ensure_saved_audio(id)?;
    let mut loaded = service
        .playback
        .lock()
        .map_err(|_| anyhow::anyhow!("playback state unavailable"))?;
    if loaded
        .as_ref()
        .is_none_or(|playback| playback.snapshot().meeting_id != id)
    {
        if let Some(previous) = loaded.take() {
            previous.stop()?;
        }
        *loaded = Some(Playback::open(&service.root, id)?);
    }
    action(loaded.as_mut().expect("loaded playback"))
}

pub(super) fn play(service: &AppService, id: &str, position: Option<u64>) -> Result<()> {
    with_audio(service, id, |playback| {
        if let Some(position) = position {
            playback.seek(position)?;
        }
        playback.play()?;
        Ok(())
    })
}

pub(super) fn seek(service: &AppService, id: &str, position: u64) -> Result<()> {
    with_audio(service, id, |playback| {
        playback.seek(position)?;
        Ok(())
    })
}

pub(super) fn volumes(service: &AppService, id: &str, mic: f32, system: f32) -> Result<()> {
    with_audio(service, id, |playback| {
        playback.set_volumes(mic, system)?;
        Ok(())
    })
}

pub(super) fn pause(service: &AppService) -> Result<()> {
    let runtime = service
        .runtime
        .lock()
        .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
    ensure_idle(service, &runtime)?;
    let mut playback = service
        .playback
        .lock()
        .map_err(|_| anyhow::anyhow!("playback state unavailable"))?;
    if let Some(playback) = playback.as_mut() {
        playback.pause()?;
    }
    Ok(())
}

pub(super) fn stop(service: &AppService) -> Result<()> {
    // Stopping output is always allowed, including during shutdown.
    if let Some(playback) = service
        .playback
        .lock()
        .map_err(|_| anyhow::anyhow!("playback state unavailable"))?
        .take()
    {
        playback.stop()?;
    }
    Ok(())
}

struct SavedReaderLease<'a>(&'a AppService);
impl Drop for SavedReaderLease<'_> {
    fn drop(&mut self) {
        // The reader runs without this lock, so a decoding panic never poisons
        // Runtime. Recover an existing poison only to release our own count.
        let mut runtime = self
            .0
            .runtime
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        runtime.saved_audio_readers = runtime.saved_audio_readers.saturating_sub(1);
    }
}
pub(super) fn with_saved_audio_reader<T>(
    service: &AppService,
    id: &str,
    read: impl FnOnce() -> Result<T>,
) -> Result<T> {
    {
        let mut runtime = service
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
        ensure_idle(service, &runtime)?;
        service.ensure_saved_audio(id)?;
        runtime.saved_audio_readers = runtime
            .saved_audio_readers
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Too many saved audio readers"))?;
    }
    let _lease = SavedReaderLease(service);
    read()
}
pub(super) fn waveform(
    service: &AppService,
    id: &str,
    bins: usize,
) -> Result<vt_playback::Waveform> {
    // A reader lease protects source files from retention without holding the
    // runtime mutex across a long waveform scan or blocking snapshot/control.
    with_saved_audio_reader(service, id, || {
        Ok(vt_playback::waveform(&service.root, id, bins)?)
    })
}
