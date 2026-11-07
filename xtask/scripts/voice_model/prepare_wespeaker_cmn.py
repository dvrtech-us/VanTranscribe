#!/usr/bin/env python3
"""Prepare one pinned research derivative; no downloads or arbitrary ONNX editing.

The source remains an identical prefix. The only append is a fixed ModelProto
metadata entry enabling the native feature normalization required by the upstream
WeSpeaker inference recipe. Neither the graph nor trained parameters is rewritten.
"""
from __future__ import annotations
import argparse
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import stat
import sys

TRANSFORMATION = "onnx-metadata-feature-normalize-global-mean-v1"
METADATA_SUFFIX = bytes.fromhex(
    "72250a16666561747572655f6e6f726d616c697a655f74797065120b676c6f62616c2d6d65616e"
)
CHUNK_BYTES = 64 * 1024

@dataclass(frozen=True)
class _Pin:
    source_bytes: int
    source_sha256: str
    derived_bytes: int
    derived_sha256: str

_PIN = _Pin(
    26_534_365,
    "5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94",
    26_534_404,
    "6928c40c3b5c695352998c261cb6f2250c4e52a991e324c0f63c2e10fe7c31f8",
)

def _unlink_owned(output: Path, created: os.stat_result | None) -> None:
    """Never remove a pre-existing or subsequently replaced destination."""
    if created is None:
        return
    try:
        current = output.lstat()
        if stat.S_ISREG(current.st_mode) and (current.st_dev, current.st_ino) == (
            created.st_dev, created.st_ino
        ):
            output.unlink()
    except OSError:
        # Preserve the original preparation failure. A partial output can never
        # pass the model pack's independently pinned size/hash verification.
        pass

def _prepare(source: Path, output: Path, pin: _Pin) -> dict[str, object]:
    """Private test injection supports small fixtures; CLI pins cannot be changed."""
    if pin.source_bytes <= 0 or pin.derived_bytes != pin.source_bytes + len(METADATA_SUFFIX):
        raise ValueError("invalid fixed transformation sizes")
    source_stat = source.lstat()
    if not stat.S_ISREG(source_stat.st_mode) or source_stat.st_size != pin.source_bytes:
        raise ValueError("source must be the pinned regular file with its exact byte count")
    created = None
    try:
        with source.open("rb") as reader:
            actual = os.fstat(reader.fileno())
            if (actual.st_dev, actual.st_ino, actual.st_size) != (
                source_stat.st_dev, source_stat.st_ino, pin.source_bytes
            ):
                raise ValueError("source changed before opening")
            # Exclusive creation refuses existing files and symlinks, including
            # source == output. No overwrite/update operation exists.
            with output.open("xb") as writer:
                created = os.fstat(writer.fileno())
                os.chmod(writer.fileno(), 0o600)
                source_hash = hashlib.sha256()
                count = 0
                while True:
                    data = reader.read(min(CHUNK_BYTES, pin.source_bytes + 1 - count))
                    if not data:
                        break
                    count += len(data)
                    if count > pin.source_bytes:
                        raise ValueError("source grew beyond its pinned bound")
                    source_hash.update(data)
                    writer.write(data)
                if count != pin.source_bytes or source_hash.hexdigest() != pin.source_sha256:
                    raise ValueError("source SHA-256 or byte count does not match the fixed pin")
                derived_hash = source_hash.copy()
                derived_hash.update(METADATA_SUFFIX)
                if derived_hash.hexdigest() != pin.derived_sha256:
                    raise ValueError("derived SHA-256 does not match the fixed pin")
                writer.write(METADATA_SUFFIX)
                if writer.tell() != pin.derived_bytes:
                    raise ValueError("derived byte count does not match the fixed pin")
                writer.flush()
                os.fsync(writer.fileno())
        directory = os.open(output.parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    except BaseException:
        _unlink_owned(output, created)
        raise
    return {
        "research_only": True,
        "transformation": TRANSFORMATION,
        "source_bytes": pin.source_bytes,
        "source_sha256": pin.source_sha256,
        "derived_bytes": pin.derived_bytes,
        "derived_sha256": pin.derived_sha256,
        "source_prefix_preserved": True,
        "appended_metadata_bytes": len(METADATA_SUFFIX),
        "runtime_pin_generated": False,
    }

def prepare(source: Path, output: Path) -> dict[str, object]:
    return _prepare(source, output, _PIN)

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="new file; parent must exist")
    args = parser.parse_args()
    try:
        report = prepare(args.source, args.output)
    except (OSError, ValueError) as error:
        print(f"Preparation failed: {error}", file=sys.stderr)
        return 2
    print(json.dumps(report, sort_keys=True))
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
