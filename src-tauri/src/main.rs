#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{Manager, State};
use tauri_plugin_notification::NotificationExt;
use vt_session::{
    AppService, AudioPreferences, CaptureMode, DeletionKind, DeletionPreview, LiveModel,
    LiveModelCatalog, MaintenancePolicy, MaintenanceReport, Meeting, MeetingDetail, PolicyPreview,
    Preferences, Snapshot, SummaryConfig, Waveform,
};

struct DesktopState(Arc<AppService>, Arc<AtomicBool>, Arc<AtomicBool>);
type ApiResult<T> = Result<T, String>;
fn api_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[tauri::command]
fn frontend_ready(state: State<'_, DesktopState>) -> bool {
    if !state.1.swap(true, Ordering::Relaxed) {
        println!("VanTranscribe interface connected");
    }
    true
}

#[tauri::command]
async fn app_snapshot(state: State<'_, DesktopState>) -> ApiResult<Snapshot> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.snapshot())
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
async fn update_meeting_detection(
    state: State<'_, DesktopState>,
    settings: vt_session::DetectionSettings,
    context_generation: u64,
) -> ApiResult<Snapshot> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.update_meeting_detection_for_generation(settings, context_generation)
    })
    .await
    .map_err(api_error)?
    .map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
async fn set_detection_context(
    state: State<'_, DesktopState>,
    mode: CaptureMode,
    setup_open: bool,
    context_generation: u64,
) -> ApiResult<Snapshot> {
    let service = state.0.clone();
    let snapshot = tauri::async_runtime::spawn_blocking(move || {
        service.set_detection_context_for_generation(mode, setup_open, context_generation)
    })
    .await
    .map_err(api_error)?
    .map_err(api_error)?;
    if !state.2.swap(true, Ordering::Relaxed) {
        println!("VanTranscribe detection context connected");
    }
    Ok(snapshot)
}
#[tauri::command(rename_all = "snake_case")]
async fn start_detected_recording(
    state: State<'_, DesktopState>,
    episode_id: String,
    title: String,
    consent: bool,
    input_device: Option<String>,
) -> ApiResult<Snapshot> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.start_detected_recording(
            &episode_id,
            title,
            consent,
            input_device.filter(|s| !s.is_empty()),
        )
    })
    .await
    .map_err(api_error)?
    .map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
async fn dismiss_detection(
    state: State<'_, DesktopState>,
    episode_id: String,
    suppress: bool,
) -> ApiResult<Snapshot> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.dismiss_detection(&episode_id, suppress))
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
async fn dismiss_detection_ended(
    state: State<'_, DesktopState>,
    episode_id: String,
) -> ApiResult<Snapshot> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.dismiss_detection_ended(&episode_id))
        .await
        .map_err(api_error)?
        .map_err(api_error)
}

#[tauri::command]
async fn live_model_catalog(state: State<'_, DesktopState>) -> ApiResult<LiveModelCatalog> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.live_model_catalog())
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command]
async fn select_live_model(
    state: State<'_, DesktopState>,
    model: LiveModel,
) -> ApiResult<Snapshot> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.select_live_model(model))
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command]
async fn maintenance_policy(state: State<'_, DesktopState>) -> ApiResult<MaintenancePolicy> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.maintenance_policy())
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command]
async fn save_maintenance_policy(
    state: State<'_, DesktopState>,
    policy: MaintenancePolicy,
) -> ApiResult<MaintenancePolicy> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.save_maintenance_policy(policy))
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command]
async fn preview_deletion(
    state: State<'_, DesktopState>,
    id: String,
    kind: DeletionKind,
) -> ApiResult<DeletionPreview> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.preview_deletion(&id, kind))
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command]
async fn commit_deletion(
    state: State<'_, DesktopState>,
    token: String,
) -> ApiResult<MaintenanceReport> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.commit_deletion(&token))
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command]
async fn preview_cleanup(state: State<'_, DesktopState>) -> ApiResult<PolicyPreview> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.preview_cleanup())
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command]
async fn apply_cleanup(
    state: State<'_, DesktopState>,
    preview: PolicyPreview,
) -> ApiResult<MaintenanceReport> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.apply_cleanup(preview))
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command]
async fn last_maintenance_report(state: State<'_, DesktopState>) -> ApiResult<MaintenanceReport> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.last_maintenance_report())
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
fn start_recording(
    state: State<'_, DesktopState>,
    title: String,
    mode: CaptureMode,
    consent_confirmed: bool,
    input_device: Option<String>,
) -> ApiResult<Snapshot> {
    state
        .0
        .start(
            title,
            mode,
            consent_confirmed,
            input_device.filter(|s| !s.is_empty()),
        )
        .map_err(api_error)
}
#[tauri::command]
fn pause_recording(state: State<'_, DesktopState>) -> ApiResult<Snapshot> {
    state.0.pause().map_err(api_error)
}
#[tauri::command]
fn resume_recording(state: State<'_, DesktopState>) -> ApiResult<Snapshot> {
    state.0.resume().map_err(api_error)
}
#[tauri::command]
fn stop_recording(state: State<'_, DesktopState>) -> ApiResult<Snapshot> {
    state.0.stop().map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
async fn meeting_detail(
    state: State<'_, DesktopState>,
    id: String,
    run_id: Option<String>,
) -> ApiResult<MeetingDetail> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.detail_for_run(&id, run_id.as_deref()))
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command]
async fn search_meetings(state: State<'_, DesktopState>, query: String) -> ApiResult<Vec<Meeting>> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.search(&query))
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
fn correct_segment(
    state: State<'_, DesktopState>,
    id: String,
    run_id: String,
    segment_id: String,
    replacement: String,
) -> ApiResult<()> {
    state
        .0
        .correct_segment(&id, &run_id, &segment_id, &replacement)
        .map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
fn set_bleed_decision(
    state: State<'_, DesktopState>,
    id: String,
    run_id: String,
    segment_id: String,
    decision: String,
) -> ApiResult<()> {
    state
        .0
        .set_bleed_decision(&id, &run_id, &segment_id, &decision)
        .map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
fn resolve_bleed_review(
    state: State<'_, DesktopState>,
    id: String,
    run_id: String,
    decision_id: String,
) -> ApiResult<()> {
    state
        .0
        .resolve_bleed_review(&id, &run_id, &decision_id)
        .map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
fn resolve_correction(
    state: State<'_, DesktopState>,
    id: String,
    correction_id: String,
    segment_id: Option<String>,
) -> ApiResult<()> {
    state
        .0
        .resolve_correction(&id, &correction_id, segment_id.as_deref())
        .map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
fn rename_speaker(
    state: State<'_, DesktopState>,
    id: String,
    speaker_id: String,
    name: String,
) -> ApiResult<()> {
    state
        .0
        .rename_speaker(&id, &speaker_id, &name)
        .map_err(api_error)
}
#[tauri::command]
async fn voice_memory_status(
    state: State<'_, DesktopState>,
) -> ApiResult<vt_session::MemoryStatus> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.voice_memory_status())
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
async fn update_voice_memory(
    state: State<'_, DesktopState>,
    enabled: bool,
    retention_months: u32,
    explanation_acknowledged: bool,
    context_generation: u64,
) -> ApiResult<vt_session::MemoryStatus> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.update_voice_memory(
            enabled,
            retention_months,
            explanation_acknowledged,
            context_generation,
        )
    })
    .await
    .map_err(api_error)?
    .map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
async fn enroll_voice(
    state: State<'_, DesktopState>,
    request: vt_session::EnrollVoiceRequest,
    context_generation: u64,
) -> ApiResult<vt_session::MemoryStatus> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.enroll_voice(request, context_generation))
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
async fn forget_person(
    state: State<'_, DesktopState>,
    person_id: String,
    remove_names: bool,
) -> ApiResult<vt_session::MemoryStatus> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.forget_person(&person_id, remove_names))
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
async fn delete_all_voiceprints(
    state: State<'_, DesktopState>,
    remove_names: bool,
) -> ApiResult<vt_session::MemoryStatus> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.delete_all_voiceprints(remove_names))
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
async fn set_speaker_shared_room(
    state: State<'_, DesktopState>,
    id: String,
    speaker_id: String,
    shared_room: bool,
) -> ApiResult<MeetingDetail> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.set_speaker_shared_room(&id, &speaker_id, shared_room)
    })
    .await
    .map_err(api_error)?
    .map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
fn add_speaker_constraint(
    state: State<'_, DesktopState>,
    id: String,
    run_id: String,
    left_turn_id: String,
    right_turn_id: String,
    kind: String,
) -> ApiResult<()> {
    state
        .0
        .add_speaker_constraint(&id, &run_id, &left_turn_id, &right_turn_id, &kind)
        .map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
fn resolve_speaker_review(
    state: State<'_, DesktopState>,
    id: String,
    run_id: String,
    review_id: String,
    previous_id: Option<String>,
    target_id: Option<String>,
) -> ApiResult<()> {
    state
        .0
        .resolve_speaker_review(
            &id,
            &run_id,
            &review_id,
            previous_id.as_deref(),
            target_id.as_deref(),
        )
        .map_err(api_error)
}
#[tauri::command]
fn refine_meeting(state: State<'_, DesktopState>, id: String) -> ApiResult<()> {
    state.0.refine(&id).map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
fn revoke_speaker_constraint(
    state: State<'_, DesktopState>,
    id: String,
    constraint_id: String,
) -> ApiResult<()> {
    state
        .0
        .revoke_speaker_constraint(&id, &constraint_id)
        .map_err(api_error)
}
#[tauri::command]
fn generate_summary(state: State<'_, DesktopState>, id: String) -> ApiResult<()> {
    state.0.generate_summary(&id).map_err(api_error)
}
#[tauri::command]
fn cancel_processing(state: State<'_, DesktopState>) {
    state.0.cancel_processing();
}
#[tauri::command]
fn install_refinement_model(state: State<'_, DesktopState>) -> ApiResult<()> {
    state.0.install_refinement_model().map_err(api_error)
}
#[tauri::command]
fn cancel_refinement_model_download(state: State<'_, DesktopState>) {
    state.0.cancel_refinement_model_download();
}
#[tauri::command]
fn install_vad_model(state: State<'_, DesktopState>) -> ApiResult<()> {
    state.0.install_vad_model().map_err(api_error)
}
#[tauri::command]
fn cancel_vad_model_download(state: State<'_, DesktopState>) {
    state.0.cancel_vad_model_download();
}
#[tauri::command(rename_all = "snake_case")]
fn install_diarization_model(
    state: State<'_, DesktopState>,
    acknowledge_terms: bool,
) -> ApiResult<()> {
    state
        .0
        .install_diarization_model(acknowledge_terms)
        .map_err(api_error)
}
#[tauri::command]
fn cancel_diarization_model_download(state: State<'_, DesktopState>) {
    state.0.cancel_diarization_model_download();
}
#[tauri::command(rename_all = "snake_case")]
fn update_preferences(
    state: State<'_, DesktopState>,
    auto_refine: bool,
    auto_summarize: bool,
    summary_config: SummaryConfig,
    api_key: Option<String>,
    clear_key: Option<bool>,
) -> ApiResult<Snapshot> {
    state
        .0
        .update_preferences(
            Preferences {
                auto_refine,
                auto_summarize,
                summary: summary_config,
            },
            api_key.as_deref(),
            clear_key.unwrap_or(false),
        )
        .map_err(api_error)
}
#[tauri::command]
async fn summary_capabilities(state: State<'_, DesktopState>) -> ApiResult<serde_json::Value> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.summary_capabilities())
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command]
fn save_note(state: State<'_, DesktopState>, id: String, text: String) -> ApiResult<()> {
    state.0.save_note(&id, &text).map_err(api_error)
}
#[tauri::command]
fn rename_meeting(state: State<'_, DesktopState>, id: String, title: String) -> ApiResult<()> {
    state.0.rename(&id, &title).map_err(api_error)
}
#[tauri::command]
fn export_meeting(state: State<'_, DesktopState>, id: String, format: String) -> ApiResult<String> {
    state
        .0
        .export(&id, &format)
        .map(|p| p.display().to_string())
        .map_err(api_error)
}
#[tauri::command]
fn install_model(state: State<'_, DesktopState>) -> ApiResult<()> {
    state.0.install_model().map_err(api_error)
}
#[tauri::command]
fn cancel_model_download(state: State<'_, DesktopState>) {
    state.0.cancel_model_download();
}
#[tauri::command]
fn transcribe_meeting(state: State<'_, DesktopState>, id: String) -> ApiResult<()> {
    state.0.transcribe(&id).map_err(api_error)
}
#[tauri::command]
async fn input_devices(state: State<'_, DesktopState>) -> ApiResult<serde_json::Value> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service
            .input_devices()
            .and_then(|d| Ok(serde_json::to_value(d)?))
    })
    .await
    .map_err(api_error)?
    .map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
async fn play_meeting_audio(
    state: State<'_, DesktopState>,
    id: String,
    position_ms: Option<u64>,
) -> ApiResult<Snapshot> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.play_audio(&id, position_ms))
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command]
async fn pause_meeting_audio(state: State<'_, DesktopState>) -> ApiResult<Snapshot> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.pause_audio())
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
async fn seek_meeting_audio(
    state: State<'_, DesktopState>,
    id: String,
    position_ms: u64,
) -> ApiResult<Snapshot> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.seek_audio(&id, position_ms))
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command]
async fn stop_meeting_audio(state: State<'_, DesktopState>) -> ApiResult<Snapshot> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.stop_audio())
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
async fn set_playback_volumes(
    state: State<'_, DesktopState>,
    id: String,
    mic_volume: f32,
    system_volume: f32,
) -> ApiResult<Snapshot> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.set_playback_volumes(&id, mic_volume, system_volume)
    })
    .await
    .map_err(api_error)?
    .map_err(api_error)
}
#[tauri::command]
async fn meeting_waveform(
    state: State<'_, DesktopState>,
    id: String,
    bins: Option<usize>,
) -> ApiResult<Waveform> {
    let service = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || service.audio_waveform(&id, bins.unwrap_or(512)))
        .await
        .map_err(api_error)?
        .map_err(api_error)
}
#[tauri::command(rename_all = "snake_case")]
fn update_audio_preferences(
    state: State<'_, DesktopState>,
    live_vad_enabled: bool,
    diarization_enabled: bool,
    echo_cancellation_enabled: Option<bool>,
) -> ApiResult<Snapshot> {
    let echo_cancellation_enabled = match echo_cancellation_enabled {
        Some(value) => value,
        None => {
            state
                .0
                .snapshot()
                .map_err(api_error)?
                .settings
                .echo_cancellation_enabled
        }
    };
    state
        .0
        .update_audio_preferences(AudioPreferences {
            live_vad_enabled,
            diarization_enabled,
            echo_cancellation_enabled,
        })
        .map_err(api_error)
}
#[tauri::command]
fn open_data_folder(state: State<'_, DesktopState>) -> ApiResult<()> {
    let status = std::process::Command::new("open")
        .arg(state.0.root())
        .status()
        .map_err(api_error)?;
    if status.success() {
        Ok(())
    } else {
        Err("Could not open the recordings folder.".into())
    }
}

fn helper_path(app: &tauri::App) -> PathBuf {
    let resource = app
        .path()
        .resource_dir()
        .unwrap_or_default()
        .join("vt-capture-helper");
    if resource.is_file() {
        resource
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/vt-capture-helper")
    }
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .on_page_load(|webview, payload| {
            if payload.event() == tauri::webview::PageLoadEvent::Started
                && let Some(state) = webview.try_state::<DesktopState>()
            {
                state.2.store(false, Ordering::Relaxed);
                state.0.reset_detection_context();
            }
        })
        .setup(|app| {
            let root = std::env::var_os("VANTRANSCRIBE_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or(app.path().app_data_dir()?);
            let service = AppService::new(root, helper_path(app))?;
            service.start_maintenance()?;
            let reminder_app = app.handle().clone();
            service.set_detection_notifier(Arc::new(move |title, body| {
                // Delivery is governed by OS notification settings. The same
                // advisory stays in the app; notifications never invoke capture.
                let _ = reminder_app
                    .notification()
                    .builder()
                    .title(title)
                    .body(body)
                    .show();
            }));
            let action_app = app.handle().clone();
            app.notification().on_action(move |_| {
                if let Some(window) = action_app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            })?;
            service.start_meeting_detection()?;
            let ui_ready = Arc::new(AtomicBool::new(false));
            #[cfg(debug_assertions)]
            let args: Vec<String> = std::env::args().collect();
            #[cfg(debug_assertions)]
            if args.iter().any(|arg| arg == "--capture-check")
                && args.iter().any(|arg| arg == "--consent-confirmed")
                && std::env::var("VANTRANSCRIBE_CAPTURE_CHECK").as_deref() == Ok("1")
            {
                let check_service = service.clone();
                let ready = ui_ready.clone();
                std::thread::spawn(move || capture_check(check_service, args, ready));
            }
            app.manage(DesktopState(
                service,
                ui_ready,
                Arc::new(AtomicBool::new(false)),
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            frontend_ready,
            app_snapshot,
            update_meeting_detection,
            set_detection_context,
            start_detected_recording,
            dismiss_detection,
            dismiss_detection_ended,
            live_model_catalog,
            select_live_model,
            maintenance_policy,
            save_maintenance_policy,
            preview_deletion,
            commit_deletion,
            preview_cleanup,
            apply_cleanup,
            last_maintenance_report,
            start_recording,
            pause_recording,
            resume_recording,
            stop_recording,
            meeting_detail,
            search_meetings,
            correct_segment,
            set_bleed_decision,
            resolve_bleed_review,
            resolve_correction,
            rename_speaker,
            voice_memory_status,
            update_voice_memory,
            enroll_voice,
            forget_person,
            delete_all_voiceprints,
            set_speaker_shared_room,
            add_speaker_constraint,
            resolve_speaker_review,
            revoke_speaker_constraint,
            refine_meeting,
            generate_summary,
            cancel_processing,
            install_refinement_model,
            cancel_refinement_model_download,
            install_vad_model,
            cancel_vad_model_download,
            install_diarization_model,
            cancel_diarization_model_download,
            update_preferences,
            summary_capabilities,
            save_note,
            rename_meeting,
            export_meeting,
            install_model,
            cancel_model_download,
            transcribe_meeting,
            input_devices,
            play_meeting_audio,
            pause_meeting_audio,
            seek_meeting_audio,
            stop_meeting_audio,
            set_playback_volumes,
            meeting_waveform,
            update_audio_preferences,
            open_data_folder
        ])
        .build(tauri::generate_context!())
        .expect("VanTranscribe could not initialize");
    app.run(|handle, event| {
        if let tauri::RunEvent::ExitRequested {
            code: None, api, ..
        } = event
            && let Some(state) = handle.try_state::<DesktopState>()
        {
            api.prevent_exit();
            let service = state.0.clone();
            let handle = handle.clone();
            std::thread::spawn(move || {
                service.wait_for_shutdown();
                handle.exit(0);
            });
        }
    });
}

// Explicit diagnostic mode exercises the same service used by the UI. It never
// starts on ordinary launch, and requires an affirmative command-line consent
// flag. macOS privacy authorization is still enforced by the operating system.
#[cfg(debug_assertions)]
fn capture_check(service: Arc<AppService>, args: Vec<String>, ui_ready: Arc<AtomicBool>) {
    let option = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let mic = option("--mic-device");
    let seconds = option("--seconds")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(12)
        .clamp(3, 120);
    let report = option("--report").map(PathBuf::from).filter(|path| {
        path.parent()
            .and_then(|parent| parent.canonicalize().ok())
            .is_some_and(|parent| parent.starts_with(service.root()))
            && !path.exists()
    });
    let result = (|| -> Result<serde_json::Value, String> {
        let ready_deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while !ui_ready.load(Ordering::Relaxed) {
            if std::time::Instant::now() >= ready_deadline {
                return Err("The packaged interface did not finish initializing.".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let snapshot = service
            .start(
                "Capture verification".into(),
                CaptureMode::Remote,
                true,
                mic,
            )
            .map_err(api_error)?;
        let id = snapshot.recording.ok_or("Recording did not start")?.id;
        for _ in 0..seconds * 10 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let snapshot = service.snapshot().map_err(api_error)?;
            if snapshot
                .recording
                .as_ref()
                .is_some_and(|r| r.status == "failed")
            {
                return serde_json::to_value(snapshot).map_err(api_error);
            }
        }
        service.stop().map_err(api_error)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(40);
        while std::time::Instant::now() < deadline {
            let snapshot = service.snapshot().map_err(api_error)?;
            if snapshot.recording.is_none() {
                break;
            }
            if snapshot
                .recording
                .as_ref()
                .is_some_and(|r| r.status == "failed")
            {
                return serde_json::to_value(snapshot).map_err(api_error);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        serde_json::to_value(service.detail(&id).map_err(api_error)?).map_err(api_error)
    })();
    let mut value = match result {
        Ok(value) => value,
        Err(error) => serde_json::json!({"error":error}),
    };
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "ui_ready".into(),
            serde_json::Value::Bool(ui_ready.load(Ordering::Relaxed)),
        );
    }
    if let Some(path) = report {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
        {
            let _ = file.write_all(&serde_json::to_vec_pretty(&value).unwrap_or_default());
        }
    }
    println!(
        "Capture check finished: ui_ready={}, status={}, transcript_lines={}",
        ui_ready.load(Ordering::Relaxed),
        value
            .get("meeting")
            .and_then(|m| m.get("status"))
            .and_then(|s| s.as_str())
            .unwrap_or("failed"),
        value
            .get("segments")
            .and_then(|s| s.as_array())
            .map_or(0, Vec::len)
    );
}
