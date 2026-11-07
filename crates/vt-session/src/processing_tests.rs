//! Processing integration fixtures live entirely under an isolated temporary root.
use super::*;
use std::{
    fs, thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;
use vt_core::{AudioBlock, FramePosition, GapReason, StreamFormat};

fn service() -> (TempDir, Arc<AppService>) {
    let temp = TempDir::new().unwrap();
    let helper = temp.path().join("unused-capture-helper");
    fs::write(
        &helper,
        b"capture must never be launched by processing tests",
    )
    .unwrap();
    let service = AppService::new(temp.path().join("data"), helper).unwrap();
    let mut prefs = service.preferences.lock().unwrap();
    prefs.auto_refine = false;
    prefs.auto_summarize = false;
    drop(prefs);
    (temp, service)
}
fn segment(id: &str, text: &str) -> Segment {
    Segment {
        id: id.into(),
        track: "mic".into(),
        start_ms: 100,
        end_ms: 200,
        text: text.into(),
        speaker: "Me".into(),
        words: vec![TranscriptWord {
            text: text.into(),
            start_ms: 100,
            end_ms: 200,
        }],
    }
}
fn record(service: &AppService, id: &str) -> vt_record::Recording {
    let recording = vt_record::Recording::create(service.root(), id).unwrap();
    store::create_meeting(service.root(), id, "Public processing fixture", "remote").unwrap();
    recording
}
fn old_transcript(service: &AppService, id: &str) -> String {
    store::save_segment(
        service.root(),
        id,
        &segment("original", "Originalword transcript"),
    )
    .unwrap();
    store::complete_live_run(service.root(), id).unwrap();
    store::set_status(service.root(), id, "ready", None).unwrap();
    service.save_note(id, "Preserved bookmark").unwrap();
    let detail = service.detail(id).unwrap();
    let run = detail.active_run_id.unwrap();
    store::save_summary(
        service.root(),
        id,
        &run,
        detail.correction_version,
        "fixture",
        "fixture",
        "general",
        "{}",
        "Preserved summary",
        &["original".into()],
    )
    .unwrap();
    run
}
fn wait_done(service: &AppService) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while service.snapshot().unwrap().processing.is_some() {
        assert!(Instant::now() < deadline, "processing did not finish");
        thread::sleep(Duration::from_millis(10));
    }
}
fn assert_preserved(service: &AppService, id: &str, old: &str) {
    let detail = service.detail(id).unwrap();
    assert_eq!(detail.active_run_id.as_deref(), Some(old));
    assert_eq!(detail.segments[0].text, "Originalword transcript");
    assert_eq!(detail.notes[0].text, "Preserved bookmark");
    assert_eq!(detail.summaries[0].content_md, "Preserved summary");
    assert!(!detail.summaries[0].is_stale);
    assert_eq!(service.search("Originalword").unwrap().len(), 1);
    let historical = service.detail_for_run(id, Some(old)).unwrap();
    assert_eq!(historical.segments[0].text, "Originalword transcript");
    let db = store::open(service.root()).unwrap();
    assert!(
        db.execute(
            "UPDATE app_run_segments SET text='overwrite' WHERE run_id=?1",
            [old]
        )
        .is_err(),
        "completed history must reject mutation"
    );
}

#[test]
fn refinement_reservation_rejects_missing_pack_competing_jobs_and_live_capture() {
    let (_temp, service) = service();
    record(&service, "reservation").finish().unwrap();
    let old = old_transcript(&service, "reservation");
    assert!(
        service
            .refine("reservation")
            .unwrap_err()
            .to_string()
            .contains("Install")
    );
    assert_eq!(
        service.detail("reservation").unwrap().meeting.status,
        "ready"
    );
    {
        let mut runtime = service.runtime.lock().unwrap();
        runtime.refinement_model.installed = true;
        runtime.processing = true;
        runtime.processing_view = Some(ProcessingView {
            meeting_id: "other".into(),
            kind: "summary".into(),
            stage: "Fixture job".into(),
            progress: 0.5,
            error: None,
        });
    }
    assert!(
        service
            .refine("reservation")
            .unwrap_err()
            .to_string()
            .contains("already running")
    );
    assert_eq!(
        service.snapshot().unwrap().processing.unwrap().meeting_id,
        "other"
    );
    assert!(
        service
            .start("Blocked recording".into(), CaptureMode::Remote, true, None)
            .is_err()
    );
    {
        let mut runtime = service.runtime.lock().unwrap();
        runtime.processing = false;
        runtime.processing_view = None;
        let (control, _) = std::sync::mpsc::channel();
        runtime.current = Some(Current {
            view: RecordingView {
                id: "active-recording".into(),
                title: "Active fixture".into(),
                status: "paused".into(),
                elapsed_ms: 0,
                mic_level: 0.0,
                system_level: 0.0,
                gaps: 0,
                transcription_configured: false,
                transcription_active: false,
                error: None,
            },
            control,
        });
    }
    assert!(
        service
            .refine("reservation")
            .unwrap_err()
            .to_string()
            .contains("active recording")
    );
    assert_eq!(service.detail("reservation").unwrap().runs.len(), 1);
    assert_preserved(&service, "reservation", &old);
}

#[test]
fn cancellation_before_audio_work_releases_reservation_and_preserves_published_content() {
    let (_temp, service) = service();
    record(&service, "cancelled").finish().unwrap();
    let old = old_transcript(&service, "cancelled");
    service.runtime.lock().unwrap().refinement_model.installed = true;
    reserve(&service, "cancelled", "refine", Start::Manual).unwrap();
    assert_eq!(
        service.detail("cancelled").unwrap().meeting.status,
        "processing"
    );
    service.cancel_processing();
    let result = refinement_job(&service, "cancelled");
    assert!(
        result
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
    finish(&service, "cancelled", result);
    assert!(service.snapshot().unwrap().processing.is_none());
    assert!(!service.runtime.lock().unwrap().processing);
    let detail = service.detail("cancelled").unwrap();
    assert_eq!(detail.meeting.status, "recovered");
    assert_eq!(detail.runs.len(), 1);
    assert_preserved(&service, "cancelled", &old);
    // Cancellation belongs to one operation; a later accepted reservation clears it.
    reserve(&service, "cancelled", "refine", Start::Manual).unwrap();
    assert!(!service.cancel_processing.load(Ordering::Relaxed));
    service.cancel_processing();
    finish(&service, "cancelled", check_cancel(&service));
}

#[test]
fn public_refinement_worker_abandons_failed_new_run_without_replacing_history() {
    let (_temp, service) = service();
    let mut recording = record(&service, "failure");
    let block = AudioBlock::new(
        StreamFormat::new(8000, 1).unwrap(),
        FramePosition {
            epoch: 0,
            first_frame: 0,
            host_time_ns: 0,
        },
        vec![0.125; 1600],
    )
    .unwrap();
    let audio = recording.write_chunk(TrackKind::Mic, &block).unwrap();
    recording.finish().unwrap();
    let old = old_transcript(&service, "failure");
    let audio_path = service.root().join(audio.path.unwrap());
    let original_audio = fs::read(&audio_path).unwrap();
    // Exercise a stale UI installation check: the worker must detect missing weights itself.
    service.runtime.lock().unwrap().refinement_model.installed = true;
    service.refine("failure").unwrap();
    wait_done(&service);
    let detail = service.detail("failure").unwrap();
    assert_eq!(detail.meeting.status, "failed");
    assert!(detail.meeting.error.is_some());
    assert_eq!(detail.runs.len(), 2);
    let failed = detail.runs.iter().find(|run| run.id != old).unwrap();
    assert_eq!(failed.kind, "refine");
    assert_eq!(failed.status, "abandoned");
    assert!(
        service
            .detail_for_run("failure", Some(&failed.id))
            .unwrap()
            .segments
            .is_empty()
    );
    let engines: serde_json::Value = serde_json::from_str(&failed.engines_json).unwrap();
    assert_eq!(engines["model"], vt_stt::REFINEMENT_MODEL_ID);
    assert_eq!(engines["weights"].as_array().unwrap().len(), 5);
    assert_preserved(&service, "failure", &old);
    assert_eq!(fs::read(audio_path).unwrap(), original_audio);
    assert!(service.search("Unpublishedword").unwrap().is_empty());
}

#[test]
fn recorded_audio_iterator_preserves_track_order_gaps_rate_and_stereo_downmix() {
    let (_temp, service) = service();
    let mut recording = record(&service, "iterator");
    let mic = StreamFormat::new(8000, 1).unwrap();
    let system = StreamFormat::new(16000, 2).unwrap();
    recording
        .write_chunk(
            TrackKind::Mic,
            &AudioBlock::new(
                mic,
                FramePosition {
                    epoch: 0,
                    first_frame: 0,
                    host_time_ns: 0,
                },
                vec![0.25; 800],
            )
            .unwrap(),
        )
        .unwrap();
    recording
        .write_chunk(
            TrackKind::System,
            &AudioBlock::new(
                system,
                FramePosition {
                    epoch: 0,
                    first_frame: 0,
                    host_time_ns: 100_000_000,
                },
                [0.5_f32, -0.25].repeat(1600),
            )
            .unwrap(),
        )
        .unwrap();
    recording
        .write_gap(
            TrackKind::Mic,
            mic,
            vt_record::Gap {
                epoch: 0,
                first_frame: 800,
                frame_count: 800,
                start_ns: 100_000_000,
                end_ns: 200_000_000,
                reason: GapReason::Pause,
            },
        )
        .unwrap();
    recording.finish().unwrap();
    let mut packets = vec![];
    for_each_audio(&service, "iterator", |packet, fraction| {
        packets.push((packet, fraction));
        Ok(())
    })
    .unwrap();
    assert_eq!(packets.len(), 11);
    assert!(packets.windows(2).all(|pair| pair[0].1 <= pair[1].1));
    assert_eq!(packets.last().unwrap().1, 1.0);
    let mut mic_count = 0;
    let mut system_count = 0;
    let mut gap_count = 0;
    let mut previous = 0;
    for (packet, _) in &packets {
        match packet {
            AudioPacket::Audio {
                track,
                samples,
                rate,
                start_ms,
            } => {
                assert!(*start_ms >= previous);
                previous = *start_ms;
                assert!(samples.len() <= *rate as usize / 50);
                if *track == TrackKind::Mic {
                    assert_eq!(*rate, 8000);
                    mic_count += samples.len();
                    assert!(samples.iter().all(|s| (*s - 0.25).abs() < 1e-6));
                } else {
                    assert_eq!(*rate, 16000);
                    system_count += samples.len();
                    assert!(samples.iter().all(|s| (*s - 0.125).abs() < 1e-6));
                }
            }
            AudioPacket::Gap { track, start_ms } => {
                assert_eq!(*track, TrackKind::Mic);
                assert_eq!(*start_ms, 100);
                gap_count += 1;
            }
        }
    }
    assert_eq!((mic_count, system_count, gap_count), (800, 1600, 1));
    let mut delivered = 0;
    let cancelled = for_each_audio(&service, "iterator", |_, _| {
        delivered += 1;
        service.cancel_processing();
        Ok(())
    });
    assert!(cancelled.unwrap_err().to_string().contains("cancelled"));
    assert_eq!(
        delivered, 1,
        "cancellation must stop before decoding the next chunk"
    );
}

#[cfg(unix)]
#[test]
#[ignore = "requires public VT_STT_FIXTURE_WAV and verified VT_REFINEMENT_MODEL_DIR cache; no downloads"]
fn public_refinement_pipeline_publishes_real_words_and_retains_original_run() {
    let (_temp, service) = service();
    let model = std::env::var_os("VT_REFINEMENT_MODEL_DIR")
        .map(PathBuf::from)
        .expect("set verified public refinement model cache");
    let fixture = std::env::var("VT_STT_FIXTURE_WAV").expect("set public WAV fixture");
    let wave = sherpa_onnx::Wave::read(&fixture).unwrap();
    let rate = wave.sample_rate() as u32;
    let destination = RefinementModelManager::new(service.root().join("models")).model_dir();
    fs::create_dir_all(&destination).unwrap();
    // Immutable test-local hard links avoid a second 640MiB model copy. No writes
    // or installation calls target either the source cache or user model folders.
    for name in vt_stt::REFINEMENT_FILES
        .iter()
        .map(|entry| entry.0)
        .chain(std::iter::once("manifest.json"))
    {
        fs::hard_link(model.join(name), destination.join(name)).unwrap();
    }
    assert!(RefinementModelManager::new(service.root().join("models")).installed());
    service.runtime.lock().unwrap().refinement_model.installed = true;
    let mut recording = record(&service, "real-pipeline");
    let format = StreamFormat::new(rate, 1).unwrap();
    for (track, origin) in [
        (TrackKind::Mic, 1_000_000_000),
        (TrackKind::System, 3_000_000_000),
    ] {
        let mut frame = 0_u64;
        for samples in wave.samples().chunks(rate as usize) {
            let block = AudioBlock::new(
                format,
                FramePosition {
                    epoch: 0,
                    first_frame: frame,
                    host_time_ns: origin + frame * 1_000_000_000 / u64::from(rate),
                },
                samples.to_vec(),
            )
            .unwrap();
            recording.write_chunk(track, &block).unwrap();
            frame += samples.len() as u64;
        }
    }
    let chunks = recording.chunks().unwrap();
    let digests = chunks
        .iter()
        .map(|chunk| (chunk.path.clone(), chunk.sha256.clone()))
        .collect::<Vec<_>>();
    recording.finish().unwrap();
    let old = old_transcript(&service, "real-pipeline");
    service.refine("real-pipeline").unwrap();
    wait_done(&service);
    let detail = service.detail("real-pipeline").unwrap();
    assert_eq!(detail.meeting.status, "ready", "{:?}", detail.meeting.error);
    let active = detail.active_run_id.as_deref().unwrap();
    assert_ne!(active, old);
    assert_eq!(detail.runs.len(), 2);
    assert!(detail.runs.iter().all(|run| run.status == "complete"));
    for (track, origin) in [("mic", 1000), ("system", 3000)] {
        let segments = detail
            .segments
            .iter()
            .filter(|segment| segment.track == track)
            .collect::<Vec<_>>();
        let text = segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        assert!(text.contains("yellow lamps"), "{track}: {text}");
        assert!(segments.iter().all(|segment| {
            segment.start_ms >= origin
                && segment.end_ms <= origin + wave.samples().len() as u64 * 1000 / u64::from(rate)
                && !segment.words.is_empty()
                && segment
                    .words
                    .iter()
                    .all(|word| word.start_ms >= segment.start_ms && word.end_ms <= segment.end_ms)
        }));
    }
    assert_eq!(service.search("yellow").unwrap().len(), 1);
    assert!(service.search("Originalword").unwrap().is_empty());
    assert_eq!(
        service
            .detail_for_run("real-pipeline", Some(&old))
            .unwrap()
            .segments[0]
            .text,
        "Originalword transcript"
    );
    assert!(detail.summaries[0].is_stale);
    assert_eq!(detail.notes[0].text, "Preserved bookmark");
    let recovered = vt_record::Recording::recover(service.root(), "real-pipeline").unwrap();
    assert_eq!(recovered.verified_chunks, chunks.len());
    let recording_rows = store::open(service.root()).unwrap();
    let mut query=recording_rows.prepare("SELECT path,sha256 FROM audio_chunks WHERE meeting_id='real-pipeline' ORDER BY track,seq").unwrap();
    let after = query
        .query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    let mut before = digests;
    before.sort();
    let mut after = after;
    after.sort();
    assert_eq!(after, before);
}

#[test]
fn shutdown_suppresses_both_automatic_chains_and_rejects_manual_refinement() {
    for auto_refine in [true, false] {
        let (_temp, service) = service();
        record(&service, "shutdown").finish().unwrap();
        let old = old_transcript(&service, "shutdown");
        {
            let mut prefs = service.preferences.lock().unwrap();
            prefs.auto_refine = auto_refine;
            prefs.auto_summarize = true;
            prefs.summary.provider = vt_summarize::Provider::Apple;
        }
        let (control, _receiver) = std::sync::mpsc::channel();
        {
            let mut runtime = service.runtime.lock().unwrap();
            runtime.refinement_model.installed = true;
            runtime.current = Some(Current {
                view: RecordingView {
                    id: "shutdown".into(),
                    title: "Shutdown fixture".into(),
                    status: "processing".into(),
                    elapsed_ms: 0,
                    mic_level: 0.0,
                    system_level: 0.0,
                    gaps: 0,
                    transcription_configured: false,
                    transcription_active: false,
                    error: None,
                },
                control,
            });
            runtime
                .partials
                .insert(TrackKind::Mic, segment("partial", "Transient partial"));
        }
        service.shutdown();
        after_recording(service.clone(), "shutdown");
        let snapshot = service.snapshot().unwrap();
        assert!(snapshot.recording.is_none());
        assert!(snapshot.processing.is_none());
        let runtime = service.runtime.lock().unwrap();
        assert!(!runtime.processing);
        assert!(runtime.partials.is_empty());
        drop(runtime);
        let rejected = service.refine("shutdown").unwrap_err().to_string();
        assert!(rejected.contains("closing"), "{rejected}");
        let detail = service.detail("shutdown").unwrap();
        assert_eq!(detail.meeting.status, "ready");
        assert!(
            detail.meeting.error.is_none(),
            "shutdown must not record an automatic-processing failure"
        );
        assert_eq!(
            detail.runs.len(),
            1,
            "neither refinement nor summary may start"
        );
        assert_preserved(&service, "shutdown", &old);
    }
}

#[test]
fn correction_clears_effective_word_alignment_but_preserves_raw_completed_history() {
    let (_temp, service) = service();
    record(&service, "correction-timing").finish().unwrap();
    let old = old_transcript(&service, "correction-timing");
    let db = store::open(service.root()).unwrap();
    let original: (String, String) = db
        .query_row(
            "SELECT text,words_json FROM app_run_segments WHERE run_id=?1 AND id='original'",
            [&old],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    service
        .correct_segment(
            "correction-timing",
            &old,
            "original",
            "Originalword transcript",
        )
        .unwrap();
    assert_eq!(
        service.detail("correction-timing").unwrap().segments[0]
            .words
            .len(),
        1,
        "an unchanged correction still matches its original alignment"
    );
    service
        .correct_segment(
            "correction-timing",
            &old,
            "original",
            "Correctedword transcript",
        )
        .unwrap();
    let effective = service.detail("correction-timing").unwrap();
    assert_eq!(effective.segments[0].text, "Correctedword transcript");
    assert!(
        effective.segments[0].words.is_empty(),
        "old acoustic word times must not label edited text"
    );
    let retained: (String, String) = db
        .query_row(
            "SELECT text,words_json FROM app_run_segments WHERE run_id=?1 AND id='original'",
            [&old],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        retained, original,
        "the correction overlay must not rewrite raw history"
    );
    let words: Vec<TranscriptWord> = serde_json::from_str(&retained.1).unwrap();
    assert_eq!(words.len(), 1);
    assert_eq!(words[0].text, "Originalword transcript");
    assert_eq!((words[0].start_ms, words[0].end_ms), (100, 200));
    assert_eq!(service.search("Correctedword").unwrap().len(), 1);
    assert!(service.search("Originalword").unwrap().is_empty());
}
