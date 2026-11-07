use super::*;

#[derive(Clone, Copy)]
pub(super) enum Pack {
    Vad,
    Diarization,
}

impl Pack {
    fn status(self, runtime: &mut Runtime) -> &mut ModelStatus {
        match self {
            Self::Vad => &mut runtime.vad_model,
            Self::Diarization => &mut runtime.diarization_model,
        }
    }
    fn cancel(self, service: &AppService) -> Arc<AtomicBool> {
        match self {
            Self::Vad => service.cancel_vad_download.clone(),
            Self::Diarization => service.cancel_diarization_download.clone(),
        }
    }
}

pub(super) fn install(service: Arc<AppService>, pack: Pack, acknowledged: bool) -> Result<()> {
    if matches!(pack, Pack::Diarization) && !acknowledged {
        bail!("Read and acknowledge the speaker model source and license notice first.");
    }
    if service.shutting_down.load(Ordering::Relaxed) {
        bail!("VanTranscribe is closing.");
    }
    {
        let mut runtime = service
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
        let status = pack.status(&mut runtime);
        if status.busy {
            bail!("This model setup is already running.");
        }
        status.busy = true;
        status.progress = 0.0;
        status.error = None;
        pack.cancel(&service).store(false, Ordering::Relaxed);
    }
    let cleanup = service.clone();
    let spawn = std::thread::Builder::new()
        .name(
            match pack {
                Pack::Vad => "live-vad-setup",
                Pack::Diarization => "speaker-model-setup",
            }
            .into(),
        )
        .spawn(move || {
            let update = service.clone();
            let progress = move |received: u64, total: u64| {
                if let Ok(mut runtime) = update.runtime.lock() {
                    pack.status(&mut runtime).progress = if total > 0 {
                        received as f64 / total as f64
                    } else {
                        0.0
                    };
                }
            };
            let outcome: Result<()> =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match pack {
                    Pack::Vad => vt_stt::VadModelManager::new(service.root.join("models"))
                        .install(progress, pack.cancel(&service))
                        .map_err(Into::into),
                    Pack::Diarization => vt_diarize::ModelManager::new(service.root.join("models"))
                        .install(progress, pack.cancel(&service), acknowledged)
                        .map_err(Into::into),
                }))
                .unwrap_or_else(|_| {
                    Err(anyhow::anyhow!(
                        "Model setup stopped unexpectedly. Retry setup."
                    ))
                });
            if let Ok(mut runtime) = service.runtime.lock() {
                let status = pack.status(&mut runtime);
                status.busy = false;
                match outcome {
                    Ok(()) => {
                        status.installed = true;
                        status.progress = 1.0;
                    }
                    Err(error) => status.error = Some(error.to_string()),
                }
            }
        });
    if let Err(error) = spawn {
        if let Ok(mut runtime) = cleanup.runtime.lock() {
            let status = pack.status(&mut runtime);
            status.busy = false;
            status.error = Some(error.to_string());
        }
        return Err(error.into());
    }
    Ok(())
}
