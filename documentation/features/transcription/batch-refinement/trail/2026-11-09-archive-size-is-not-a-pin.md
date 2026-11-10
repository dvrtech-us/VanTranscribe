# Official Parakeet archive size is not an acceptance pin

- Date: 2026-11-09
- Feature: batch-refinement
- Related code: crates/vt-stt/src/refinement_models.rs, crates/vt-session/src/processing.rs

## Context

GitHub replaced `sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2` on 2026-11-07. The new object is 487,211,443 bytes with digest `1495e91b59d7fc53ddca91f3431758ec99f8fa253fdb0e1e851be0f72b14b0f9`. The installer rejected it before extraction because Content-Length was not 487,170,055. The extracted encoder, decoder and joiner also differ from the earlier catalog. Tokens and Silero VAD are unchanged. The pack already installed from the earlier archive still matches that earlier catalog.

## Decisions

- The compressed archive's Content-Length and SHA-256 are provenance, not a reason to refuse the download.
- A download larger than 1 GiB is still refused.
- Extracted encoder, decoder, joiner, tokens and VAD must match the current official catalog or the pre-2026-11-07 catalog.
- The Silero file downloaded beside the archive keeps its exact size and SHA-256 check.
- A processing run records the weight hashes of the pack on disk. If no pack is installed, it records the current official catalog.

## Alternatives Considered

- Update only the frozen archive size and digest. The next republication of the same URL would break installation again.
- Accept whatever bytes the URL serves. That would install a changed model with no weight check.
- Drop the previously installed catalog. Machines that already verified those weights would have to download again before refining.

## Consequences

- Positive: a republished archive with the same weights installs. Both known weight sets keep working.
- Negative: a third set of weights at the same URL fails until a catalog entry is added. The current official weights are not the ones used by refinements already published from the earlier pack.
