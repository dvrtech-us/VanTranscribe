#!/usr/bin/env python3
"""Derive a bounded public AMI benchmark from explicitly supplied local assets.

No downloads/devices/private meeting discovery. Source: official AMI manual NXT
v1.6.2 and an official mono PCM16 WAV; CC BY 4.0. See generated provenance.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path
import wave
import xml.etree.ElementTree as ET
import zipfile


def sha(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for data in iter(lambda: stream.read(65536), b""):
            digest.update(data)
    return digest.hexdigest()


def prepare(args):
    if not (math.isfinite(args.start) and math.isfinite(args.duration)
            and args.start >= 0 and 0 < args.duration <= 600):
        raise ValueError("require finite start>=0 and duration in (0,600]")
    if not args.meeting.isalnum():
        raise ValueError("meeting must be alphanumeric")
    if args.audio.stat().st_size > 64 * 1024 * 1024 or args.annotations.stat().st_size > 64 * 1024 * 1024:
        raise ValueError("source exceeds64MiB")
    stop = args.start + args.duration
    args.output.mkdir()  # Never replace existing evidence.
    with wave.open(str(args.audio), "rb") as source:
        if source.getnchannels() != 1 or source.getsampwidth() != 2 or source.getcomptype() != "NONE":
            raise ValueError("requires official mono signed PCM16 WAV")
        rate = source.getframerate()
        first = round(args.start * rate)
        count = round(args.duration * rate)
        if first + count > source.getnframes():
            raise ValueError("selected window extends beyond source")
        source.setpos(first)
        pcm = source.readframes(count)
        if len(pcm) != count * 2:
            raise ValueError("truncated source")
        clip = args.output / "audio.wav"
        with wave.open(str(clip), "wb") as out:
            out.setparams((1, 2, rate, count, "NONE", "not compressed"))
            out.writeframes(pcm)
    words, segments, annotation_files = [], [], []
    with zipfile.ZipFile(args.annotations) as archive:
        license_bytes = archive.read("LICENCE.txt")
        if b"Creative" not in license_bytes or b"4.0" not in license_bytes:
            raise ValueError("expected official AMI CC BY4.0 licence")
        (args.output / "AMI-LICENCE.txt").write_bytes(license_bytes)
        for speaker in "ABCD":
            intervals = []
            for family in ["words", "segments"]:
                name = f"{family}/{args.meeting}.{speaker}.{family}.xml"
                info = archive.getinfo(name)
                if info.file_size > 4 * 1024 * 1024:
                    raise ValueError("annotation exceeds bounded size")
                data = archive.read(name)
                # XML source is the pinned public AMI archive, not arbitrary
                # remotely fetched XML; never resolve external entities.
                if b"<!DOCTYPE" in data or b"<!ENTITY" in data:
                    raise ValueError("XML entities/DTDs are not supported")
                path = args.output / Path(name).name
                path.write_bytes(data)
                annotation_files.append(path)
                for node in ET.fromstring(data):
                    if family == "words" and node.tag == "w":
                        a, b = float(node.attrib["starttime"]), float(node.attrib["endtime"])
                        if args.rttm_kind == "only_words" and max(a, args.start) < min(b, stop):
                            intervals.append((max(a, args.start)-args.start, min(b, stop)-args.start))
                        if args.start <= (a + b) / 2 < stop and node.attrib.get("punc") != "true":
                            text = "".join(node.itertext()).strip()
                            if text:
                                words.append({"speaker": speaker, "start_ms": max(0, round((a-args.start)*1000)),
                                              "end_ms": min(round(args.duration*1000), round((b-args.start)*1000)), "text": text})
                    elif family == "segments" and node.tag == "segment" and args.rttm_kind == "segments":
                        a, b = float(node.attrib["transcriber_start"]), float(node.attrib["transcriber_end"])
                        a, b = max(a, args.start), min(b, stop)
                        if a < b:
                            intervals.append((a-args.start, b-args.start))
            # Official segment annotations can touch/overlap within a speaker;
            # union them instead of creating invalid same-speaker RTTM overlap.
            merged = []
            for a, b in sorted(intervals):
                if merged and a <= merged[-1][1]:
                    merged[-1] = (merged[-1][0], max(b, merged[-1][1]))
                else:
                    merged.append((a, b))
            segments.extend((a, b, speaker) for a, b in merged)
    words.sort(key=lambda w: (w["start_ms"], w["speaker"], w["end_ms"]))
    observed_speakers = {speaker for _, _, speaker in segments}
    if observed_speakers != set("ABCD") and not args.allow_fewer_speakers:
        raise ValueError("selected window does not contain all four annotated speakers")
    rid = f"{args.meeting}-array1-01-{args.start:g}-{stop:g}"
    ref = args.output / "reference.txt"
    ref.write_text(" ".join(w["text"] for w in words) + "\n")
    word_path = args.output / "reference-words.json"
    word_path.write_text(json.dumps(words, ensure_ascii=False, indent=2) + "\n")
    rttm = args.output / "reference.rttm"
    rttm.write_text("".join(f"SPEAKER {rid} 1 {a:.6f} {b-a:.6f} <NA> <NA> {s} <NA> <NA>\n" for a, b, s in sorted(segments)))
    uem = args.output / "regions.uem"
    uem.write_text(f"{rid} 1 0.000000 {args.duration:.6f}\n")
    pin = lambda p: {"path": str(p.resolve()), "sha256": sha(p)}
    source_audio_url = f"https://groups.inf.ed.ac.uk/ami/AMICorpusMirror/amicorpus/{args.meeting}/audio/{args.audio.name}"
    annotation_url = "https://groups.inf.ed.ac.uk/ami/AMICorpusAnnotations/ami_public_manual_1.6.2.zip"
    rttm_source = ("each A-D words XML starttime/endtime (BUTSpeechFIT only_words protocol; positive-duration words only)" if args.rttm_kind == "only_words" else "each A-D segments XML transcriber_start/end (includes non-lexical vocal regions)")
    derivation = (f"Official mono Array1-01 source cropped [{args.start:g},{stop:g}) seconds; samples unchanged PCM16. "
                  f"RTTM derived from {rttm_source}, clipped and same-speaker-unioned; "
                  "not a prebuilt official RTTM. Whole-window text uses non-punctuation <w> only, word midpoint inside window, "
                  "ordered by actual word start then speaker and end; overlaps have no unique serial order. "
                  f"Observed reference speakers: {','.join(sorted(observed_speakers))}. No invented timing or speaker labels.")
    manifest = {"schema_version": 1,
                "public_corpus": {"id": f"ami-manual-1.6.2-{args.rttm_kind}-{rid}", "recording_id": rid, "license": "CC-BY-4.0",
                                  "attribution": "AMI Meeting Corpus, AMI Consortium; University of Edinburgh/CSTR and participating institutions. https://groups.inf.ed.ac.uk/ami/corpus/",
                                  "source_urls": [source_audio_url, annotation_url, "https://groups.inf.ed.ac.uk/ami/download/"],
                                  "derivation": derivation},
                "audio": pin(clip), "reference_text": pin(ref), "reference_rttm": pin(rttm), "uem": pin(uem),
                "annotation_sources": [pin(args.audio), pin(args.annotations), pin(word_path)] + [pin(p) for p in annotation_files],
                "models": {"streaming": str(args.streaming.resolve()), "refinement": str(args.refinement.resolve()), "diarization": str(args.diarization.resolve())},
                "paced_live": args.paced_live}
    (args.output / "inference-input.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Prepared {args.duration:g}s public AMI: {len(words)} timed words, {len(segments)} reference intervals, {len(observed_speakers)} reference speakers; no acceptance claimed.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ["audio", "annotations", "output", "streaming", "refinement", "diarization"]:
        parser.add_argument(f"--{key}", type=Path, required=True)
    parser.add_argument("--meeting", default="ES2002a")
    parser.add_argument("--start", type=float, default=0)
    parser.add_argument("--duration", type=float, default=300)
    parser.add_argument("--paced-live", action="store_true")
    parser.add_argument("--rttm-kind", choices=["segments", "only_words"], default="segments",
                        help="Explicit reference protocol; legacy segments baseline remains unchanged.")
    parser.add_argument("--allow-fewer-speakers", action="store_true",
                        help="Explicitly allow a holdout window with fewer than four annotated speakers; report actual count.")
    prepare(parser.parse_args())
