"""Hermetic annotation-conversion tests; no AMI audio or downloads required."""
import importlib.util
import json
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
import wave
import zipfile

spec = importlib.util.spec_from_file_location("prepare_ami", Path(__file__).with_name("prepare_ami.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class PrepareTests(unittest.TestCase):
    def fixture(self, root):
        audio = root / "ES2002a.Array1-01.wav"
        with wave.open(str(audio), "wb") as out:
            out.setparams((1, 2, 16000, 64000, "NONE", "not compressed"))
            out.writeframes(b"\x00\x00" * 64000)
        archive = root / "annotations.zip"
        with zipfile.ZipFile(archive, "w") as out:
            out.writestr("LICENCE.txt", "Creative Commons Attribution 4.0 synthetic test")
            for i, speaker in enumerate("ABCD"):
                out.writestr(f"words/ES2002a.{speaker}.words.xml", f'<root><w starttime="{1+i*.1}" endtime="{1.2+i*.1}">word{speaker}</w><w starttime="1.2" endtime="1.2" punc="true">,</w></root>')
                out.writestr(f"segments/ES2002a.{speaker}.segments.xml", '<root><segment transcriber_start="0.8" transcriber_end="1.4"/><segment transcriber_start="1.2" transcriber_end="2.5"/></root>')
        return SimpleNamespace(audio=audio, annotations=archive, output=root / "output", meeting="ES2002a", start=1., duration=1., streaming=root, refinement=root, diarization=root, paced_live=False, rttm_kind="segments", allow_fewer_speakers=False)

    def test_crop_word_timing_and_four_distinct_speaker_annotations(self):
        with tempfile.TemporaryDirectory() as directory:
            args = self.fixture(Path(directory))
            module.prepare(args)
            words = json.loads((args.output / "reference-words.json").read_text())
            self.assertEqual([w["start_ms"] for w in words], [0, 100, 200, 300])
            self.assertEqual([w["speaker"] for w in words], list("ABCD"))
            lines = (args.output / "reference.rttm").read_text().splitlines()
            self.assertEqual(len(lines), 4)
            self.assertTrue(all("0.000000 1.000000" in line for line in lines))
            manifest = json.loads((args.output / "inference-input.json").read_text())
            self.assertIn("not a prebuilt official RTTM", manifest["public_corpus"]["derivation"])
            self.assertEqual(manifest["audio"]["sha256"], module.sha(args.output / "audio.wav"))
            with self.assertRaises(FileExistsError):
                module.prepare(args)

    def test_bounds_and_nonfinite_offsets_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            args = self.fixture(Path(directory))
            for start, duration in [(float("nan"), 1), (0, 601), (-1, 1), (0, 0)]:
                args.start, args.duration = start, duration
                with self.assertRaises(ValueError):
                    module.prepare(args)

    def test_only_words_protocol_keeps_actual_word_intervals_and_excludes_segment_gaps(self):
        with tempfile.TemporaryDirectory() as directory:
            args = self.fixture(Path(directory))
            args.rttm_kind = "only_words"
            module.prepare(args)
            lines = (args.output / "reference.rttm").read_text().splitlines()
            self.assertEqual(len(lines), 4)
            self.assertTrue(all(float(line.split()[4]) == .2 for line in lines))
            self.assertEqual([float(line.split()[3]) for line in lines], [0., .1, .2, .3])


if __name__ == "__main__":
    unittest.main()
