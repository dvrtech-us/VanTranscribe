//! User-initiated biometric enrollment from retained, refined source speech.
//! The WebView supplies consent and identity choices, never embeddings or keys.
use super::*;
use anyhow::Context;
use chrono::Utc;
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use vt_speakers::{ChannelType, KeyProvider, MemorySettings, MemoryStatus, SecretKey};
use zeroize::Zeroizing;

const MAX_CLEAN_MS: u64 = 120_000;
const MIN_CLEAN_MS: u64 = 20_000;
const SECOND: u64 = 1_000_000_000;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollVoiceRequest {
    pub meeting_id: String,
    pub run_id: String,
    pub speaker_id: String,
    pub display_name: String,
    pub person_id: Option<String>,
    pub is_self: bool,
    pub written_consent: bool,
    pub consent_method: String,
    pub attested_by: String,
    pub channel_type: Option<ChannelType>,
}

fn idle(service: &AppService, runtime: &Runtime) -> Result<()> {
    if service.shutting_down.load(Ordering::Relaxed) {
        bail!("VanTranscribe is closing.");
    }
    if runtime.processing
        || runtime.current.as_ref().is_some_and(|c| {
            matches!(
                c.view.status.as_str(),
                "recording" | "paused" | "processing"
            )
        })
    {
        bail!("Finish recording and processing before changing speaker memory.");
    }
    Ok(())
}
fn current_generation(
    service: &AppService,
    generation: u64,
) -> Result<std::sync::MutexGuard<'_, meeting_detection::DetectionState>> {
    let context = service
        .meeting_detection
        .lock()
        .map_err(|_| anyhow::anyhow!("Window context unavailable."))?;
    if context.context_generation != generation {
        bail!("This window has been replaced. Reopen speaker memory and retry.");
    }
    Ok(context)
}
fn cancelled(service: &AppService) -> Result<()> {
    if service.shutting_down.load(Ordering::Relaxed)
        || service.cancel_processing.load(Ordering::Relaxed)
    {
        bail!("Voice enrollment cancelled. No voiceprint was saved.");
    }
    Ok(())
}
struct Reservation<'a>(&'a AppService);
impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        let mut runtime = self.0.runtime.lock().unwrap_or_else(|e| e.into_inner());
        runtime.saved_audio_readers = runtime.saved_audio_readers.saturating_sub(1);
        runtime.processing = false;
        runtime.processing_view = None;
    }
}
impl AppService {
    /// No secret access. Expired biometric payloads are removed even with Keychain locked.
    pub fn voice_memory_status(&self) -> Result<MemoryStatus> {
        let mut db = store::open(&self.root)?;
        let tx = db.transaction()?;
        vt_speakers::expire(&tx, Utc::now())?;
        let result = vt_speakers::status(&tx, Some(&vt_diarize::voice_embedding_version()))?;
        tx.commit()?;
        Ok(result)
    }
    pub fn update_voice_memory(
        &self,
        enabled: bool,
        retention_months: u32,
        explanation_acknowledged: bool,
        context_generation: u64,
    ) -> Result<MemoryStatus> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("Session state unavailable."))?;
        // Turning memory off can invalidate an in-flight enrollment; enabling or
        // changing retention waits for the existing processing reservation.
        if enabled {
            idle(self, &runtime)?;
        } else if self.shutting_down.load(Ordering::Relaxed) {
            bail!("VanTranscribe is closing.");
        }
        let _context = current_generation(self, context_generation)?;
        let mut db = store::open(&self.root)?;
        let tx = db.transaction()?;
        vt_speakers::update_settings(
            &tx,
            &MemorySettings {
                enabled,
                retention_months,
            },
            explanation_acknowledged,
            Utc::now(),
        )?;
        vt_speakers::expire(&tx, Utc::now())?;
        let result = vt_speakers::status(&tx, Some(&vt_diarize::voice_embedding_version()))?;
        tx.commit()?;
        Ok(result)
    }
    pub fn enroll_voice(
        &self,
        request: EnrollVoiceRequest,
        context_generation: u64,
    ) -> Result<MemoryStatus> {
        let provider = vt_speakers::KeychainProvider::new(&self.root)?;
        self.enroll_voice_with(request, context_generation, &provider, || {
            let path = vt_diarize::ModelManager::new(self.root.join("models")).model_dir();
            Ok(Box::new(vt_diarize::VoiceEmbedder::new(&path)?) as Box<dyn EmbeddingEngine>)
        })
    }
    fn enroll_voice_with(
        &self,
        request: EnrollVoiceRequest,
        context_generation: u64,
        provider: &dyn KeyProvider,
        create: impl FnOnce() -> Result<Box<dyn EmbeddingEngine>>,
    ) -> Result<MemoryStatus> {
        validate_request(&request)?;
        {
            let mut runtime = self
                .runtime
                .lock()
                .map_err(|_| anyhow::anyhow!("Session state unavailable."))?;
            idle(self, &runtime)?;
            let _context = current_generation(self, context_generation)?;
            self.ensure_saved_audio(&request.meeting_id)?;
            let db = store::open(&self.root)?;
            if !vt_speakers::settings(&db)?.enabled {
                bail!("Enable speaker memory and acknowledge the biometric explanation first.");
            }
            runtime.saved_audio_readers = runtime
                .saved_audio_readers
                .checked_add(1)
                .context("Too many source readers.")?;
            runtime.processing = true;
            runtime.processing_view = Some(ProcessingView {
                meeting_id: request.meeting_id.clone(),
                kind: "voice_memory".into(),
                stage: "Preparing consented voice enrollment".into(),
                progress: 0.0,
                error: None,
            });
            self.cancel_processing.store(false, Ordering::Relaxed);
        }
        let _reservation = Reservation(self);
        let selected_source = selection(self, &request)?;
        let mut engine = create()?;
        let (embedding, clean_speech_ms) =
            recompute(self, &request, &selected_source, engine.as_mut())?;
        let mut embedding = Zeroizing::new(embedding);
        cancelled(self)?;
        // Keychain access can prompt/block. It occurs without runtime, detection,
        // or SQLite transaction locks. Only a prepared in-memory key is used in
        // the final atomic, generation-checked publication.
        {
            let _runtime = self
                .runtime
                .lock()
                .map_err(|_| anyhow::anyhow!("Session state unavailable."))?;
            let _context = current_generation(self, context_generation)?;
            if !vt_speakers::settings(&store::open(&self.root)?)?.enabled {
                bail!("Speaker memory was turned off. No voiceprint was saved.");
            }
        }
        let db = store::open(&self.root)?;
        let key = prepare_key(&db, provider)?;
        drop(db);
        cancelled(self)?;
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("Session state unavailable."))?;
        let _context = current_generation(self, context_generation)?;
        if !runtime.processing
            || runtime
                .processing_view
                .as_ref()
                .is_none_or(|p| p.kind != "voice_memory" || p.meeting_id != request.meeting_id)
        {
            bail!("Voice enrollment reservation was lost.");
        }
        cancelled(self)?;
        let fresh = selection(self, &request)?;
        if fresh.fingerprint != selected_source.fingerprint {
            bail!("The speaker or source changed. Refresh the meeting and retry enrollment.");
        }
        let mut db = store::open(&self.root)?;
        let tx = db.transaction()?;
        let enrollment = vt_speakers::Enrollment {
            display_name: request.display_name.trim().into(),
            person_id: request.person_id.clone(),
            is_self: request.is_self,
            written_consent: request.written_consent,
            consent_method: request.consent_method.trim().into(),
            attested_by: request.attested_by.trim().into(),
            channel_type: selected_source.channel,
            channel_provenance: if selected_source.channel == ChannelType::InPerson {
                "meeting_mode"
            } else {
                "user_declared"
            }
            .into(),
            embedding_version: vt_diarize::voice_embedding_version(),
            clean_speech_ms,
            shared_room: false,
            embedding: std::mem::take(&mut *embedding),
        };
        let person = vt_speakers::enroll(&tx, &PreparedKey(key), &enrollment, Utc::now())?;
        store::link_voice_person(
            &tx,
            &request.meeting_id,
            &request.speaker_id,
            &person.id,
            &person.display_name,
        )?;
        let result = vt_speakers::status(&tx, Some(&vt_diarize::voice_embedding_version()))?;
        tx.commit()?;
        Ok(result)
    }
    pub fn forget_person(&self, person_id: &str, remove_names: bool) -> Result<MemoryStatus> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("Session state unavailable."))?;
        idle(self, &runtime)?;
        let mut db = store::open(&self.root)?;
        let tx = db.transaction()?;
        store::forget_person_links(&tx, &[person_id.into()], remove_names)?;
        vt_speakers::forget_person(&tx, person_id)?;
        let result = vt_speakers::status(&tx, Some(&vt_diarize::voice_embedding_version()))?;
        tx.commit()?;
        Ok(result)
    }
    pub fn delete_all_voiceprints(&self, remove_names: bool) -> Result<MemoryStatus> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("Session state unavailable."))?;
        idle(self, &runtime)?;
        let mut db = store::open(&self.root)?;
        let tx = db.transaction()?;
        let ids = {
            let mut q = tx.prepare("SELECT id FROM app_people")?;
            q.query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        store::forget_person_links(&tx, &ids, remove_names)?;
        vt_speakers::delete_all(&tx)?;
        let result = vt_speakers::status(&tx, Some(&vt_diarize::voice_embedding_version()))?;
        tx.commit()?;
        Ok(result)
    }
}
fn validate_request(request: &EnrollVoiceRequest) -> Result<()> {
    if request.display_name.trim().is_empty()
        || request.display_name.chars().count() > 120
        || request.display_name.len() > 200
    {
        bail!("Enter a speaker name of at most 120 characters.");
    }
    if request.attested_by.trim().is_empty()
        || request.attested_by.chars().count() > 120
        || request.attested_by.len() > 200
    {
        bail!("Enter who is attesting to this consent (at most 120 characters).");
    }
    if !request.written_consent || request.consent_method.trim().is_empty() {
        bail!("Saving another person's voice requires their written consent and its method.");
    }
    if request.consent_method.len() > 1000 {
        bail!("Consent method must contain at most 1000 characters.");
    }
    Ok(())
}
struct PreparedKey(SecretKey);
impl KeyProvider for PreparedKey {
    fn load(&self) -> vt_speakers::Result<Option<SecretKey>> {
        Ok(Some(self.0.clone()))
    }
    fn create(&self) -> vt_speakers::Result<SecretKey> {
        Err(vt_speakers::MemoryError::KeyUnavailable)
    }
}
fn prepare_key(db: &Connection, provider: &dyn KeyProvider) -> Result<SecretKey> {
    if let Some(key) = provider.load()? {
        return Ok(key);
    }
    let existing: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM app_voiceprints)", [], |r| {
        r.get(0)
    })?;
    // The store's authenticated keycheck remains authoritative even after expiry;
    // do not create a replacement key for any previously initialized key slot.
    let initialized: bool = db.query_row(
        "SELECT key_check IS NOT NULL FROM app_voice_memory_settings WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    if existing || initialized {
        return Err(vt_speakers::MemoryError::KeyUnavailable.into());
    }
    Ok(provider.create()?)
}

trait EmbeddingEngine {
    fn extract(&mut self, samples: &[f32], rate: u32) -> Result<Vec<f32>>;
}
impl EmbeddingEngine for vt_diarize::VoiceEmbedder {
    fn extract(&mut self, samples: &[f32], rate: u32) -> Result<Vec<f32>> {
        Ok(self.extract(samples, rate)?)
    }
}

#[derive(Clone, Debug, Serialize)]
struct SourceChunk {
    epoch: u32,
    first_frame: u64,
    frames: u64,
    rate: u32,
    start: u64,
    end: u64,
    ready: bool,
    clipped: u64,
    hash: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Span {
    start: u64,
    end: u64,
    rate: u32,
    epoch: u32,
}
struct Selection {
    chunks: Vec<SourceChunk>,
    spans: Vec<Span>,
    track: TrackKind,
    channel: ChannelType,
    fingerprint: String,
}
fn unsigned(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}
fn source_chunks(db: &Connection, meeting: &str, track: TrackKind) -> Result<Vec<SourceChunk>> {
    let mut q=db.prepare("SELECT epoch,first_frame,frame_count,sample_rate,start_ns,end_ns,state='ready' AND kind='audio',clipped_samples,sha256 FROM audio_chunks WHERE meeting_id=?1 AND track=?2 ORDER BY start_ns,seq LIMIT 100001")?;
    let chunks = q
        .query_map(params![meeting, track.as_str()], |r| {
            Ok(SourceChunk {
                epoch: r.get(0)?,
                first_frame: unsigned(r, 1)?,
                frames: unsigned(r, 2)?,
                rate: r.get(3)?,
                start: unsigned(r, 4)?,
                end: unsigned(r, 5)?,
                ready: r.get(6)?,
                clipped: unsigned(r, 7)?,
                hash: r.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if chunks.len() > 100_000 {
        bail!("The source exceeds the supported manifest bound.");
    }
    Ok(chunks)
}
fn selection(service: &AppService, request: &EnrollVoiceRequest) -> Result<Selection> {
    let detail = store::detail(&service.root, &request.meeting_id)?;
    if detail.active_run_id.as_deref() != Some(&request.run_id)
        || detail.run_id.as_deref() != Some(&request.run_id)
        || !detail
            .runs
            .iter()
            .any(|r| r.id == request.run_id && r.kind == "refine" && r.status == "complete")
    {
        bail!("Remember voice requires the current completed refined transcript.");
    }
    let speaker = detail
        .speakers
        .iter()
        .find(|s| s.id == request.speaker_id)
        .context("Choose a speaker from this transcript.")?;
    if speaker.provisional || speaker.shared_room {
        bail!("Provisional and shared-room speakers cannot create voiceprints.");
    }
    if detail.diarization.as_ref().is_none_or(|d| {
        d.embedding_version != vt_diarize::EMBEDDING_VERSION
            || d.segmentation_context != Some(vt_diarize::Phase::Refined.segmentation_context())
    }) {
        bail!("Refine with the current installed speaker model before remembering this voice.");
    }
    let mode = store::meeting_mode(&service.root, &request.meeting_id)?;
    let (track, channel) = match mode {
        vt_diarize::MeetingMode::InPerson
            if speaker.track == "mic"
                && request
                    .channel_type
                    .is_none_or(|c| c == ChannelType::InPerson) =>
        {
            (TrackKind::Mic, ChannelType::InPerson)
        }
        vt_diarize::MeetingMode::Remote if speaker.track == "system" => {
            match request.channel_type {
                Some(c @ ChannelType::RemoteWideband) | Some(c @ ChannelType::RemoteNarrowband) => {
                    (TrackKind::System, c)
                }
                _ => bail!("Choose the remote source's known channel type before enrollment."),
            }
        }
        _ => bail!("This speaker has no compatible acoustic enrollment channel."),
    };
    let db = store::open(&service.root)?;
    let chunks = source_chunks(&db, &request.meeting_id, track)?;
    let turns = &detail.speaker_turns;
    let spans = select_spans(turns, &request.speaker_id, track, &chunks, channel)?;
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(
            &serde_json::json!({"run":request.run_id,"speaker":speaker,"turns":turns,"chunks":chunks,"corrections":detail.correction_version})
        )?)
    );
    Ok(Selection {
        chunks,
        spans,
        track,
        channel,
        fingerprint,
    })
}
fn select_spans(
    turns: &[vt_diarize::SpeakerTurn],
    speaker: &str,
    track: TrackKind,
    chunks: &[SourceChunk],
    channel: ChannelType,
) -> Result<Vec<Span>> {
    for (index, c) in chunks.iter().enumerate() {
        if c.end <= c.start
            || (index > 0 && chunks[index - 1].end > c.start)
            || (c.ready
                && (!(8_000..=192_000).contains(&c.rate)
                    || c.frames == 0
                    || c.frames > u64::from(c.rate) * 2
                    || c.end - c.start != c.frames * SECOND / u64::from(c.rate)))
        {
            bail!("The source manifest has invalid or overlapping frame geometry.");
        }
    }
    let mut islands: Vec<Span> = Vec::new();
    let mut previous: Option<&SourceChunk> = None;
    for c in chunks {
        if c.ready
            && c.clipped == 0
            && (8_000..=192_000).contains(&c.rate)
            && c.end > c.start
            && c.frames > 0
        {
            let adjacent = previous.is_some_and(|p| {
                p.ready
                    && p.clipped == 0
                    && p.epoch == c.epoch
                    && p.rate == c.rate
                    && p.end == c.start
                    && p.first_frame.checked_add(p.frames) == Some(c.first_frame)
            });
            if adjacent {
                islands.last_mut().context("Missing source island.")?.end = c.end;
            } else {
                islands.push(Span {
                    start: c.start,
                    end: c.end,
                    rate: c.rate,
                    epoch: c.epoch,
                });
            }
        }
        previous = Some(c);
    }
    let mut output = Vec::new();
    let mut total = 0;
    let mut ordered = turns
        .iter()
        .filter(|t| t.track == track.as_str())
        .collect::<Vec<_>>();
    ordered.sort_by_key(|t| (t.start_ms, t.end_ms, t.id.as_str()));
    let mut prior_end = 0;
    for (index, turn) in ordered.iter().enumerate() {
        let conflicting = prior_end > turn.start_ms
            || ordered
                .get(index + 1)
                .is_some_and(|next| next.start_ms < turn.end_ms);
        prior_end = prior_end.max(turn.end_ms);
        if turn.track != track.as_str()
            || turn.speaker_ids.as_slice() != [speaker]
            || turn.overlap
            || turn.uncertain
            || turn.provisional
            || turn.end_ms <= turn.start_ms
            || !matches!(turn.provenance.as_str(), "refined-cluster" | "user")
        {
            continue;
        }
        if conflicting {
            continue;
        }
        let begin = turn
            .start_ms
            .checked_mul(1_000_000)
            .context("Speaker clock overflow.")?;
        let end = turn
            .end_ms
            .checked_mul(1_000_000)
            .context("Speaker clock overflow.")?;
        for island in &islands {
            let mut start = begin.max(island.start);
            let end = end.min(island.end);
            if channel == ChannelType::RemoteWideband && start < end && island.rate < 16_000 {
                bail!("Wideband enrollment requires a source sampled at 16 kHz or higher.");
            }
            while end.saturating_sub(start) >= SECOND && total < MAX_CLEAN_MS * 1_000_000 {
                let stop = end
                    .min(start + 5 * SECOND)
                    .min(start + MAX_CLEAN_MS * 1_000_000 - total);
                if stop - start < SECOND {
                    break;
                }
                output.push(Span {
                    start,
                    end: stop,
                    rate: island.rate,
                    epoch: island.epoch,
                });
                total += stop - start;
                start = stop;
            }
        }
    }
    output.sort_by_key(|s| s.start);
    if output.windows(2).any(|p| p[0].end > p[1].start) {
        bail!("Speaker source spans overlap.");
    }
    if total < MIN_CLEAN_MS * 1_000_000 {
        bail!("At least 20 seconds of clean, non-overlapped refined speech is required.");
    }
    Ok(output)
}
fn recompute(
    service: &AppService,
    request: &EnrollVoiceRequest,
    selection: &Selection,
    engine: &mut dyn EmbeddingEngine,
) -> Result<(Vec<f32>, u64)> {
    let mut reader = vt_playback::SavedAudioReader::open(&service.root, &request.meeting_id)?;
    let mut source = 0usize;
    let mut frame = 0u64;
    let mut selected = 0usize;
    let mut audio = Zeroizing::new(Vec::<f32>::new());
    let mut mean = Zeroizing::new(vec![0.0f64; 512]);
    let mut count = 0u64;
    let mut duration = 0u64;
    let mut flush = |audio: &mut Vec<f32>, span: &Span| -> Result<()> {
        if audio.len() < span.rate as usize {
            audio.clear();
            return Ok(());
        }
        let embedding = Zeroizing::new(engine.extract(audio, span.rate)?);
        if embedding.len() != 512 || embedding.iter().any(|v| !v.is_finite()) {
            bail!("The voice embedding engine returned invalid output.");
        }
        let speech = audio.len() as u64 * 1_000_000_000 / u64::from(span.rate);
        for (sum, value) in mean.iter_mut().zip(embedding.iter()) {
            *sum += f64::from(*value) * speech as f64;
        }
        duration = duration
            .checked_add(speech)
            .context("Speech clock overflow.")?;
        count += 1;
        use zeroize::Zeroize;
        audio.zeroize();
        audio.clear();
        Ok(())
    };
    while let Some(packet) = reader.next_packet()? {
        cancelled(service)?;
        match packet {
            vt_playback::SavedAudioPacket::Gap {
                track,
                start_ms,
                end_ms,
            } if track == selection.track => {
                let chunk = selection
                    .chunks
                    .get(source)
                    .context("Source manifest changed.")?;
                if chunk.ready
                    || chunk.start / 1_000_000 != start_ms
                    || chunk.end / 1_000_000 != end_ms
                    || frame != 0
                {
                    bail!("The source clock no longer matches its manifest.");
                }
                source += 1;
            }
            vt_playback::SavedAudioPacket::Audio {
                track,
                samples,
                rate,
                start_ms,
            } if track == selection.track => {
                let samples = Zeroizing::new(samples);
                let chunk = selection
                    .chunks
                    .get(source)
                    .context("Source manifest changed.")?;
                let packet_start = chunk.start + frame * SECOND / u64::from(rate);
                if !chunk.ready
                    || rate != chunk.rate
                    || packet_start / 1_000_000 != start_ms
                    || frame + samples.len() as u64 > chunk.frames
                {
                    bail!("The source clock no longer matches its manifest.");
                }
                let packet_end =
                    chunk.start + (frame + samples.len() as u64) * SECOND / u64::from(rate);
                while selected < selection.spans.len()
                    && selection.spans[selected].end <= packet_start
                {
                    flush(&mut audio, &selection.spans[selected])?;
                    selected += 1;
                }
                let mut index = selected;
                while let Some(span) = selection
                    .spans
                    .get(index)
                    .filter(|span| span.start < packet_end)
                {
                    if span.rate != rate || span.epoch != chunk.epoch {
                        bail!("A selected speech span crossed a device or format epoch.");
                    }
                    let lo = span
                        .start
                        .saturating_sub(packet_start)
                        .saturating_mul(u64::from(rate))
                        .div_ceil(SECOND)
                        .min(samples.len() as u64) as usize;
                    let hi = span
                        .end
                        .saturating_sub(packet_start)
                        .saturating_mul(u64::from(rate))
                        / SECOND;
                    let hi = hi.min(samples.len() as u64) as usize;
                    if lo < hi {
                        audio.extend_from_slice(&samples[lo..hi]);
                    }
                    if audio.len() > rate as usize * 5 {
                        bail!("Enrollment source window exceeded its bound.");
                    }
                    if span.end <= packet_end {
                        flush(&mut audio, span)?;
                        index += 1;
                        selected = index;
                    } else {
                        break;
                    }
                }
                frame += samples.len() as u64;
                if frame == chunk.frames {
                    frame = 0;
                    source += 1;
                }
            }
            _ => {}
        }
        if selected == selection.spans.len() {
            break;
        }
    }
    if let Some(span) = selection.spans.get(selected) {
        flush(&mut audio, span)?;
    }
    if count == 0 || duration < MIN_CLEAN_MS * 1_000_000 {
        bail!(
            "At least 20 seconds of retained clean source speech is required after frame clipping."
        );
    }
    let norm = mean.iter().map(|v| v * v).sum::<f64>().sqrt();
    if !norm.is_finite() || norm <= f64::EPSILON {
        bail!("The clean speech did not produce a usable voice embedding.");
    }
    Ok((
        mean.iter().map(|v| (*v / norm) as f32).collect(),
        duration / 1_000_000,
    ))
}

#[cfg(test)]
#[path = "voice_memory_tests.rs"]
mod tests;
