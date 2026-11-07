use crate::{
    AudioRenderer, AudioSink, AudioSinkFactory, OpenedSink, OutputFormat, PlaybackError, Result,
};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::time::Duration;

pub(crate) struct CpalFactory;
struct CpalSink(cpal::Stream);
impl AudioSink for CpalSink {
    fn play(&self) -> Result<()> {
        self.0
            .play()
            .map_err(|error| PlaybackError::Output(error.to_string()))
    }
}
impl AudioSinkFactory for CpalFactory {
    fn open(&mut self, renderer: AudioRenderer) -> Result<OpenedSink> {
        // Resolve the default device again for every explicit play/resume/seek.
        // A removed or changed route yields a visible error, never a fake clock.
        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| {
                PlaybackError::Output("No default audio output device is available.".into())
            })?;
        let supported = device
            .default_output_config()
            .map_err(|error| PlaybackError::Output(error.to_string()))?;
        let format = OutputFormat {
            sample_rate: supported.sample_rate(),
            channels: supported.channels(),
        };
        format.validate()?;
        let config = supported.config();
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(&device, config, renderer),
            cpal::SampleFormat::F64 => build::<f64>(&device, config, renderer),
            cpal::SampleFormat::I8 => build::<i8>(&device, config, renderer),
            cpal::SampleFormat::I16 => build::<i16>(&device, config, renderer),
            cpal::SampleFormat::I32 => build::<i32>(&device, config, renderer),
            cpal::SampleFormat::I64 => build::<i64>(&device, config, renderer),
            cpal::SampleFormat::U8 => build::<u8>(&device, config, renderer),
            cpal::SampleFormat::U16 => build::<u16>(&device, config, renderer),
            cpal::SampleFormat::U32 => build::<u32>(&device, config, renderer),
            cpal::SampleFormat::U64 => build::<u64>(&device, config, renderer),
            _ => Err(PlaybackError::Output(
                "The default output uses an unsupported sample format.".into(),
            )),
        }?;
        Ok(OpenedSink {
            format,
            sink: Box::new(CpalSink(stream)),
        })
    }
}
fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut renderer: AudioRenderer,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels;
    let errors = renderer.shared.clone();
    let generation = renderer.generation;
    device
        .build_output_stream(
            config,
            move |output: &mut [T], _: &cpal::OutputCallbackInfo| {
                renderer.render_samples(output, channels)
            },
            move |error| {
                if generation == errors.generation.load(std::sync::atomic::Ordering::Acquire)
                    && !errors.cancel.load(std::sync::atomic::Ordering::Acquire)
                {
                    errors.fail(format!("Audio output device failed: {error}"));
                }
            },
            Some(Duration::from_secs(3)),
        )
        .map_err(|error| PlaybackError::Output(error.to_string()))
}
