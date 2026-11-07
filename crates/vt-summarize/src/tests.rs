use super::*;
use std::sync::Mutex;
fn config() -> SummaryConfig {
    SummaryConfig {
        provider: Provider::Apple,
        ..Default::default()
    }
}
fn cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}
fn segments(count: usize) -> Vec<SummarySegment> {
    (0..count)
        .map(|i| SummarySegment {
            id: format!("immutable-{i}"),
            text: format!(
                "Synthetic source {i}. {}",
                "A team discusses a bounded fictional example. ".repeat(8)
            ),
        })
        .collect()
}
struct Fake {
    calls: Mutex<Vec<GenerationRequest>>,
    refuse_first: bool,
    invalid_first: bool,
    budget: usize,
}
impl Fake {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            refuse_first: false,
            invalid_first: false,
            budget: usize::MAX,
        }
    }
}
impl Summarizer for Fake {
    fn generate(
        &self,
        request: &GenerationRequest,
        cancel: &Arc<AtomicBool>,
    ) -> Result<SummaryContent> {
        check_cancel(cancel)?;
        let mut calls = self.calls.lock().unwrap();
        let first = calls.is_empty();
        calls.push(request.clone());
        drop(calls);
        if first && self.refuse_first {
            return Err(SummaryError::Refused);
        }
        if request.input.len() > self.budget {
            return Err(SummaryError::ContextBudget);
        }
        let ids: Vec<String> = if first && self.invalid_first {
            vec!["invented".into()]
        } else {
            request
                .input
                .lines()
                .flat_map(|line| {
                    serde_json::from_str::<serde_json::Value>(line).unwrap()["source_ids"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|x| x.as_str().unwrap().to_string())
                        .collect::<Vec<_>>()
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        };
        Ok(SummaryContent {
            key_points: vec![CitedItem {
                text: "A synthetic grounded note.".into(),
                source_ids: ids,
            }],
            ..Default::default()
        })
    }
}
#[test]
fn small_single_pass_maps_citations_and_never_embeds_real_ids() {
    let fake = Fake::new();
    let result = summarize_with(&config(), &segments(1), &fake, cancel(), |_| {}).unwrap();
    assert!(!result.partial);
    assert_eq!(result.citations, vec!["immutable-0"]);
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
    assert!(!fake.calls.lock().unwrap()[0].input.contains("immutable"));
    assert!(result.content_json.contains("immutable-0"));
    assert!(result.markdown.contains("sources: immutable-0"));
}
#[test]
fn long_input_uses_bounded_map_and_recursive_reduce() {
    let fake = Fake::new();
    let result = summarize_with(&config(), &segments(160), &fake, cancel(), |_| {}).unwrap();
    assert!(!result.partial, "{:?}", result.errors);
    assert_eq!(result.citations.len(), 160);
    let calls = fake.calls.lock().unwrap();
    assert!(calls.len() > 20);
    assert!(calls.iter().all(|r| r.input.len() <= 2600));
    assert!(calls.iter().all(|r| r.max_output_tokens == 600));
}
#[test]
fn refusal_retries_neutrally_exactly_once() {
    let fake = Fake {
        refuse_first: true,
        ..Fake::new()
    };
    let result = summarize_with(&config(), &segments(1), &fake, cancel(), |_| {}).unwrap();
    assert!(!result.partial);
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(calls[1].retry);
    assert!(calls[1].instructions.contains("neutral factual"));
}
#[test]
fn invented_citations_do_not_publish_and_all_failed_is_error() {
    let fake = Fake {
        invalid_first: true,
        ..Fake::new()
    };
    assert!(summarize_with(&config(), &segments(1), &fake, cancel(), |_| {}).is_err());
    let fake = Fake {
        invalid_first: true,
        ..Fake::new()
    };
    let result = summarize_with(&config(), &segments(12), &fake, cancel(), |_| {}).unwrap();
    assert!(result.partial);
    assert!(!result.citations.contains(&"invented".into()));
    assert!(result.markdown.contains("Partial summary"));
}
#[test]
fn adaptive_context_split_preserves_immutable_ids() {
    let fake = Fake {
        budget: 350,
        ..Fake::new()
    };
    let result = summarize_with(&config(), &segments(2), &fake, cancel(), |_| {}).unwrap();
    assert_eq!(result.citations, vec!["immutable-0", "immutable-1"]);
    assert!(fake.calls.lock().unwrap().len() > 2);
}
#[test]
fn unicode_and_json_escaping_stay_in_budget() {
    let mut inputs = segments(2);
    inputs[0].text = "\"\\\n🦊".repeat(4000);
    let fake = Fake::new();
    let result = summarize_with(&config(), &inputs, &fake, cancel(), |_| {}).unwrap();
    assert_eq!(result.citations.len(), 2);
    assert!(
        fake.calls
            .lock()
            .unwrap()
            .iter()
            .all(|x| x.input.len() <= 2600)
    );
}
#[test]
fn cancellation_prevents_calls_and_works_between_chunks() {
    let fake = Fake::new();
    let flag = cancel();
    flag.store(true, Ordering::Relaxed);
    assert!(matches!(
        summarize_with(&config(), &segments(20), &fake, flag, |_| {}),
        Err(SummaryError::Cancelled)
    ));
    assert!(fake.calls.lock().unwrap().is_empty());
    let flag = cancel();
    let trigger = flag.clone();
    assert!(matches!(
        summarize_with(&config(), &segments(20), &fake, flag, move |_| trigger
            .store(true, Ordering::Relaxed)),
        Err(SummaryError::Cancelled)
    ));
}
#[test]
fn cloud_requires_explicit_consent_and_config_contains_no_secret() {
    let cloud = SummaryConfig {
        provider: Provider::OpenAi,
        model: Some("synthetic".into()),
        ..Default::default()
    };
    assert!(matches!(
        validate_config(&cloud),
        Err(SummaryError::Config(_))
    ));
    let value = serde_json::to_value(cloud).unwrap();
    assert!(value.get("secret").is_none());
    assert!(value.get("api_key").is_none());
}
#[test]
fn local_classification_and_cloud_transport_are_strict() {
    for endpoint in [
        "http://example.invalid/v1",
        "http://192.168.1.1/v1",
        "http://localhost/v1",
        "http://127.0.0.1.example.invalid/v1",
        "http://secret@127.0.0.1/v1",
        "http://127.0.0.1/v1?key=secret",
    ] {
        let c = SummaryConfig {
            provider: Provider::Local,
            endpoint: Some(endpoint.into()),
            ..Default::default()
        };
        assert!(providers::endpoint(&c).is_err(), "{endpoint}");
    }
    for endpoint in ["http://127.0.0.1:11434/v1", "http://[::1]:1234/v1"] {
        let c = SummaryConfig {
            provider: Provider::Local,
            endpoint: Some(endpoint.into()),
            ..Default::default()
        };
        assert!(providers::endpoint(&c).is_ok());
    }
    let c = SummaryConfig {
        provider: Provider::Compatible,
        endpoint: Some("http://example.invalid/v1".into()),
        ..Default::default()
    };
    assert!(providers::endpoint(&c).is_err());
}
#[test]
fn malformed_json_and_missing_schema_fields_are_rejected() {
    assert!(serde_json::from_str::<SummaryContent>("{}").is_err());
    assert!(serde_json::from_str::<SummaryContent>("not json").is_err());
    let mut value = serde_json::to_value(SummaryContent::default()).unwrap();
    value["extra"] = true.into();
    assert!(serde_json::from_value::<SummaryContent>(value).is_err());
}
#[test]
fn markdown_escapes_links_and_html_from_model_output() {
    let content = SummaryContent {
        key_points: vec![CitedItem {
            text: "<script> [unsafe](https://invalid.example)".into(),
            source_ids: vec!["1".into()],
        }],
        ..Default::default()
    };
    let rendered = render_markdown(&content, false);
    assert!(!rendered.contains("<script>"));
    assert!(rendered.contains("\\[unsafe\\]"));
}

#[test]
fn citations_from_another_known_chunk_are_rejected() {
    struct CrossChunk;
    impl Summarizer for CrossChunk {
        fn generate(&self, _: &GenerationRequest, _: &Arc<AtomicBool>) -> Result<SummaryContent> {
            Ok(SummaryContent {
                key_points: vec![CitedItem {
                    text: "Synthetic".into(),
                    source_ids: vec!["2".into()],
                }],
                ..Default::default()
            })
        }
    }
    let config = config();
    let ids = BTreeMap::from([("1".into(), "real-a".into()), ("2".into(), "real-b".into())]);
    assert!(matches!(
        generate_valid(
            &CrossChunk,
            &config,
            "{\"source_ids\":[\"1\"],\"text\":\"Synthetic\"}",
            &ids,
            &cancel()
        ),
        Err(SummaryError::InvalidOutput)
    ));
}

#[test]
fn repeat_refusal_only_makes_two_calls_and_ends_as_error() {
    struct Refusal(Mutex<usize>);
    impl Summarizer for Refusal {
        fn generate(&self, _: &GenerationRequest, _: &Arc<AtomicBool>) -> Result<SummaryContent> {
            *self.0.lock().unwrap() += 1;
            Err(SummaryError::Refused)
        }
    }
    let fake = Refusal(Mutex::new(0));
    assert!(summarize_with(&config(), &segments(1), &fake, cancel(), |_| {}).is_err());
    assert_eq!(*fake.0.lock().unwrap(), 2);
}
