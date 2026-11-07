# Public cross-session speaker-memory corpus

Date: 2026-11-07. This research supports the accepted M4 speaker-memory design without claiming its recognition gate is complete. It uses public AMI material only, outside installed app data. No microphone, private meeting, OS biometric enrollment or persisted embedding is involved. The existing design reviews remain accepted.

## Source and feasibility

The [official AMI download page](https://groups.inf.ed.ac.uk/ami/download/) publishes the audio and transcription under CC BY 4.0 and supplies manual NXT annotations v1.6.2. The [corpus overview](https://groups.inf.ed.ac.uk/ami/corpus/) distinguishes staged design-team meetings from natural meetings. The [meeting-ID documentation](https://groups.inf.ed.ac.uk/ami/corpus/meetingids.shtml) identifies a/b/c/d as distinct meetings in one design trial; they are not independent natural projects. The [participant-ID documentation](https://groups.inf.ed.ac.uk/ami/corpus/participantids.shtml) identifies `corpusResources/meetings.xml` as the participant-to-channel/agent mapping. That mapping must be read separately for every recording: an annotation agent such as A does not establish the same person across meetings.

A cache-only audit of the pinned official archive found:

| Recording category | Recordings | Distinct participant IDs | IDs in at least two recordings | IDs spanning different six-character series prefixes |
|---|---:|---:|---:|---:|
| Design scenario | 138 | 140 | 140 | 0 |
| Natural (`N`) | 26 | 51 | 28 | 13 |
| Other staged (`B`) | 7 | 12 | 10 | 10 |

These are derived metadata counts, not model results. The archive's `nonscenario` category includes other staged meetings, so its combined recurring-person count must not be described as natural-meeting coverage. Natural AMI recordings alone do not provide fifty recurring participants for the requested gallery.

The [official suggested scenario split](https://groups.inf.ed.ac.uk/ami/corpus/datasets.shtml) has only five project teams in each development/evaluation partition. A fifty-person gallery therefore requires an explicitly named custom partition, with its own limitations, rather than a claim to use that standard split.

## Frozen protocol

The active research protocol is `vantranscribe-ami-open-set-in-person-v3`, frozen after the full input-availability audit and before any identification embedding or score. Its v2 predecessor and failures remain available as evidence. Its source channel is the same physical room microphone, `Array1-01`, throughout; it does not mix individual headsets with room audio. The [official signal documentation](https://groups.inf.ed.ac.uk/ami/corpus/signals.shtml) describes the released 16 kHz WAV channels. Every acquired file must still independently prove mono signed PCM16 at 16 kHz from its RIFF header.

| Partition | Project teams | Participants | Gallery people | Identity-unknown people | Known probe trials | Unknown probe trials |
|---|---:|---:|---:|---:|---:|---:|
| Calibration | 16 | 64 | 50 | 14 | 150 | 42 |
| Held out | 16 | 64 | 50 | 14 | 145 | 42 |

Subjects and project teams are disjoint between these partitions. Each active partition has seven Edinburgh, four Idiap and five TNO projects. V2 originally had seventeen/eighteen projects and 68/72 participants; the source-availability exclusions below account for the difference. Project order uses a fixed, published SHA-256 seed within each site, and gallery IDs use a separate phase-specific salted ordering among metadata-eligible subjects. No model scores selected these IDs.

There are exactly one hundred enrollment samples and 379 probe samples. Each gallery participant enrolls from meeting `a`, with at least twenty seconds of annotation-defined clean speech. Every participant has at least two separate probe recordings among b/c/d, each with at least fifteen seconds. Missing or insufficient a-recording speech can make a participant identity-unknown rather than a gallery candidate; the selection is recorded before any model evaluation. All 128 retained participants have sufficient probe support under the fixed rule. No adjacent windows from one recording stand in for a cross-session probe.

Before any new audio acquisition or identity scores, a partition audit found that the initial seeded v1 placed ES2002 in the held-out set. That project had already informed earlier diarization/context development. The preserved v1 is an audit artifact only. V2 uses its own seed and explicitly reserves ES2002 for calibration, swapping the last selected Edinburgh project if needed. V2 was frozen before acquisition. After every selected source task became terminal, v3 excluded whole ES2010, IS1003 and IS1007 projects solely because of the HTTP/native-format failures below. It retained v2 project-to-phase assignments and the v2 person hash ordering, refilling each gallery to exactly fifty before any embedding or identity score. The source/identity/role/frame rows for 472 common samples compare exactly equal as frozen sample objects; seven newly selected enrollment samples follow the already declared first-clean-a rule. Gallery membership changes also change which existing probe people count as known or unknown; the entire enclosing metadata plan is not claimed identical. The frozen v3 must not be redrawn after scores.

Identity-unknown participants can have teammates in the gallery. This is an open-set identity test, including shared room/project acoustics; it does not make every unknown a previously unseen acoustic environment. All twelve calibration unknown-containing projects share gallery teammates. Six of seven held-out unknown-containing projects share gallery teammates, with one entirely unknown project. These are retained in the project-sharing audit. Stripping the documented role suffixes from participant IDs reveals no base-ID aliases or cross-partition collisions.

## Reference and embedding recipe

The planner retains timings and public participant IDs in its research manifests; transcript words are not written to those manifests. It takes positive-duration, non-punctuation `w` intervals, unions touching intervals without filling gaps, and subtracts other participants' positive-duration words and vocal sounds. This follows the existing word-only reference distinction while conservatively excluding annotated competing vocal activity. The [official transcription description](https://groups.inf.ed.ac.uk/ami/corpus/transcription.shtml) explains the separate word, non-speech and vocal-event annotations. Annotation-defined solo speech remains reference evidence, not proof that every audible contaminant was annotated.

Each surviving contiguous island is split into source-ordered windows of one to five seconds. Windows are never joined across a gap, zero-padded or presented as fake contiguous audio. Selection stops at the duration quota; a final window is at least one second, so a sample can slightly exceed twenty/fifteen seconds. Original source-frame boundaries are retained. The exact selected support totals 123,942,416 frames, or 247,884,832 PCM bytes, before WAV headers.

The research runner must use the production `VoiceEmbedder`: normalize each span vector, compute its duration-weighted mean using f64 accumulation, then normalize that mean. A gallery person has the mean of their explicitly supplied enrollment exemplar(s), within the same embedding contract and `in_person` channel. No embedding, voiceprint vector or model output containing vectors is persisted. Model-contract and calibration-version pins are separate from the frozen corpus partition; supplying a model directory cannot change the partition.

Oracle participant labels and word masks deliberately isolate voice identification from diarization mistakes. This experiment does not establish end-to-end automatic speaker clustering or enrollment eligibility in live recordings. A calibration profile must be fixed using calibration results before held-out decisions are examined. Held-out results cannot tune thresholds. Known recognition, known wrong-person assignment, unknown false acceptance, rejection and suggestion must be reported separately with raw denominators. Repeated probes from one person are correlated; trial counts must not be presented as the number of independent identities. A fifty-person gallery alone does not calibrate behavior above fifty people, remote codecs, other rooms or an in-house acceptance set.

## Acquisition and provenance

The initial v2 acquisition budget was approximately 296 MB over 1,942 bounded requests: 1,804 PCM ranges after merging fetch gaps of at most two seconds, plus 138 headers of 64 KiB. Those fetch-only gaps do not enter exported speech windows. The largest planned PCM request is 1,023,040 bytes, below the one-MiB request cap. At most two official-origin requests run concurrently, with bounded timeouts, retries and backoff. A server returning 200 to a range request is rejected; there is no full-file fallback.

The first official ES2002a probe returned 206 for bytes 0–65535, with a strong ETag, a 44-byte data offset and valid mono PCM16/16 kHz geometry. This proves range support for that source only. Every other source must pass its own header, exact Content-Range, native-format, ETag and selected-frame-bounds checks. Subsequent requests use If-Range and must retain the same ETag and total file length. Per-range hashes and response metadata are recorded. These ETags are provenance checks, not cryptographic full-source hashes. Unknown full-source SHA-256 values remain null; a clip hash must never be labeled a full-source hash.

The output consists of canonical mono PCM16 WAV windows and a strict research manifest with participant/project/meeting IDs, gallery membership, sample roles, original frame bounds, clip hashes and source URLs/ETags. The manifest also references exact copies of the official annotation archive and frozen partition. Raw range caches can resume only after their stored hashes and metadata validate. Native evaluation must not run on an incomplete acquisition.

All source audio, annotations and acquisition artifacts stay under `/private/tmp/vantranscribe-voice-calibration/` or the pre-existing `/private/tmp/vantranscribe-ami-eval/` cache. The attribution is AMI Meeting Corpus, AMI Consortium, University of Edinburgh/CSTR and participating institutions; the cached release retains its full license. The [official ethics page](https://groups.inf.ed.ac.uk/ami/corpus/ethicsandconsent.shtml) provides collection/consent documentation. This public research use does not enroll anyone in the installed application's People store.

## Reproducible developer tools

The repository [tool README](../../../xtask/scripts/voice_corpus/README.md) supplies commands using explicit input/output paths. Source audio, XML/ZIP releases and full frame plans remain outside the repository.

| Entry point | Responsibility |
|---|---|
| `xtask/scripts/voice_corpus/analyze_ami.py` | Pinned, bounded, cache-only XML audit; `union`, `subtract`, `usable` and `take` preserve clean source-frame support; v3 uses only terminal source-availability evidence |
| `xtask/scripts/voice_corpus/freeze_partition.py` | Canonical labels/gallery/frames/selection serialization and SHA-256 before any model score |
| `xtask/scripts/voice_corpus/acquire_ami.py` | `NoRedirect`, `request`, `geometry`, `cached_range` and `source_job` enforce official HTTPS origin, exact bounded ranges, native geometry and verified reuse of only missing coverage |
| `xtask/scripts/voice_corpus/export_verified.py` | Binds the strict runner template to frozen participant/project/meeting/role/window choices, verifies canonical WAV hashes and source bounds, and copies hash-verified provenance |
| `xtask/scripts/voice_corpus/audit_pcm.py` | Hash-bound native PCM input sanity: detects empty/all-zero/constant windows and identical window hashes across distinct source origins without model scoring or amplitude thresholds |
| `xtask/scripts/voice_corpus/tests/` | Twelve network-free checks using fictional response metadata and generated PCM, including exact missing-range reuse |

The parameterized analyzer/freezer reproduced the exact published v2 partition hash from the existing official archive and original declared archive path. Python syntax compilation and all twelve acquisition/export/PCM checks pass. Critical checks cannot be bypassed by Python optimization: the tools refuse optimized execution. A separate current-source control verified twenty downloaded ES2002a windows (970,080 frames) byte-for-byte against the previously pinned full public recording, independently checking range offsets and canonical window extraction. This control proves those windows only.

The exact one-off scripts used by the still-running v2 acquisition have separate SHA records in `ami-room-v2/provenance/pre-score-script-pins.json`; packaging does not modify that live process. The repository implementation also rejects redirects before a request can follow them and accepts only the official HTTPS Array1-01 origin/path. The strict runner template uses relative paths contained in its directory; a generated-PCM integration case exercises the real exporter and verifies this portable layout. No script discovers app data, reads Keychain/model data or constructs an embedding.

## Completed acquisition and native input audit

The original v2 job terminated with 134 successful source tasks and four failures: ES2010c HTTP 403; ES2010d stereo PCM16 instead of mono; IS1003b and IS1007d HTTP 404. Its 1,757 successful PCM ranges and 134 headers transferred 287,245,504 bytes. The full status and caches remain intact; no failed project is quietly treated as observed input.

The availability-only v3 cache follow-up completed all 126 required sources with zero failures. It reused 271,419,648 verified response bytes and downloaded exactly 4,856,192 missing PCM bytes in thirty-two requests, within the additional ten-MiB budget. It did not redownload cached coverage, downmix stereo input, substitute channels or fall back to full files. Atomic concurrent budget reservations and a required existing reuse cache protect this narrow follow-up.

The verified export contains 479 samples and 2,788 separate canonical WAV windows totaling 248,007,504 bytes including headers. All source-frame windows are globally non-overlapping. Each exported role/participant/project/meeting/origin-frame tuple binds to the frozen v3; copied annotation and partition files verify their own pins. Every declared waveform hash and original frame count was checked. Twenty ES2002a windows also match the earlier independently pinned full source byte-for-byte.

Before native inference, the input-only PCM audit checked all 123,942,416 selected frames. All 2,788 windows and all 479 samples contain nonzero, nonconstant PCM. There are zero all-zero windows/samples, zero constant windows (including nonzero DC), and zero identical WAV hashes across distinct source-frame identities. Calibration contains 1,441 windows/242 samples with native peak 23,087; held out contains 1,347 windows/237 samples with native peak 26,046. These are input-integrity facts, not voice-quality/identification scores. No RMS or amplitude threshold selected or rejected a sample, and the audit changed no corpus row. Its descriptor hash binds phase, role, public participant ID, source coordinates and waveform hash independently of the later model-directory setting.

The strict runner template is `ami-room-v3/voice-eval-template.json`. Its relative paths allow native containment checks. `model_dir` remains an explicit operator setting; the expected production raw embedding contract is pinned before inference. This agent's work ends at verified inputs. Native recognition results, threshold fitting and final implementation review are separate evidence and cannot be inferred from these checks.

## Immutable evidence pins

| Artifact | SHA-256 |
|---|---|
| Official `ami_public_manual_1.6.2.zip` (22,887,865 bytes) | `b56e5babb2496b8795deeeda7e71178d7fbc9963f94276cf2a3f4b56ebbc9f9d` |
| `corpusResources/meetings.xml` (119,064 bytes) | `8ab6cdcf03ed863e839418f31c7380392397a2eed892d73b9bc1b438c7ec520a` |
| Preserved `metadata-plan-v2.json` | `03ada9f149d21a2b36d9e71a1bdf112b899eb48c5870cb9d22eed09d125641e9` |
| Preserved `partition-v2.json` (490,957 bytes) | `d11f13458ec85f8921d8b4e423cae73fbc15d87a5fe2150d0278c302cb4e49a5` |
| Preserved, unused `partition-v1.json` | `69de8b9e61f16c6e9ee97d4827acdde1484695c6a4d89211142850e236fc6398` |
| Active `metadata-plan-v3.json` | `4e50e823c407de3d3b53c554df7342593b8133c7b2ad5381c410eb0b75246cc7` |
| Active `partition-v3.json` (458,342 bytes) | `90b40f63eea00a794e8cb0e3805e24ec211100a18824a42191882d23d9ac8454` |
| V3 source-availability exclusion journal | `03a8a2aa8f176002c8962bcf08577f5662595e83ba86d94f6475bcf3f40a2b45` |
| Verified relative-path runner template | `7381f9506eb5a98def3f3f6715f64038e511029b0f66cc7f2a024006f3e8e954` |
| Native PCM input audit | `d59367b88d456c6dcdc0a53f1d5ae3733b01553c858d2f93ecd928a9c7065705` |
| Audit window-descriptor set | `43b72bbabca6b8da5cb745136abf3a59f0dbf3b7eb27a649ee2979df88768e54` |
| Expected raw embedding contract, supplied before inference | `d0e9574d06f367f35d6ad1ab4e8b2a6c13c2e586ed56ff159c95f1ceb6925928` |
| ES2002a first 64 KiB response | `d9380afe45ccdef67c884fe1929cb1cb837d834c8897962f7d4d5eebf5b897ec` |

All acquisition, export and input-audit jobs are terminal. No embeddings, recognition scores or thresholds were computed or persisted by the corpus-preparation tools. This completed input preparation establishes no recognition percentage, false-identification rate or production-calibration release.

## Final review limits

The final Opus implementation review found no blockers. At the held-out denominators of 42 unknown and 145 known probes, even zero errors give Wilson 95% upper bounds of about 8.4% unknown false identification and 2.6% known wrong-person identification. No outcome of this experiment can establish the 2% error criteria. Those intervals are descriptive because repeated people/projects further correlate the trials. Target booleans describe observed proportions only. Same room/microphone/project-day enrollment and probes are a favorable channel condition. Production eligibility remains false for every possible result.

The reviewed preparation hardening filters every candidate row to its selected projects and rejects unequal frozen/export partition counts before pairing. The original v3 inputs and their hashes remain unchanged; no new selection or re-export is needed for this corpus, whose scenario subjects do not span project prefixes. Across separate runner invocations, held-out use is a procedural one-shot rule, not enforced by a durable global ledger. Any repeat must be declared as consuming this held-out set.

## First native calibration result

The first native run is now complete; its [integration checkpoint](../Architecture/2026-11-07-speaker-matching-calibration-validation.md#actual-native-result) records provenance, exact counts and immutable report hash. It recognized 113/150 known probes (75.33%), assigned one wrong identity and abstained on 36. Unknown false identification was 0/42 observed. The fixed-grid candidate was score 0.53 / margin 0.13. Because calibration recognition missed 90%, held-out extraction/scoring was skipped entirely. The protocol and all frozen inputs are unchanged. This is an unavailable calibration, not a production profile or milestone pass.

Later [calibration-only model comparisons](../Architecture/2026-11-07-english-speaker-calibration-comparison.md#corrected-native-result) retained exactly the same query IDs/truth, source windows and fitting rules. Restoring a verified missing WeSpeaker normalization improves one result to 130/150 known recognition (86.67%), with three wrong-known and 0/42 unknown false IDs. The reference frontend on those same weights then reaches 138/150 (92%), with 0 wrong-known and 0/42 unknown false IDs. Held-out identity scoring remains unused. The public protocol itself was not redrawn or filtered after scores.
