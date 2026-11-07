//! Explicit synthetic-only on-device integration check. Never reads meeting storage.
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};
use vt_summarize::{Provider, SummaryConfig, SummarySegment, summarize};
fn main() {
    let helper = std::env::args()
        .nth(1)
        .expect("usage: synthetic /path/to/vt-summary-helper");
    let config = SummaryConfig {
        provider: Provider::Apple,
        ..Default::default()
    };
    let mut input=vec![SummarySegment{id:"synthetic-1".into(),text:"In a fictional planning meeting, the team decided to use blue labels. The coordinator will prepare a sample by Friday.".into()},SummarySegment{id:"synthetic-2".into(),text:"The team asked whether the sample should include a red label. No decision was made on the red label.".into()}];
    if std::env::args().any(|arg| arg == "--long") {
        input=(0..24).map(|i|SummarySegment {
            id:format!("synthetic-long-{i}"),
            text:format!("In fictional topic {i}, the team decided to use blue labels for the sample. The coordinator will prepare the sample by Friday. The team asked whether a red label was needed but made no decision about red labels.")
        }).collect();
    }
    match summarize(
        &config,
        &input,
        Some(Path::new(&helper)),
        Arc::new(AtomicBool::new(false)),
        |_| {},
    ) {
        Ok(result) => println!("{}", serde_json::to_string_pretty(&result).unwrap()),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
