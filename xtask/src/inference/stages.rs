use super::{error, telemetry::Telemetry};
use crate::EvalResult;
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use vt_core::TrackKind;
use vt_diarize::{DiarizationResult, Diarizer, DiarizerConfig, MeetingMode, Phase, SpeakerTurn};
use vt_stt::{BatchRefiner, StreamingRecognizer, WordTiming};
pub(super) struct TextRun {
    pub text: String,
    pub events: Vec<Value>,
    pub phase: Value,
}
pub(super) struct DiarRun {
    pub result: DiarizationResult,
    pub turns: Vec<SpeakerTurn>,
    pub phase: Value,
    pub emissions: Vec<Value>,
}
fn words(words: &[WordTiming]) -> Vec<Value> {
    words
        .iter()
        .map(|w| json!({"text":w.text,"start_ms":w.start_ms,"end_ms":w.end_ms}))
        .collect()
}
fn pace(start: Instant, end_ms: f64, enabled: bool) {
    if enabled {
        let deadline = Duration::from_secs_f64(end_ms / 1000.0);
        if let Some(wait) = deadline.checked_sub(start.elapsed()) {
            std::thread::sleep(wait)
        }
    }
}
fn phase(
    name: &str,
    load_ms: f64,
    start: Instant,
    audio_ms: f64,
    calls: &[f64],
    extra: Value,
) -> Value {
    let wall_ms = start.elapsed().as_secs_f64() * 1000.0;
    json!({"name":name,"constructor_load_ms":load_ms,"processing_wall_ms":wall_ms,"audio_ms":audio_ms,"real_time_factor":wall_ms/audio_ms,"native_accept_call_ms":crate::observations::percentiles(calls),"extra":extra})
}
pub(super) fn live(
    samples: &[f32],
    rate: u32,
    dir: &Path,
    vad: Option<&Path>,
    paced: bool,
    telemetry: &mut Telemetry,
) -> EvalResult<TextRun> {
    live_options(samples, rate, dir, vad, None, paced, telemetry)
}
pub(super) fn live_options(
    samples: &[f32],
    rate: u32,
    dir: &Path,
    vad: Option<&Path>,
    options: Option<vt_stt::LiveVadOptions>,
    paced: bool,
    telemetry: &mut Telemetry,
) -> EvalResult<TextRun> {
    eprintln!(
        "Inference phase: production streaming Zipformer; VAD={}",
        vad.is_some()
    );
    let load = Instant::now();
    let mut engine = match (vad, options) {
        (Some(vad), Some(options)) => StreamingRecognizer::with_vad_options(dir, vad, options),
        (Some(vad), None) => StreamingRecognizer::with_vad(dir, Some(vad)),
        (None, None) => StreamingRecognizer::new(dir),
        (None, Some(_)) => return Err("VAD options require an explicit pinned VAD".into()),
    }
    .map_err(error)?;
    let load_ms = load.elapsed().as_secs_f64() * 1000.0;
    telemetry.sample();
    let start = Instant::now();
    let mut calls = vec![];
    let mut events = vec![];
    let mut final_text = vec![];
    let mut offset = 0;
    for packet in samples.chunks((rate / 50) as usize) {
        let end = (offset + packet.len()) as f64 * 1000.0 / f64::from(rate);
        pace(start, end, paced);
        let call = Instant::now();
        let updates = engine
            .accept(
                TrackKind::Mic,
                packet,
                rate,
                offset as u64 * 1000 / u64::from(rate),
            )
            .map_err(error)?;
        calls.push(call.elapsed().as_secs_f64() * 1000.0);
        let emitted = start.elapsed().as_secs_f64() * 1000.0;
        for e in updates {
            if e.is_final {
                final_text.push(e.text.clone())
            }
            events.push(json!({"track":"mic","start_ms":e.start_ms,"end_ms":e.end_ms,"text":e.text,"is_final":e.is_final,"words":words(&e.words),"input_available_ms":end,"emitted_wall_ms":emitted,"paced_delivery_delay_ms":paced.then_some(emitted-end)}));
        }
        offset += packet.len();
        telemetry.sample();
    }
    for e in engine.finish().map_err(error)? {
        if e.is_final {
            final_text.push(e.text.clone())
        }
        events.push(json!({"track":"mic","start_ms":e.start_ms,"end_ms":e.end_ms,"text":e.text,"is_final":e.is_final,"words":words(&e.words),"input_available_ms":samples.len() as f64*1000.0/f64::from(rate),"emitted_wall_ms":start.elapsed().as_secs_f64()*1000.0,"terminal_flush":true}));
    }
    Ok(TextRun {
        text: final_text.join(" "),
        events,
        phase: phase(
            if vad.is_some() {
                "streaming_zipformer_vad"
            } else {
                "streaming_zipformer_ungated"
            },
            load_ms,
            start,
            samples.len() as f64 * 1000.0 / f64::from(rate),
            &calls,
            json!({"paced":paced,"vad_enabled":vad.is_some(),"vad_threshold":vad.map(|_|options.unwrap_or_default().threshold),"thread_budget":2,"vad_threads":if vad.is_some(){1}else{0}}),
        ),
    })
}
pub(super) fn batch(
    samples: &[f32],
    rate: u32,
    dir: &Path,
    telemetry: &mut Telemetry,
) -> EvalResult<TextRun> {
    eprintln!("Inference phase: production Parakeet + Silero VAD");
    let load = Instant::now();
    let mut engine = BatchRefiner::new(dir).map_err(error)?;
    let load_ms = load.elapsed().as_secs_f64() * 1000.0;
    telemetry.sample();
    let start = Instant::now();
    let mut calls = vec![];
    let mut events = vec![];
    let mut text = vec![];
    let mut offset = 0;
    for packet in samples.chunks((rate / 50) as usize) {
        let call = Instant::now();
        let updates = engine
            .accept(
                TrackKind::Mic,
                packet,
                rate,
                offset as u64 * 1000 / u64::from(rate),
            )
            .map_err(error)?;
        calls.push(call.elapsed().as_secs_f64() * 1000.0);
        for e in updates {
            text.push(e.text.clone());
            events.push(json!({"track":"mic","start_ms":e.start_ms,"end_ms":e.end_ms,"text":e.text,"words":words(&e.words),"input_available_ms":(offset+packet.len()) as f64*1000.0/f64::from(rate),"emitted_wall_ms":start.elapsed().as_secs_f64()*1000.0}));
        }
        offset += packet.len();
        telemetry.sample();
    }
    for e in engine.finish().map_err(error)? {
        text.push(e.text.clone());
        events.push(json!({"track":"mic","start_ms":e.start_ms,"end_ms":e.end_ms,"text":e.text,"words":words(&e.words),"emitted_wall_ms":start.elapsed().as_secs_f64()*1000.0,"terminal_flush":true}));
    }
    Ok(TextRun {
        text: text.join(" "),
        events,
        phase: phase(
            "batch_parakeet_vad",
            load_ms,
            start,
            samples.len() as f64 * 1000.0 / f64::from(rate),
            &calls,
            json!({"thread_budget":2,"vad_threads":1}),
        ),
    })
}
pub(super) fn diarize(
    samples: &[f32],
    rate: u32,
    dir: &Path,
    kind: Phase,
    paced: bool,
    telemetry: &mut Telemetry,
) -> EvalResult<DiarRun> {
    eprintln!("Inference phase: independent production {kind:?} diarization");
    let load = Instant::now();
    let mut engine = Diarizer::new(
        dir,
        DiarizerConfig {
            mode: MeetingMode::InPerson,
            phase: kind,
            ..Default::default()
        },
    )
    .map_err(error)?;
    let load_ms = load.elapsed().as_secs_f64() * 1000.0;
    telemetry.sample();
    let start = Instant::now();
    let mut calls = vec![];
    let mut online = vec![];
    let mut emissions = vec![];
    let mut offset = 0;
    for packet in samples.chunks((rate / 50) as usize) {
        let end = (offset + packet.len()) as f64 * 1000.0 / f64::from(rate);
        pace(start, end, paced);
        let call = Instant::now();
        let updates = engine
            .accept(
                TrackKind::Mic,
                packet,
                rate,
                offset as u64 * 1000 / u64::from(rate),
            )
            .map_err(error)?;
        calls.push(call.elapsed().as_secs_f64() * 1000.0);
        for turn in &updates {
            emissions.push(json!({"turn_id":turn.id,"source_start_ms":turn.start_ms,"source_end_ms":turn.end_ms,"input_available_ms":end,"emitted_wall_ms":start.elapsed().as_secs_f64()*1000.0,"paced_delivery_delay_ms":paced.then_some(start.elapsed().as_secs_f64()*1000.0-end)}));
        }
        online.extend(updates);
        offset += packet.len();
        telemetry.sample();
    }
    let tails = engine.reset_track(TrackKind::Mic).map_err(error)?;
    for turn in &tails {
        emissions.push(json!({"turn_id":turn.id,"source_start_ms":turn.start_ms,"source_end_ms":turn.end_ms,"emitted_wall_ms":start.elapsed().as_secs_f64()*1000.0,"terminal_flush":true}));
    }
    online.extend(tails);
    let result = engine.finish().map_err(error)?;
    // Live scoring uses what accept/reset actually emitted, before finish's
    // retrospective clustering. Phase is not a shortcut to an offline score.
    let turns = if kind == Phase::Live {
        online
    } else {
        result.turns.clone()
    };
    let extra = json!({"paced":paced,"segmentation_threads":1,"embedding_threads":1,"segmentation_context":kind.segmentation_context(),"turns":turns.len(),"unknown_or_uncertain":turns.iter().filter(|t|t.uncertain||t.provisional||t.speaker_ids.is_empty()).count(),"calibrated":result.calibrated,"live_uses_emitted_updates":kind==Phase::Live});
    Ok(DiarRun {
        result,
        turns,
        emissions,
        phase: phase(
            if kind == Phase::Live {
                "live_diarization"
            } else {
                "refined_diarization"
            },
            load_ms,
            start,
            samples.len() as f64 * 1000.0 / f64::from(rate),
            &calls,
            extra,
        ),
    })
}
