# Pinned WeSpeaker CMN research preparation

`prepare_wespeaker_cmn.py` accepts the exact official ordinary WeSpeaker English
ResNet34 ONNX source and creates a new file. It appends only the fixed 39-byte
ONNX `feature_normalize_type=global-mean` metadata entry. The original graph,
parameters and all source bytes remain an identical prefix. Existing source and
output files are never overwritten; the script performs no download or inference.

```sh
python3 xtask/scripts/voice_model/prepare_wespeaker_cmn.py \
  --source /path/to/original/embedding.onnx \
  --output /path/to/new/embedding.onnx
```

The source's SHA-256
`5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94`
matches both the acquired bytes and the filename-specific publisher entry in
[the official release checksum file](https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/checksum.txt).
That 3,477-byte checksum file was independently recorded with SHA-256
`c3a928cc16d165ce869553f71fcbe6fc1c1b513aee0665414548780a979b2325`.
The source is 26,534,365 bytes. The derived 26,534,404-byte model has the local
reproducibility pin
`6928c40c3b5c695352998c261cb6f2250c4e52a991e324c0f63c2e10fe7c31f8`;
this derivative checksum is not a publisher checksum.

Use the distinct `wespeaker-resnet34-cmn-v1` research profile and its required
transformation manifest value
`onnx-metadata-feature-normalize-global-mean-v1`. Preserve Apache-2.0.txt and use
[the CMN notice](../../../crates/vt-diarize/resources/WeSpeaker-CMN-NOTICE.txt)
as that pack's WeSpeaker-NOTICE.txt. The notice records CC BY 4.0 pretrained
weights attribution, the upstream Apache model-card/code statement and the exact
modification. No weights are bundled. This feature choice does not claim full
upstream inference parity or authorize production speaker recognition.

Small local fixture checks:

```sh
python3 -m unittest discover -s xtask/scripts/voice_model -p 'test_*.py' -v
```
