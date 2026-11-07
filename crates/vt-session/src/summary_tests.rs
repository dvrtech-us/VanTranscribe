//! Synthetic service-level summary tests. No capture, recording input, real
//! meeting storage, cloud request, or saved provider credential is used.
#![cfg(target_os = "macos")]

use super::*;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    thread,
    time::Duration,
};
use tempfile::TempDir;
use vt_summarize::{CitedItem, Provider, SummaryConfig, SummaryContent};

const MEETING: &str = "synthetic-summary-meeting";
const FIRST: &str = "immutable-synthetic-segment-a";
const SECOND: &str = "immutable-synthetic-segment-b";

fn fixture(helper: PathBuf) -> (TempDir, Arc<AppService>) {
    let temp = TempDir::new().unwrap();
    let service = AppService::new(temp.path().join("data"), helper).unwrap();
    // This creates an empty synthetic manifest, never opens a microphone or file
    // from a user's recordings folder, and supplies final transcript rows directly.
    let recording = vt_record::Recording::create(service.root(), MEETING).unwrap();
    store::create_meeting(
        service.root(),
        MEETING,
        "Synthetic summary fixture",
        "remote",
    )
    .unwrap();
    for (id, start_ms, text) in [
        (
            FIRST,
            0,
            "The team decided to use blue labels. The coordinator will prepare a sample by Friday.",
        ),
        (
            SECOND,
            2000,
            "The team asked whether a red label was needed. No red-label decision was made.",
        ),
    ] {
        store::save_segment(
            service.root(),
            MEETING,
            &Segment {
                id: id.into(),
                track: "mic".into(),
                start_ms,
                end_ms: start_ms + 1500,
                text: text.into(),
                speaker: "Synthetic participant".into(),
                words: Vec::new(),
            },
        )
        .unwrap();
    }
    store::complete_live_run(service.root(), MEETING).unwrap();
    recording.finish().unwrap();
    store::set_status(service.root(), MEETING, "ready", None).unwrap();
    (temp, service)
}

fn wait_finished(service: &AppService, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if service.snapshot().unwrap().processing.is_none() {
            return;
        }
        assert!(Instant::now() < deadline, "summary worker did not finish");
        thread::sleep(Duration::from_millis(20));
    }
}

/// One-request fake OpenAI-compatible server. Its port is ephemeral, so the
/// selected local origin has no configured provider credential. Responses are
/// gated so tests can correct/cancel after the worker snapshots its transcript.
struct LocalFixture {
    endpoint: String,
    received: mpsc::Receiver<std::result::Result<Value, String>>,
    reply: Option<mpsc::Sender<String>>,
    worker: Option<thread::JoinHandle<()>>,
}
impl LocalFixture {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        let (request_tx, received) = mpsc::channel();
        let (reply, reply_rx) = mpsc::channel::<String>();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return;
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            };
            match read_request(&mut stream) {
                Ok(body) => {
                    let _ = request_tx.send(Ok(body));
                    if let Ok(response) = reply_rx.recv_timeout(Duration::from_secs(10)) {
                        let _ = stream.write_all(response.as_bytes());
                    }
                }
                Err(error) => {
                    let _ = request_tx.send(Err(format!(
                        "synthetic HTTP fixture read failed: {:?}",
                        error.kind()
                    )));
                }
            }
        });
        Self {
            endpoint,
            received,
            reply: Some(reply),
            worker: Some(worker),
        }
    }
    fn configure(&self, service: &AppService) {
        service
            .update_preferences(
                Preferences {
                    auto_refine: false,
                    auto_summarize: false,
                    summary: SummaryConfig {
                        provider: Provider::Local,
                        endpoint: Some(self.endpoint.clone()),
                        model: Some("synthetic-fixture-model".into()),
                        ..Default::default()
                    },
                },
                None,
                false,
            )
            .unwrap();
    }
    fn request(&self, service: &AppService) -> Value {
        match self.received.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(body)) => body,
            result => {
                wait_finished(service, Duration::from_secs(5));
                let error = service.detail(MEETING).unwrap().summary_error;
                panic!(
                    "synthetic local provider request failed: {result:?}; summary_error={error:?}"
                );
            }
        }
    }
    fn respond(&mut self, response: String) {
        self.reply.take().unwrap().send(response).unwrap();
    }
}
impl Drop for LocalFixture {
    fn drop(&mut self) {
        self.reply.take();
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}
fn read_request(stream: &mut TcpStream) -> std::io::Result<Value> {
    // Darwin accepted sockets inherit the listener's nonblocking flag. Clear it
    // before reading so packet arrival timing cannot produce spurious WouldBlock.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut length = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if line == "\r\n" {
            break;
        }
        if line.is_empty() {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        if let Some((key, value)) = line.split_once(':')
            && key.eq_ignore_ascii_case("content-length")
        {
            length = value
                .trim()
                .parse()
                .map_err(|_| std::io::ErrorKind::InvalidData)?;
        }
        // Do not log headers or accidentally display a credential on failure.
        if line.to_ascii_lowercase().starts_with("authorization:") {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
    }
    if length > 64 * 1024 {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(|_| std::io::ErrorKind::InvalidData.into())
}
fn notes_response() -> String {
    let content = SummaryContent {
        decisions: vec![CitedItem {
            text: "Synthetic blue-label decision".into(),
            source_ids: vec!["1".into()],
        }],
        open_questions: vec![CitedItem {
            text: "Synthetic unresolved red-label question".into(),
            source_ids: vec!["2".into()],
        }],
        ..Default::default()
    };
    let body=json!({"choices":[{"message":{"content":serde_json::to_string(&content).unwrap()},"finish_reason":"stop"}]}).to_string();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}
fn generate_success(service: &Arc<AppService>) {
    let mut local = LocalFixture::new();
    local.configure(service);
    service.generate_summary(MEETING).unwrap();
    let request = local.request(service);
    assert_eq!(request["model"], "synthetic-fixture-model");
    let input = request["messages"][1]["content"].as_str().unwrap();
    assert!(!input.contains(FIRST));
    assert!(!input.contains(SECOND));
    assert!(input.contains("Synthetic participant"));
    assert!(
        request["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("untrusted")
    );
    local.respond(notes_response());
    wait_finished(service, Duration::from_secs(10));
    assert_eq!(service.detail(MEETING).unwrap().meeting.status, "ready");
}

#[test]
fn local_summary_service_persists_citations_metadata_and_restart() {
    let (_temp, service) = fixture(PathBuf::from("unused-synthetic-capture-helper"));
    let original = service.detail(MEETING).unwrap();
    generate_success(&service);
    let detail = service.detail(MEETING).unwrap();
    assert_eq!(detail.summaries.len(), 1);
    let saved = &detail.summaries[0];
    assert_eq!(saved.run_id, original.active_run_id.unwrap());
    assert_eq!(saved.correction_version, original.correction_version);
    assert_eq!(saved.provider, "local");
    assert_eq!(saved.model, "synthetic-fixture-model");
    assert_eq!(saved.template, "general");
    assert!(!saved.is_stale);
    assert_eq!(
        saved.source_segment_ids,
        vec![FIRST.to_string(), SECOND.to_string()]
    );
    let envelope: Value = serde_json::from_str(&saved.content_json).unwrap();
    assert_eq!(envelope["partial"], false);
    assert_eq!(envelope["errors"], json!([]));
    assert_eq!(
        envelope["content"]["decisions"][0]["source_ids"],
        json!([FIRST])
    );
    assert_eq!(
        envelope["content"]["open_questions"][0]["source_ids"],
        json!([SECOND])
    );
    assert!(
        detail
            .summary
            .as_ref()
            .unwrap()
            .contains("Synthetic blue-label decision")
    );
    assert!(
        service
            .search("blue-label")
            .unwrap()
            .iter()
            .any(|meeting| meeting.id == MEETING)
    );
    let root = service.root.clone();
    let helper = service.helper.clone();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Arc::strong_count(&service) > 1 {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    drop(service);
    let restarted = AppService::new(root, helper).unwrap();
    let restored = restarted.detail(MEETING).unwrap();
    assert_eq!(restored.summaries[0].content_json, saved.content_json);
    assert_eq!(
        restored.summaries[0].source_segment_ids,
        saved.source_segment_ids
    );
}

#[test]
fn provider_failure_preserves_the_previous_summary_and_citations() {
    let (_temp, service) = fixture(PathBuf::from("unused-synthetic-capture-helper"));
    generate_success(&service);
    store::set_status(
        service.root(),
        MEETING,
        "recovered",
        Some("Synthetic source transcript warning."),
    )
    .unwrap();
    let before = service.detail(MEETING).unwrap();
    let mut local = LocalFixture::new();
    local.configure(&service);
    service.generate_summary(MEETING).unwrap();
    local.request(&service);
    local.respond(
        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            .into(),
    );
    wait_finished(&service, Duration::from_secs(10));
    let after = service.detail(MEETING).unwrap();
    assert_eq!(after.summaries.len(), 1);
    assert_eq!(after.summaries[0].id, before.summaries[0].id);
    assert_eq!(after.summary, before.summary);
    assert_eq!(
        after.summaries[0].source_segment_ids,
        before.summaries[0].source_segment_ids
    );
    assert!(!after.summaries[0].is_stale);
    assert_eq!(after.meeting.status, before.meeting.status);
    assert_eq!(after.meeting.error, before.meeting.error);
    assert!(
        after
            .summary_error
            .as_deref()
            .is_some_and(|error| error.contains("HTTP status"))
    );
}

#[test]
fn correction_during_generation_rejects_stale_publication_without_losing_prior_notes() {
    let (_temp, service) = fixture(PathBuf::from("unused-synthetic-capture-helper"));
    generate_success(&service);
    let before = service.detail(MEETING).unwrap();
    let mut local = LocalFixture::new();
    local.configure(&service);
    service.generate_summary(MEETING).unwrap();
    local.request(&service);
    service
        .correct_segment(
            MEETING,
            before.active_run_id.as_deref().unwrap(),
            FIRST,
            "The coordinator will prepare a synthetic sample by Monday.",
        )
        .unwrap();
    local.respond(notes_response());
    wait_finished(&service, Duration::from_secs(10));
    let after = service.detail(MEETING).unwrap();
    assert_eq!(after.correction_version, before.correction_version + 1);
    assert_eq!(after.summaries.len(), 1);
    assert_eq!(after.summaries[0].id, before.summaries[0].id);
    assert_eq!(
        after.summaries[0].content_md,
        before.summaries[0].content_md
    );
    assert!(after.summaries[0].is_stale);
    assert!(after.summary.is_none());
    assert_eq!(after.meeting.status, before.meeting.status);
    assert_eq!(after.meeting.error, before.meeting.error);
    assert!(
        after
            .summary_error
            .as_deref()
            .is_some_and(|error| error.contains("Transcript changed"))
    );
    assert_eq!(
        after.segments[0].text,
        "The coordinator will prepare a synthetic sample by Monday."
    );
}

#[test]
fn cancelling_summary_restores_source_status_and_warning_and_preserves_prior_notes() {
    let (_temp, service) = fixture(PathBuf::from("unused-synthetic-capture-helper"));
    generate_success(&service);
    store::set_status(
        service.root(),
        MEETING,
        "recovered",
        Some("Synthetic source warning before cancellation."),
    )
    .unwrap();
    let before = service.detail(MEETING).unwrap();
    let local = LocalFixture::new();
    local.configure(&service);
    service.generate_summary(MEETING).unwrap();
    local.request(&service);
    service.cancel_processing();
    // Hold the response: cancellation must end the inflight request itself.
    wait_finished(&service, Duration::from_secs(10));
    drop(local);
    let after = service.detail(MEETING).unwrap();
    assert_eq!(after.meeting.status, before.meeting.status);
    assert_eq!(after.meeting.error, before.meeting.error);
    assert_eq!(after.correction_version, before.correction_version);
    assert_eq!(after.summaries.len(), 1);
    assert_eq!(after.summaries[0].id, before.summaries[0].id);
    assert_eq!(
        after.summaries[0].source_segment_ids,
        before.summaries[0].source_segment_ids
    );
    assert_eq!(after.summary, before.summary);
    assert!(!after.summaries[0].is_stale);
    assert!(
        after
            .summary_error
            .as_deref()
            .is_some_and(|error| error.to_ascii_lowercase().contains("cancel"))
    );
}

#[test]
fn successful_summary_preserves_source_warning_and_marks_json_and_markdown_partial() {
    let (_temp, service) = fixture(PathBuf::from("unused-synthetic-capture-helper"));
    let source_warning = "Synthetic source transcript processing warning.";
    store::set_status(service.root(), MEETING, "ready", Some(source_warning)).unwrap();
    generate_success(&service);
    let detail = service.detail(MEETING).unwrap();
    assert_eq!(detail.meeting.status, "ready");
    assert_eq!(detail.meeting.error.as_deref(), Some(source_warning));
    assert!(detail.summary_error.is_none());
    let saved = &detail.summaries[0];
    let envelope: Value = serde_json::from_str(&saved.content_json).unwrap();
    assert_eq!(envelope["partial"], true);
    assert!(!envelope["errors"].as_array().unwrap().is_empty());
    assert!(saved.content_md.contains("Partial summary"));
    assert_eq!(detail.summary.as_deref(), Some(saved.content_md.as_str()));
    assert!(!saved.is_stale);
}

#[test]
fn rename_and_manual_note_preserve_summary_and_do_not_change_transcript_revision() {
    let (_temp, service) = fixture(PathBuf::from("unused-synthetic-capture-helper"));
    generate_success(&service);
    let before = service.detail(MEETING).unwrap();
    service
        .rename(MEETING, "Renamed synthetic summary fixture")
        .unwrap();
    service
        .save_note(MEETING, "Synthetic manual bookmark")
        .unwrap();
    let after = service.detail(MEETING).unwrap();
    assert_eq!(after.correction_version, before.correction_version);
    assert_eq!(after.active_run_id, before.active_run_id);
    assert_eq!(after.summaries.len(), 1);
    assert_eq!(after.summaries[0].id, before.summaries[0].id);
    assert!(!after.summaries[0].is_stale);
    assert_eq!(after.summary, before.summary);
    assert_eq!(
        after.summaries[0].source_segment_ids,
        before.summaries[0].source_segment_ids
    );
    assert_eq!(after.meeting.title, "Renamed synthetic summary fixture");
    assert_eq!(after.notes[0].text, "Synthetic manual bookmark");
    assert!(after.summary_error.is_none());
}

#[test]
fn rename_and_note_during_generation_do_not_reject_the_current_transcript_snapshot() {
    let (_temp, service) = fixture(PathBuf::from("unused-synthetic-capture-helper"));
    let before = service.detail(MEETING).unwrap();
    let mut local = LocalFixture::new();
    local.configure(&service);
    service.generate_summary(MEETING).unwrap();
    local.request(&service);
    service
        .rename(MEETING, "Synthetic title changed during generation")
        .unwrap();
    service
        .save_note(MEETING, "Synthetic bookmark added during generation")
        .unwrap();
    local.respond(notes_response());
    wait_finished(&service, Duration::from_secs(10));
    let after = service.detail(MEETING).unwrap();
    assert_eq!(after.correction_version, before.correction_version);
    assert_eq!(after.meeting.status, "ready");
    assert!(after.meeting.error.is_none());
    assert!(after.summary_error.is_none());
    assert_eq!(after.summaries.len(), 1);
    assert!(!after.summaries[0].is_stale);
    assert_eq!(
        after.summaries[0].correction_version,
        before.correction_version
    );
    assert!(after.summary.is_some());
}

#[test]
#[ignore = "opt-in on-device Apple service smoke; requires VT_RUN_APPLE_SUMMARY_SMOKE=1 and the built helper"]
fn actual_apple_summary_service_persists_current_correction_version_and_sources() {
    assert_eq!(
        std::env::var("VT_RUN_APPLE_SUMMARY_SMOKE").as_deref(),
        Ok("1"),
        "Explicit synthetic Apple smoke opt-in is required."
    );
    let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../src-tauri/resources/vt-summary-helper");
    assert!(
        helper.is_file(),
        "Build src-tauri/resources/vt-summary-helper first."
    );
    assert!(
        vt_summarize::apple_availability(&helper).unwrap().available,
        "The on-device model is unavailable; no OS features are enabled by this test."
    );
    let (_temp, service) = fixture(helper.with_file_name("unused-synthetic-capture-helper"));
    let initial = service.detail(MEETING).unwrap();
    service
        .correct_segment(
            MEETING,
            initial.active_run_id.as_deref().unwrap(),
            FIRST,
            "The team decided to use blue labels. The coordinator will prepare a sample by Monday.",
        )
        .unwrap();
    let snapshot = service.detail(MEETING).unwrap();
    service
        .update_preferences(
            Preferences {
                auto_refine: false,
                auto_summarize: false,
                summary: SummaryConfig {
                    provider: Provider::Apple,
                    ..Default::default()
                },
            },
            None,
            false,
        )
        .unwrap();
    service.generate_summary(MEETING).unwrap();
    wait_finished(&service, Duration::from_secs(150));
    let detail = service.detail(MEETING).unwrap();
    assert_eq!(
        detail.meeting.status, "ready",
        "Synthetic Apple generation failed."
    );
    assert_eq!(detail.summaries.len(), 1);
    let saved = &detail.summaries[0];
    assert_eq!(saved.provider, "apple");
    assert_eq!(saved.run_id, snapshot.active_run_id.unwrap());
    assert_eq!(saved.correction_version, snapshot.correction_version);
    assert!(!saved.is_stale);
    assert!(!saved.source_segment_ids.is_empty());
    assert!(
        saved
            .source_segment_ids
            .iter()
            .all(|id| id == FIRST || id == SECOND)
    );
    let envelope: Value = serde_json::from_str(&saved.content_json).unwrap();
    assert!(envelope["content"].is_object());
    assert_eq!(envelope["partial"], false);
    assert_eq!(envelope["errors"], json!([]));
    assert!(detail.summary.is_some());
}

#[test]
fn confirmed_bleed_is_excluded_from_summary_input_and_restore_invalidates_notes_without_losing_sources()
 {
    let (_temp, service) = fixture(PathBuf::from("unused-synthetic-capture-helper"));
    generate_success(&service);
    let before = service.detail(MEETING).unwrap();
    let run = before.active_run_id.clone().unwrap();
    service.runtime.lock().unwrap().processing = true;
    assert!(
        service
            .set_bleed_decision(MEETING, &run, FIRST, "bleed")
            .is_err()
    );
    assert_eq!(
        service.detail(MEETING).unwrap().correction_version,
        before.correction_version
    );
    service.runtime.lock().unwrap().processing = false;
    service
        .set_bleed_decision(MEETING, &run, FIRST, "bleed")
        .unwrap();
    assert!(service.detail(MEETING).unwrap().summaries[0].is_stale);
    let mut local = LocalFixture::new();
    local.configure(&service);
    service.generate_summary(MEETING).unwrap();
    let request = local.request(&service);
    let input = request["messages"][1]["content"].as_str().unwrap();
    assert!(!input.contains("The coordinator will prepare a sample"));
    assert!(input.contains("whether a red label was needed"));
    let content = SummaryContent {
        tldr: vec![CitedItem {
            text: "Synthetic visible-source note".into(),
            source_ids: vec!["1".into()],
        }],
        ..Default::default()
    };
    let body=json!({"choices":[{"message":{"content":serde_json::to_string(&content).unwrap()},"finish_reason":"stop"}]}).to_string();
    local.respond(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body));
    wait_finished(&service, Duration::from_secs(10));
    let hidden = service.detail(MEETING).unwrap();
    assert_eq!(
        hidden.segments.len(),
        2,
        "detail/JSON retain hidden source speech"
    );
    let current = hidden.summaries.iter().find(|s| !s.is_stale).unwrap();
    assert_eq!(current.source_segment_ids, vec![SECOND.to_string()]);
    let generated = current.id.clone();
    service
        .set_bleed_decision(MEETING, &run, FIRST, "genuine")
        .unwrap();
    assert!(
        service
            .detail(MEETING)
            .unwrap()
            .summaries
            .iter()
            .find(|s| s.id == generated)
            .unwrap()
            .is_stale
    );
    generate_success(&service);
    let restored = service.detail(MEETING).unwrap();
    assert_eq!(
        restored
            .summaries
            .iter()
            .find(|s| !s.is_stale)
            .unwrap()
            .source_segment_ids,
        vec![FIRST.to_string(), SECOND.to_string()]
    );
    assert_eq!(restored.segments[0].text, before.segments[0].text);
    service
        .set_bleed_decision(MEETING, &run, FIRST, "bleed")
        .unwrap();
    service
        .set_bleed_decision(MEETING, &run, SECOND, "bleed")
        .unwrap();
    assert!(
        service
            .generate_summary(MEETING)
            .unwrap_err()
            .to_string()
            .contains("No visible speech")
    );
    assert!(service.snapshot().unwrap().processing.is_none());
}
