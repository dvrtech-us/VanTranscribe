#!/usr/bin/env python3
"""Compare raw vt-speaker-frontend log-mel frames with torchaudio's Kaldi fbank.

The pass limit is fixed before any result is read: identical frame and bin
counts, and a maximum absolute difference of 1e-3. Mean subtraction stays off.
This script scores no identity and writes no embedding.

Synthetic fixtures are generated here. An optional WAV is a single frozen
calibration window supplied by the caller; its samples are compared as features
only.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import struct
import subprocess
import sys
import tempfile
import wave
from pathlib import Path

MAX_ABS_ERROR = 1e-3
SAMPLE_RATE = 16_000
BINS = 80


def sine(n: int, freq: float, amp: float) -> list[float]:
    return [
        amp * math.sin(2.0 * math.pi * freq * i / SAMPLE_RATE) for i in range(n)
    ]


def noise(n: int, amp: float, seed: int) -> list[float]:
    state = seed & 0xFFFFFFFF
    values = []
    for _ in range(n):
        state = (1664525 * state + 1013904223) & 0xFFFFFFFF
        unit = (state / 0xFFFFFFFF) * 2.0 - 1.0
        values.append(unit * amp)
    return values


def synthetic_fixtures() -> list[tuple[str, list[float]]]:
    return [
        ("zeros-1s", [0.0] * SAMPLE_RATE),
        ("sine-440-1s", sine(SAMPLE_RATE, 440.0, 0.3)),
        ("noise-2s", noise(SAMPLE_RATE * 2, 0.5, 0x56414E54)),
        ("sine-220-5s", sine(SAMPLE_RATE * 5, 220.0, 0.2)),
    ]


def read_wav(path: Path) -> list[float]:
    with wave.open(str(path), "rb") as handle:
        if handle.getnchannels() != 1 or handle.getframerate() != SAMPLE_RATE:
            raise SystemExit(f"{path} must be mono {SAMPLE_RATE} Hz")
        if handle.getsampwidth() != 2:
            raise SystemExit(f"{path} must be PCM16")
        frames = handle.getnframes()
        if not SAMPLE_RATE <= frames <= SAMPLE_RATE * 5:
            raise SystemExit(f"{path} must be one to five seconds")
        raw = handle.readframes(frames)
    return [sample / 32768.0 for sample in struct.unpack("<" + "h" * frames, raw)]


def write_f32(path: Path, samples: list[float]) -> None:
    path.write_bytes(b"".join(struct.pack("<f", sample) for sample in samples))


def probe(binary: Path, samples: list[float]) -> tuple[dict, list[float]]:
    with tempfile.TemporaryDirectory(prefix="vt-frontend-probe-") as directory:
        root = Path(directory)
        source = root / "input.f32le"
        output = root / "features.bin"
        write_f32(source, samples)
        subprocess.run(
            [str(binary), "--input", str(source), "--output", str(output)],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        payload = output.read_bytes()
    newline = payload.index(b"\n")
    header = json.loads(payload[:newline])
    raw = payload[newline + 1 :]
    if len(raw) != header["values_bytes"] or len(raw) % 4:
        raise SystemExit("probe payload does not match its header")
    values = list(struct.unpack("<" + "f" * (len(raw) // 4), raw))
    return header, values


def reference(samples: list[float]):
    import torch
    import torchaudio

    wave = torch.tensor(samples, dtype=torch.float32).unsqueeze(0) * 32768.0
    matrix = torchaudio.compliance.kaldi.fbank(
        wave,
        num_mel_bins=BINS,
        frame_length=25.0,
        frame_shift=10.0,
        dither=0.0,
        sample_frequency=float(SAMPLE_RATE),
        window_type="hamming",
        use_energy=False,
        snip_edges=True,
        high_freq=0.0,
        low_freq=20.0,
        preemphasis_coefficient=0.97,
        remove_dc_offset=True,
        round_to_power_of_two=True,
        subtract_mean=False,
    )
    return matrix.detach().cpu().tolist()


def compare(name: str, samples: list[float], binary: Path) -> dict:
    header, native = probe(binary, samples)
    expected = reference(samples)
    frames = len(expected)
    bins = len(expected[0]) if frames else 0
    flat = [value for row in expected for value in row]
    max_abs = 0.0
    if header["frames"] == frames and header["bins"] == bins and len(native) == len(flat):
        max_abs = max((abs(a - b) for a, b in zip(native, flat)), default=0.0)
        geometry_ok = True
    else:
        geometry_ok = False
    passed = geometry_ok and max_abs <= MAX_ABS_ERROR
    digest = hashlib.sha256(struct.pack("<" + "f" * len(samples), *samples)).hexdigest()
    return {
        "name": name,
        "samples": len(samples),
        "samples_sha256": digest,
        "native_frames": header["frames"],
        "native_bins": header["bins"],
        "reference_frames": frames,
        "reference_bins": bins,
        "geometry_ok": geometry_ok,
        "max_abs_error": None if not geometry_ok else max_abs,
        "limit": MAX_ABS_ERROR,
        "passed": passed,
        "stage": "raw_log_mel_before_cmn",
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--probe-bin", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--wav", type=Path)
    args = parser.parse_args()
    if args.output.exists():
        raise SystemExit("output must be a new file")
    cases = synthetic_fixtures()
    if args.wav:
        cases.append((f"wav:{args.wav.name}", read_wav(args.wav)))
    rows = [compare(name, samples, args.probe_bin) for name, samples in cases]
    report = {
        "schema_version": 1,
        "research_only": True,
        "identity_scored": False,
        "reference": "torchaudio.compliance.kaldi.fbank",
        "window": "hamming",
        "snip_edges": True,
        "high_freq": 0,
        "low_freq": 20,
        "subtract_mean": False,
        "max_abs_error_limit": MAX_ABS_ERROR,
        "passed": all(row["passed"] for row in rows),
        "cases": rows,
    }
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": report["passed"], "cases": rows}, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
