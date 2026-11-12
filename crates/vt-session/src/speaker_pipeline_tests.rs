//! Pure alignment tests plus an explicitly ignored public-audio/model integration.
//! No test opens a microphone, speakers, private recordings, or cloud services.
use super::*;
use crate::{AppService, Segment, TranscriptWord, store};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};
use vt_diarize::SpeakerTurn;

fn turn(id: &str, label: &str, start: u64, end: u64) -> SpeakerTurn {
    SpeakerTurn {
        id: format!("turn-{id}-{start}"),
        track: "system".into(),
        start_ms: start,
        end_ms: end,
        speaker_ids: vec![id.into()],
        label: label.into(),
        provenance: "refined-cluster".into(),
        score: Some(0.8),
        overlap: false,
        uncertain: false,
        provisional: false,
    }
}
fn segment(text: &str, words: &[(&str, u64, u64)]) -> Segment {
    Segment {
        id: "source-segment".into(),
        track: "system".into(),
        start_ms: 50,
        end_ms: 1000,
        text: text.into(),
        speaker: "Prior label must not be inherited".into(),
        words: words
            .iter()
            .map(|(text, start, end)| TranscriptWord {
                text: (*text).into(),
                start_ms: *start,
                end_ms: *end,
            })
            .collect(),
    }
}
fn assert_word_bounds(segments: &[Segment]) {
    for segment in segments {
        for word in &segment.words {
            assert!(
                segment.start_ms <= word.start_ms
                    && word.start_ms <= word.end_ms
                    && word.end_ms <= segment.end_ms,
                "word must stay inside its source segment"
            );
        }
    }
}

#[test]
fn located_words_preserve_exact_source_bytes_punctuation_and_whitespace_across_speakers() {
    let input = segment(
        "  Hello,  world!\n",
        &[("Hello,", 100, 300), ("world!", 600, 850)],
    );
    let text = input.text.clone();
    let output = split_segment(
        input,
        &[turn("a", "Alpha", 0, 500), turn("b", "Beta", 500, 1100)],
    );
    assert_eq!(output.len(), 2);
    assert_eq!(
        output
            .iter()
            .map(|part| part.text.as_str())
            .collect::<String>(),
        text,
        "speaker changes must not remove exact source punctuation or whitespace"
    );
    assert_eq!(output[0].speaker, "Alpha");
    assert_eq!(output[1].speaker, "Beta");
    assert_eq!(output[0].id, "source-segment");
    assert_ne!(output[0].id, output[1].id);
    assert_eq!((output[0].start_ms, output[0].end_ms), (50, 300));
    assert_eq!((output[1].start_ms, output[1].end_ms), (600, 1000));
    assert_eq!(output[0].words[0].text, "Hello,");
    assert_eq!(output[1].words[0].text, "world!");
    assert_word_bounds(&output);
}

#[test]
fn cjk_and_repeated_tokens_are_located_in_order_without_utf8_or_punctuation_damage() {
    let input = segment(
        "你好，世界！你好。",
        &[
            ("你好", 100, 250),
            ("，", 250, 300),
            ("世界", 550, 700),
            ("！", 700, 750),
            ("你好", 800, 900),
            ("。", 900, 950),
        ],
    );
    let original = input.text.clone();
    let output = split_segment(
        input,
        &[turn("a", "Alpha", 0, 500), turn("b", "Beta", 500, 1100)],
    );
    assert_eq!(output.len(), 2);
    assert_eq!(output[0].text, "你好，");
    assert_eq!(output[1].text, "世界！你好。");
    assert_eq!(
        output
            .iter()
            .map(|part| part.text.as_str())
            .collect::<String>(),
        original
    );
    assert_eq!(output.iter().map(|part| part.words.len()).sum::<usize>(), 6);
    assert_word_bounds(&output);
}

#[test]
fn speaker_ids_not_display_names_define_boundaries_and_word_midpoints_are_half_open() {
    let input = segment("one two", &[("one", 100, 300), ("two", 400, 600)]);
    let output = split_segment(
        input,
        &[
            turn("a", "Same display name", 0, 500),
            turn("b", "Same display name", 500, 1100),
        ],
    );
    assert_eq!(
        output.len(),
        2,
        "the second word midpoint is exactly the next turn's boundary, and distinct IDs must remain distinct even with equal names"
    );
    assert_eq!(output[0].words[0].text, "one");
    assert_eq!(output[1].words[0].text, "two");
    assert_word_bounds(&output);
}

#[test]
fn unknown_provisional_uncertain_overlap_and_other_tracks_never_inherit_a_clean_label() {
    let input = segment("one two", &[("one", 100, 300), ("two", 600, 800)]);
    let mut uncertain = turn("b", "Must not survive uncertainty", 500, 1100);
    uncertain.uncertain = true;
    let output = split_segment(input.clone(), &[turn("a", "Alpha", 0, 500), uncertain]);
    assert_eq!(output[1].speaker, "Unknown");
    let mut provisional = turn("b", "Must not survive provisional", 500, 1100);
    provisional.provisional = true;
    assert_eq!(
        split_segment(input.clone(), &[turn("a", "Alpha", 0, 500), provisional])[1].speaker,
        "Unknown"
    );
    let mut overlapping = turn("b", "Must not survive overlap", 500, 1100);
    overlapping.overlap = true;
    assert_eq!(
        split_segment(input.clone(), &[turn("a", "Alpha", 0, 500), overlapping])[1].speaker,
        "Overlap"
    );
    let simultaneous = split_segment(
        input.clone(),
        &[turn("a", "Alpha", 0, 1100), turn("b", "Beta", 500, 1100)],
    );
    assert_eq!(simultaneous[1].speaker, "Overlap");
    let mut microphone = turn("mic", "Mic only", 0, 1100);
    microphone.track = "mic".into();
    assert_eq!(
        split_segment(input.clone(), &[microphone])[0].speaker,
        "Unknown"
    );
    assert_eq!(split_segment(input, &[])[0].speaker, "Unknown");
}

#[test]
fn decoder_token_mismatch_keeps_complete_source_text_and_reports_combined_labels() {
    let input = segment(
        "alpha, beta!",
        &[("alpha", 100, 300), ("decoder-mismatch", 600, 850)],
    );
    let output = split_segment(
        input.clone(),
        &[turn("a", "Alpha", 0, 500), turn("b", "Beta", 500, 1100)],
    );
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].text, input.text);
    assert_eq!(output[0].id, input.id);
    assert_eq!(output[0].speaker, "Alpha / Beta");
    assert_eq!(
        (output[0].start_ms, output[0].end_ms),
        (input.start_ms, input.end_ms)
    );
    assert_eq!(
        output[0].words[1].text, "decoder-mismatch",
        "the fallback must not invent or alter acoustic decoder alignment"
    );
    assert_word_bounds(&output);
}

#[test]
fn missing_word_alignment_cannot_assign_an_entire_multi_voice_segment_to_its_midpoint() {
    let input = segment("A sentence spanning two different voices.", &[]);
    let text = input.text.clone();
    let output = split_segment(
        input,
        &[turn("a", "Alpha", 0, 500), turn("b", "Beta", 500, 1100)],
    );
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].text, text);
    assert!(output[0].words.is_empty());
    assert!(
        !["Alpha", "Beta"].contains(&output[0].speaker.as_str()),
        "without acoustic words a crossing segment must keep an honest combined/unknown label, not one voice chosen at the sentence midpoint"
    );
}

fn hardlink_models(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            hardlink_models(&entry.path(), &target);
        } else {
            assert!(
                kind.is_file(),
                "cached public model tree must contain only regular files"
            );
            crate::test_fixtures::link_or_copy(&entry.path(), &target);
        }
    }
}
fn audio_snapshot(root: &Path, meeting: &str) -> Vec<(String, String, Vec<u8>)> {
    let db = store::open(root).unwrap();
    let mut q=db.prepare("SELECT path,sha256 FROM audio_chunks WHERE meeting_id=?1 AND state='ready' ORDER BY track,seq").unwrap();
    q.query_map([meeting], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })
    .unwrap()
    .map(|row| {
        let (path, hash) = row.unwrap();
        let bytes = fs::read(root.join(&path)).unwrap();
        (path, hash, bytes)
    })
    .collect()
}
fn has_vector_payload(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(fields) => fields.iter().any(|(key, value)| {
            matches!(
                key.as_str(),
                "embedding" | "embeddings" | "vector" | "vectors"
            ) || has_vector_payload(value)
        }),
        serde_json::Value::Array(values) => values.iter().any(has_vector_payload),
        _ => false,
    }
}

#[test]
#[ignore = "opt-in public fixture with preverified cached Parakeet/diarization models; no downloads, capture, output, or private data"]
fn real_public_refinement_publishes_new_run_with_turns_words_and_retained_history() {
    public_refinement_case(false);
}

#[test]
#[ignore = "opt-in public Parakeet/diarization caches; unresolved saved correction must not discard recognized speech"]
fn real_public_refinement_with_unresolved_constraint_publishes_unknown_speech_and_review() {
    public_refinement_case(true);
}

fn public_refinement_case(with_pending_constraint: bool) {
    use vt_core::{AudioBlock, FramePosition, StreamFormat};
    let fixture = crate::test_fixtures::path("vantranscribe-stt-fixture.wav");
    let refinement = crate::test_fixtures::path("vantranscribe-refinement-models")
        .join(vt_stt::REFINEMENT_MODEL_ID);
    let diarization =
        crate::test_fixtures::path("vt-diarization-models/verified").join(vt_diarize::MODEL_ID);
    assert!(
        fixture.is_file() && refinement.is_dir() && diarization.is_dir(),
        "explicit public fixture/model caches must exist before this ignored test is selected"
    );
    let original_wave = fs::read(&fixture).unwrap();
    let temp = tempfile::TempDir::new_in("/private/tmp").unwrap();
    let root = temp.path().join("data");
    let models = root.join("models");
    hardlink_models(&refinement, &models.join(vt_stt::REFINEMENT_MODEL_ID));
    hardlink_models(&diarization, &models.join(vt_diarize::MODEL_ID));
    let service = AppService::new(root, temp.path().join("never-launched-capture-helper")).unwrap();
    {
        let mut prefs = service.preferences.lock().unwrap();
        prefs.auto_refine = false;
        prefs.auto_summarize = false;
        let mut audio = service.audio_preferences.lock().unwrap();
        audio.diarization_enabled = true;
        audio.live_vad_enabled = true;
    }
    assert!(service.snapshot().unwrap().refinement_models.installed);
    assert!(service.snapshot().unwrap().diarization_models.installed);
    let wave = sherpa_onnx::Wave::read(fixture.to_str().unwrap()).unwrap();
    let rate = wave.sample_rate() as u32;
    let high_rate = sherpa_onnx::LinearResampler::create(rate as i32, 48000)
        .unwrap()
        .resample(wave.samples(), true);
    let meeting = "public-speaker-integration";
    let mut recording = vt_record::Recording::create(service.root(), meeting).unwrap();
    store::create_meeting(
        service.root(),
        meeting,
        "Public speaker integration fixture",
        "remote",
    )
    .unwrap();
    for (track, samples, rate, origin) in [
        (TrackKind::Mic, wave.samples(), rate, 0u64),
        (TrackKind::System, high_rate.as_slice(), 48000, 1000u64),
    ] {
        let mut first = 0;
        for packet in samples.chunks(rate as usize * 2) {
            recording
                .write_chunk(
                    track,
                    &AudioBlock::new(
                        StreamFormat::new(rate, 1).unwrap(),
                        FramePosition {
                            epoch: 0,
                            first_frame: first,
                            host_time_ns: origin * 1_000_000
                                + first * 1_000_000_000 / u64::from(rate),
                        },
                        packet.to_vec(),
                    )
                    .unwrap(),
                )
                .unwrap();
            first += packet.len() as u64;
        }
    }
    recording.finish().unwrap();
    store::save_segment(
        service.root(),
        meeting,
        &Segment {
            id: "previous-source".into(),
            track: "system".into(),
            start_ms: 1000,
            end_ms: 2000,
            text: "Synthetic previous transcript retained for history.".into(),
            speaker: "Previous channel label".into(),
            words: vec![],
        },
    )
    .unwrap();
    let previous = store::detail(service.root(), meeting)
        .unwrap()
        .run_id
        .unwrap();
    let mut old_a = turn(
        "old-a",
        "Old alpha",
        1000,
        if with_pending_constraint { 10000 } else { 3500 },
    );
    old_a.id = "previous-turn-a".into();
    let mut old_b = turn(
        "old-b",
        "Old beta",
        if with_pending_constraint { 12000 } else { 3500 },
        if with_pending_constraint { 14000 } else { 7000 },
    );
    old_b.id = "previous-turn-b".into();
    store::save_diarization_result(
        service.root(),
        meeting,
        &previous,
        &DiarizationResult {
            turns: vec![old_a, old_b],
            speakers: vec![],
            merges: vec![],
            constraint_reviews: vec![],
            segmentation_context: None,
            embedding_version: "synthetic-previous-version".into(),
            calibrated: false,
            warnings: vec!["Synthetic prior metadata, not a diarization benchmark.".into()],
        },
        false,
    )
    .unwrap();
    let before = store::detail(service.root(), meeting).unwrap();
    let mut names = BTreeMap::new();
    for (speaker, name) in before
        .speakers
        .iter()
        .zip(["Fixture Alpha", "Fixture Beta"])
    {
        service.rename_speaker(meeting, &speaker.id, name).unwrap();
        names.insert(speaker.id.clone(), name.to_owned());
    }
    store::open(service.root()).unwrap().execute("INSERT INTO app_speaker_reviews(id,run_id,kind,previous_ids_json,current_ids_json,reason) VALUES ('prior-review',?1,'synthetic-history','[]','[]','Prior synthetic review must remain attached to its source run')",[&previous]).unwrap();
    store::complete_live_run(service.root(), meeting).unwrap();
    store::set_status(service.root(), meeting, "ready", None).unwrap();
    let pending_constraint = if with_pending_constraint {
        service
            .add_speaker_constraint(
                meeting,
                &previous,
                "previous-turn-a",
                "previous-turn-b",
                "cannot_link",
            )
            .unwrap();
        Some(
            store::speaker_constraints(service.root(), meeting)
                .unwrap()
                .remove(0)
                .id,
        )
    } else {
        None
    };
    let audio_before = audio_snapshot(service.root(), meeting);
    service.refine(meeting).unwrap();
    let deadline = Instant::now() + Duration::from_secs(180);
    while service.snapshot().unwrap().processing.is_some() {
        if Instant::now() >= deadline {
            service.cancel_processing();
            panic!("public refinement did not finish within the bounded integration timeout");
        }
        thread::sleep(Duration::from_millis(20));
    }
    let current = store::detail(service.root(), meeting).unwrap();
    assert_eq!(
        current.meeting.status, "ready",
        "{:?}",
        current.meeting.error
    );
    assert_ne!(current.run_id.as_ref(), Some(&previous));
    let active = current.run_id.as_ref().unwrap();
    assert!(
        current
            .runs
            .iter()
            .any(|run| &run.id == active && run.kind == "refine" && run.status == "complete")
    );
    assert!(
        current
            .runs
            .iter()
            .any(|run| run.id == previous && run.status == "complete")
    );
    assert!(
        !current.speaker_turns.is_empty(),
        "actual independent audio-derived turns must be persisted"
    );
    assert!(current.speaker_turns.iter().any(|turn| turn.track == "mic"));
    assert!(
        current
            .speaker_turns
            .iter()
            .any(|turn| turn.track == "system")
    );
    assert!(
        current
            .speaker_turns
            .iter()
            .all(|turn| turn.start_ms < turn.end_ms && !turn.provenance.is_empty())
    );
    let info = current.diarization.as_ref().unwrap();
    assert_eq!(info.embedding_version, vt_diarize::EMBEDDING_VERSION);
    assert_eq!(
        info.segmentation_context,
        Some(vt_diarize::Phase::Refined.segmentation_context())
    );
    assert!(!info.calibrated);
    assert!(!current.segments.is_empty());
    assert!(
        current
            .segments
            .iter()
            .any(|segment| !segment.words.is_empty())
    );
    assert_word_bounds(&current.segments);
    assert!(
        current
            .segments
            .iter()
            .any(|segment| segment.track == "mic")
    );
    assert!(
        current
            .segments
            .iter()
            .any(|segment| segment.track == "system")
    );
    for assignment in &current.speaker_assignments {
        let source = current
            .segments
            .iter()
            .find(|segment| segment.id == assignment.segment_id)
            .unwrap();
        assert_eq!(assignment.words.len(), source.words.len());
        assert!(["user", "audio-derived"].contains(&assignment.provenance.as_str()));
        assert!(
            assignment
                .words
                .iter()
                .enumerate()
                .all(|(index, word)| word.index == index && !word.label.is_empty())
        );
    }
    assert!(
        current
            .speaker_assignments
            .iter()
            .any(|assignment| !assignment.words.is_empty()),
        "word-level source assignment metadata must be returned"
    );
    let historical = service.detail_for_run(meeting, Some(&previous)).unwrap();
    assert!(
        historical
            .diarization
            .as_ref()
            .unwrap()
            .segmentation_context
            .is_none()
    );
    assert_eq!(
        historical.segments[0].text,
        "Synthetic previous transcript retained for history."
    );
    assert!(
        historical
            .speaker_reviews
            .iter()
            .any(|review| review.id == "prior-review" && review.status == "needs_review")
    );
    for (id, name) in names {
        assert!(
            historical
                .speakers
                .iter()
                .any(|speaker| speaker.id == id
                    && speaker.user_name.as_deref() == Some(name.as_str()))
        );
    }
    if let Some(constraint) = &pending_constraint {
        let text = current
            .segments
            .iter()
            .filter(|segment| segment.track == "system")
            .map(|segment| segment.text.as_str())
            .collect::<String>()
            .to_lowercase();
        assert!(
            text.contains("yellow lamps"),
            "complete Parakeet text must survive unresolved speaker constraints: {text}"
        );
        assert!(
            current
                .segments
                .iter()
                .filter(|segment| segment.track == "system")
                .all(|segment| segment.speaker == "Unknown")
        );
        let review = current
            .speaker_reviews
            .iter()
            .find(|review| {
                review.kind == "constraint"
                    && serde_json::to_value(review).unwrap()["constraint_id"] == *constraint
            })
            .expect("real unresolved correction must retain its originating user UUID");
        assert_eq!(review.status, "needs_review");
        assert!(
            current
                .speaker_constraints
                .iter()
                .any(|entry| &entry.id == constraint && entry.active)
        );
        let revision = current.correction_version;
        service
            .revoke_speaker_constraint(meeting, constraint)
            .unwrap();
        let revoked = service.detail(meeting).unwrap();
        assert_eq!(revoked.correction_version, revision + 1);
        assert_eq!(revoked.active_run_id, current.active_run_id);
        assert!(
            revoked
                .speaker_reviews
                .iter()
                .find(|entry| entry.id == review.id)
                .unwrap()
                .status
                != "needs_review"
        );
        assert!(
            revoked
                .runs
                .iter()
                .any(|run| Some(&run.id) == revoked.active_run_id.as_ref()
                    && run.status == "complete")
        );
    }
    assert!(
        audio_snapshot(service.root(), meeting) == audio_before,
        "refinement must not rewrite source FLAC bytes or manifest audio paths/hashes"
    );
    assert!(
        fs::read(&fixture).unwrap() == original_wave,
        "the public source fixture must remain unchanged"
    );
    assert!(
        !has_vector_payload(&serde_json::to_value(&current).unwrap()),
        "returned metadata must not expose embedding vectors"
    );
    let db = store::open(service.root()).unwrap();
    let mut query = db
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'app_%'")
        .unwrap();
    let tables = query
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    for table in tables {
        let mut columns = db
            .prepare(&format!(
                "PRAGMA table_info(\"{}\")",
                table.replace('"', "\"\"")
            ))
            .unwrap();
        let fields = columns
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            !fields.iter().any(|field| matches!(
                field.as_str(),
                "embedding"
                    | "embeddings"
                    | "embedding_blob"
                    | "embedding_json"
                    | "vector"
                    | "vectors"
            )),
            "embedding payload columns must not be created"
        );
    }
    drop(query);
    drop(db);
    service.wait_for_shutdown();
}

#[test]
fn refinement_publication_uses_speaker_turns_only_when_separation_is_configured() {
    publish_refined_recording(false);
    publish_refined_recording(true);
}

fn publish_refined_recording(speakers: bool) {
    use std::time::{Duration, Instant};
    use vt_core::{AudioBlock, FramePosition, StreamFormat};
    let fixture = crate::test_fixtures::path("vantranscribe-stt-fixture.wav");
    let refinement = crate::test_fixtures::path("vantranscribe-refinement-models")
        .join(vt_stt::REFINEMENT_MODEL_ID);
    let diarization =
        crate::test_fixtures::path("vt-diarization-models/verified").join(vt_diarize::MODEL_ID);
    assert!(
        fixture.is_file() && refinement.is_dir() && (!speakers || diarization.is_dir()),
        "explicit public fixture and pinned model caches must exist"
    );
    let temp = tempfile::TempDir::new_in("/private/tmp").unwrap();
    let root = temp.path().join("data");
    let models = root.join("models");
    hardlink_models(&refinement, &models.join(vt_stt::REFINEMENT_MODEL_ID));
    if speakers {
        hardlink_models(&diarization, &models.join(vt_diarize::MODEL_ID));
    }
    let service = AppService::new(root, temp.path().join("never-launched-capture-helper")).unwrap();
    {
        let mut prefs = service.preferences.lock().unwrap();
        prefs.auto_refine = false;
        prefs.auto_summarize = false;
        let mut audio = service.audio_preferences.lock().unwrap();
        audio.diarization_enabled = speakers;
        audio.echo_cancellation_enabled = false;
    }
    assert!(service.snapshot().unwrap().refinement_models.installed);
    assert_eq!(
        service.snapshot().unwrap().diarization_models.installed,
        speakers
    );
    let wave = sherpa_onnx::Wave::read(fixture.to_str().unwrap()).unwrap();
    let rate = wave.sample_rate() as u32;
    let meeting = if speakers {
        "public-refine-speakers"
    } else {
        "public-refine-no-speakers"
    };
    let mut recording = vt_record::Recording::create(service.root(), meeting).unwrap();
    store::create_meeting(
        service.root(),
        meeting,
        "Public refinement fixture",
        "remote",
    )
    .unwrap();
    let mut first = 0u64;
    for packet in wave.samples().chunks(rate as usize) {
        recording
            .write_chunk(
                vt_core::TrackKind::Mic,
                &AudioBlock::new(
                    StreamFormat::new(rate, 1).unwrap(),
                    FramePosition {
                        epoch: 0,
                        first_frame: first,
                        host_time_ns: first * 1_000_000_000 / u64::from(rate),
                    },
                    packet.to_vec(),
                )
                .unwrap(),
            )
            .unwrap();
        first += packet.len() as u64;
    }
    recording.finish().unwrap();
    store::save_segment(
        service.root(),
        meeting,
        &Segment {
            id: "live-source".into(),
            track: "mic".into(),
            start_ms: 0,
            end_ms: 500,
            text: "Live transcript that refinement must not copy forward.".into(),
            speaker: "Me".into(),
            words: vec![],
        },
    )
    .unwrap();
    let live = store::detail(service.root(), meeting)
        .unwrap()
        .active_run_id
        .unwrap();
    store::complete_live_run(service.root(), meeting).unwrap();
    store::set_status(service.root(), meeting, "ready", None).unwrap();
    service.refine(meeting).unwrap();
    let deadline = Instant::now() + Duration::from_secs(180);
    while service.snapshot().unwrap().processing.is_some() {
        if Instant::now() >= deadline {
            service.cancel_processing();
            panic!("refinement did not finish");
        }
        thread::sleep(Duration::from_millis(50));
    }
    let current = store::detail(service.root(), meeting).unwrap();
    assert_eq!(
        current.meeting.status, "ready",
        "{:?}",
        current.meeting.error
    );
    let active = current.active_run_id.clone().unwrap();
    assert_ne!(active, live);
    assert!(current.runs.iter().any(|run| {
        run.id == active
            && run.kind == "refine"
            && run.status == "complete"
            && run.engines_json.contains(vt_stt::REFINEMENT_MODEL_ID)
    }));
    assert!(
        current
            .runs
            .iter()
            .any(|run| run.id == live && run.kind == "live" && run.status == "complete")
    );
    let historical = service.detail_for_run(meeting, Some(&live)).unwrap();
    assert_eq!(
        historical.segments[0].text,
        "Live transcript that refinement must not copy forward."
    );
    assert!(
        current
            .segments
            .iter()
            .all(|segment| segment.text != historical.segments[0].text)
    );
    assert!(current.segments.iter().any(|segment| {
        segment
            .words
            .iter()
            .any(|word| word.end_ms >= word.start_ms && !word.text.is_empty())
    }));
    assert_word_bounds(&current.segments);
    if speakers {
        assert!(
            !current.speaker_turns.is_empty(),
            "configured separation must persist diarizer turns"
        );
        assert!(
            current
                .speaker_turns
                .iter()
                .all(|turn| !turn.provenance.is_empty())
        );
        let labels = current
            .speaker_turns
            .iter()
            .map(|turn| turn.label.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(
            current.segments.iter().all(|segment| {
                labels.contains(segment.speaker.as_str())
                    || segment.speaker == "Unknown"
                    || segment.speaker == "Overlap"
            }),
            "configured refinement must label segments from diarizer turns"
        );
    } else {
        assert!(
            current.speaker_turns.is_empty(),
            "unconfigured refinement must not invent speaker clusters"
        );
        assert!(current.diarization.is_none());
        assert!(
            current
                .segments
                .iter()
                .all(|segment| segment.speaker == "Me" || segment.speaker == "Them")
        );
    }
    service.wait_for_shutdown();
}

#[test]
fn disabled_speaker_model_preserves_existing_timed_user_names_on_new_transcript_run() {
    let temp = tempfile::TempDir::new().unwrap();
    let service =
        AppService::new(temp.path().to_path_buf(), temp.path().join("unused-helper")).unwrap();
    let recording = vt_record::Recording::create(service.root(), "retained-speaker").unwrap();
    store::create_meeting(
        service.root(),
        "retained-speaker",
        "Synthetic retention fixture",
        "remote",
    )
    .unwrap();
    let old = store::detail(service.root(), "retained-speaker")
        .unwrap()
        .active_run_id
        .unwrap();
    let audio_turn = turn("old-voice", "Speaker 1", 0, 1500);
    let derived = store::save_diarization_result(
        service.root(),
        "retained-speaker",
        &old,
        &vt_diarize::DiarizationResult {
            turns: vec![audio_turn],
            speakers: vec![],
            merges: vec![],
            constraint_reviews: vec![],
            segmentation_context: None,
            embedding_version: "synthetic".into(),
            calibrated: false,
            warnings: vec![],
        },
        false,
    )
    .unwrap();
    store::rename_speaker(
        service.root(),
        "retained-speaker",
        &derived[0].speaker_ids[0],
        "Alice",
    )
    .unwrap();
    store::save_segment(
        service.root(),
        "retained-speaker",
        &segment("Alpha", &[("Alpha", 100, 500)]),
    )
    .unwrap();
    store::complete_live_run(service.root(), "retained-speaker").unwrap();
    recording.finish().unwrap();
    let next = store::create_run(service.root(), "retained-speaker", "refine", "{}").unwrap();
    let retained = refine(&service, "retained-speaker", &next.id, true)
        .unwrap()
        .unwrap();
    assert_eq!(retained[0].label, "Alice");
    store::append_run_segments(
        service.root(),
        &next.id,
        &[segment("Alpha", &[("Alpha", 100, 500)])],
    )
    .unwrap();
    store::publish_run(service.root(), "retained-speaker", &next.id).unwrap();
    let detail = service.detail("retained-speaker").unwrap();
    assert_eq!(detail.segments[0].speaker, "Alice");
    assert!(
        detail
            .diarization
            .unwrap()
            .warnings
            .iter()
            .any(|warning| warning.contains("not rerun"))
    );
    assert_eq!(
        service
            .detail_for_run("retained-speaker", Some(&old))
            .unwrap()
            .segments[0]
            .speaker,
        "Alice"
    );
}

#[test]
fn disabled_model_fallback_retains_pending_constraint_review_and_publishes_complete_source() {
    let temp = tempfile::TempDir::new().unwrap();
    let service =
        AppService::new(temp.path().to_path_buf(), temp.path().join("unused-helper")).unwrap();
    let meeting = "pending-speaker-fallback";
    let recording = vt_record::Recording::create(service.root(), meeting).unwrap();
    store::create_meeting(
        service.root(),
        meeting,
        "Pending speaker correction fixture",
        "remote",
    )
    .unwrap();
    let old = service.detail(meeting).unwrap().active_run_id.unwrap();
    let left = turn("old-a", "Speaker 1", 0, 1500);
    let right = turn("old-b", "Speaker 2", 2000, 3500);
    store::save_diarization_result(
        service.root(),
        meeting,
        &old,
        &DiarizationResult {
            turns: vec![left.clone(), right.clone()],
            speakers: vec![],
            merges: vec![],
            constraint_reviews: vec![],
            segmentation_context: None,
            embedding_version: "synthetic".into(),
            calibrated: false,
            warnings: vec![],
        },
        false,
    )
    .unwrap();
    store::add_speaker_constraint(
        service.root(),
        meeting,
        &old,
        &left.id,
        &right.id,
        "cannot_link",
    )
    .unwrap();
    let constraint = store::speaker_constraints(service.root(), meeting)
        .unwrap()
        .remove(0);
    let unknown = |mut turn: SpeakerTurn| {
        turn.speaker_ids.clear();
        turn.label = "Unknown".into();
        turn.uncertain = true;
        turn.provenance = "constraint-review".into();
        turn
    };
    store::save_diarization_result(
        service.root(),
        meeting,
        &old,
        &DiarizationResult {
            turns: vec![unknown(left), unknown(right)],
            speakers: vec![],
            merges: vec![],
            segmentation_context: None,
            constraint_reviews: vec![vt_diarize::ConstraintReview {
                provenance: format!("user:{}", constraint.id),
                reason: "Synthetic unresolved saved anchors".into(),
                anchors: vec![
                    vt_diarize::Anchor {
                        track: "system".into(),
                        start_ms: 0,
                        end_ms: 1500,
                    },
                    vt_diarize::Anchor {
                        track: "system".into(),
                        start_ms: 2000,
                        end_ms: 3500,
                    },
                ],
            }],
            embedding_version: "synthetic".into(),
            calibrated: false,
            warnings: vec!["Affected regions remain Unknown.".into()],
        },
        false,
    )
    .unwrap();
    let source = Segment {
        id: "pending-source".into(),
        track: "system".into(),
        start_ms: 50,
        end_ms: 3500,
        text: "Alpha Beta".into(),
        speaker: "Unknown".into(),
        words: vec![
            TranscriptWord {
                text: "Alpha".into(),
                start_ms: 100,
                end_ms: 400,
            },
            TranscriptWord {
                text: "Beta".into(),
                start_ms: 2100,
                end_ms: 2500,
            },
        ],
    };
    store::save_segment(service.root(), meeting, &source).unwrap();
    store::complete_live_run(service.root(), meeting).unwrap();
    recording.finish().unwrap();
    let next = store::create_run(service.root(), meeting, "refine", "{}").unwrap();
    let retained = refine(&service, meeting, &next.id, true).unwrap().unwrap();
    assert!(
        retained
            .iter()
            .all(|turn| turn.uncertain && turn.speaker_ids.is_empty())
    );
    let parts = split_segment(source.clone(), &retained);
    assert_eq!(
        parts
            .iter()
            .map(|part| part.text.as_str())
            .collect::<String>(),
        source.text
    );
    store::append_run_segments(service.root(), &next.id, &parts).unwrap();
    store::publish_run(service.root(), meeting, &next.id).unwrap();
    let current = service.detail(meeting).unwrap();
    assert_eq!(current.active_run_id.as_deref(), Some(next.id.as_str()));
    assert!(
        current
            .runs
            .iter()
            .any(|run| run.id == next.id && run.status == "complete")
    );
    assert!(
        current
            .segments
            .iter()
            .all(|segment| segment.speaker == "Unknown")
    );
    assert_eq!(
        current
            .segments
            .iter()
            .map(|part| part.text.as_str())
            .collect::<String>(),
        "Alpha Beta"
    );
    assert_word_bounds(&current.segments);
    let review = current
        .speaker_reviews
        .iter()
        .find(|review| review.kind == "constraint")
        .expect("fallback must retain unresolved correction review");
    assert_eq!(
        serde_json::to_value(review).unwrap()["constraint_id"],
        constraint.id
    );
    assert_eq!(review.status, "needs_review");
    assert!(
        current
            .speaker_constraints
            .iter()
            .any(|entry| entry.id == constraint.id && entry.active)
    );
    assert!(!has_vector_payload(
        &serde_json::to_value(&current).unwrap()
    ));
    assert_eq!(
        service
            .detail_for_run(meeting, Some(&old))
            .unwrap()
            .segments[0]
            .text,
        "Alpha Beta"
    );
    service
        .revoke_speaker_constraint(meeting, &constraint.id)
        .unwrap();
    let revoked = service.detail(meeting).unwrap();
    assert!(
        revoked
            .speaker_reviews
            .iter()
            .filter(
                |review| serde_json::to_value(review).unwrap()["constraint_id"] == constraint.id
            )
            .all(|review| review.status != "needs_review")
    );
}

#[test]
fn speaker_capacity_fallback_retains_previous_timed_user_names_and_reviews() {
    let temp = tempfile::TempDir::new().unwrap();
    let service =
        AppService::new(temp.path().to_path_buf(), temp.path().join("unused-helper")).unwrap();
    let meeting = "capacity-retained-speaker";
    let recording = vt_record::Recording::create(service.root(), meeting).unwrap();
    store::create_meeting(
        service.root(),
        meeting,
        "Capacity retention fixture",
        "remote",
    )
    .unwrap();
    let old = service.detail(meeting).unwrap().active_run_id.unwrap();
    let derived = store::save_diarization_result(
        service.root(),
        meeting,
        &old,
        &DiarizationResult {
            turns: vec![turn("old-voice", "Speaker 1", 0, 1500)],
            speakers: vec![],
            merges: vec![],
            constraint_reviews: vec![],
            segmentation_context: None,
            embedding_version: "synthetic".into(),
            calibrated: false,
            warnings: vec![],
        },
        false,
    )
    .unwrap();
    store::rename_speaker(service.root(), meeting, &derived[0].speaker_ids[0], "Alice").unwrap();
    store::save_segment(
        service.root(),
        meeting,
        &segment("Alpha", &[("Alpha", 100, 500)]),
    )
    .unwrap();
    store::complete_live_run(service.root(), meeting).unwrap();
    recording.finish().unwrap();
    let previous = service.detail(meeting).unwrap();
    let next = store::create_run(service.root(), meeting, "refine", "{}").unwrap();
    let error = anyhow::Error::from(vt_diarize::DiarizeError::Capacity(
        vt_diarize::CapacityLimit::Turns,
    ));
    let retained = settle(&service, meeting, &next.id, Err(error))
        .unwrap()
        .expect("previous timed turns are carried forward");
    assert_eq!(retained[0].label, "Alice");
    store::append_run_segments(
        service.root(),
        &next.id,
        &[segment("Alpha", &[("Alpha", 100, 500)])],
    )
    .unwrap();
    store::publish_run(service.root(), meeting, &next.id).unwrap();
    let detail = service.detail(meeting).unwrap();
    assert_eq!(detail.active_run_id.as_deref(), Some(next.id.as_str()));
    assert_eq!(detail.segments[0].speaker, "Alice");
    assert_eq!(detail.speaker_turns.len(), previous.speaker_turns.len());
    assert_eq!(detail.speaker_reviews.len(), previous.speaker_reviews.len());
    let warnings = detail.diarization.unwrap().warnings;
    assert!(
        warnings.iter().any(|warning| warning
            == "Speaker separation could not finish for this recording (limit: turns). The refined transcript is complete. Speaker labels, names and corrections from the previous transcript were kept and applied by time."),
        "{warnings:?}"
    );
    assert!(!warnings.iter().any(|warning| warning.contains("not rerun")));
    assert_eq!(
        service
            .detail_for_run(meeting, Some(&old))
            .unwrap()
            .segments[0]
            .speaker,
        "Alice"
    );
}
