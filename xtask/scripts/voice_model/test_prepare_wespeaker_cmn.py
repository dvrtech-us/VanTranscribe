"""Small local fixtures; never download or alter real models."""
import hashlib
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock
import prepare_wespeaker_cmn as prepare

class PreparationTests(unittest.TestCase):
    def fixture(self, folder, data=b"fictional-model-prefix"):
        source = Path(folder) / "source.onnx"
        source.write_bytes(data)
        pin = prepare._Pin(
            len(data), hashlib.sha256(data).hexdigest(),
            len(data) + len(prepare.METADATA_SUFFIX),
            hashlib.sha256(data + prepare.METADATA_SUFFIX).hexdigest(),
        )
        return source, Path(folder) / "derived.onnx", pin

    def test_only_fixed_metadata_is_appended_and_existing_files_are_preserved(self):
        with tempfile.TemporaryDirectory() as folder:
            source, output, pin = self.fixture(folder)
            original = source.read_bytes()
            result = prepare._prepare(source, output, pin)
            self.assertEqual(output.read_bytes(), original + prepare.METADATA_SUFFIX)
            self.assertEqual(source.read_bytes(), original)
            self.assertEqual(result["appended_metadata_bytes"], 39)
            self.assertTrue(result["source_prefix_preserved"])
            self.assertEqual(result["transformation"], prepare.TRANSFORMATION)
            saved = output.read_bytes()
            with self.assertRaises(FileExistsError):
                prepare._prepare(source, output, pin)
            self.assertEqual(output.read_bytes(), saved)
            with self.assertRaises(FileExistsError):
                prepare._prepare(source, source, pin)
            self.assertEqual(source.read_bytes(), original)

    def test_wrong_source_or_derived_pin_cleans_only_new_destination(self):
        with tempfile.TemporaryDirectory() as folder:
            source, output, pin = self.fixture(folder)
            for wrong in [
                prepare._Pin(pin.source_bytes, "0" * 64, pin.derived_bytes, pin.derived_sha256),
                prepare._Pin(pin.source_bytes, pin.source_sha256, pin.derived_bytes, "0" * 64),
            ]:
                with self.assertRaises(ValueError):
                    prepare._prepare(source, output, wrong)
                self.assertFalse(output.exists())
            output.write_bytes(b"pre-existing")
            with self.assertRaises(FileExistsError):
                prepare._prepare(source, output, pin)
            self.assertEqual(output.read_bytes(), b"pre-existing")

    def test_size_and_symlink_guards_fail_before_any_destination_is_created(self):
        with tempfile.TemporaryDirectory() as folder:
            source, output, pin = self.fixture(folder)
            source.write_bytes(source.read_bytes() + b"extra")
            with self.assertRaises(ValueError):
                prepare._prepare(source, output, pin)
            self.assertFalse(output.exists())
            source.write_bytes(b"fictional-model-prefix")
            link = Path(folder) / "linked.onnx"
            link.symlink_to(source)
            with self.assertRaises(ValueError):
                prepare._prepare(link, output, pin)
            output.symlink_to(source)
            with self.assertRaises(FileExistsError):
                prepare._prepare(source, output, pin)
            self.assertEqual(source.read_bytes(), b"fictional-model-prefix")

    def test_failure_cleanup_does_not_delete_a_replaced_destination(self):
        with tempfile.TemporaryDirectory() as folder:
            _, output, _ = self.fixture(folder)
            output.write_bytes(b"ours")
            previous = output.stat()
            held = Path(folder) / "held.onnx"
            output.rename(held)
            output.write_bytes(b"someone else's new file")
            prepare._unlink_owned(output, previous)
            self.assertEqual(output.read_bytes(), b"someone else's new file")

    def test_production_interface_rejects_fictional_bytes_without_pin_override(self):
        with tempfile.TemporaryDirectory() as folder:
            source, output, _ = self.fixture(folder)
            with self.assertRaises(ValueError):
                prepare.prepare(source, output)
            self.assertFalse(output.exists())

    def test_read_budget_rejects_a_file_that_grows_after_initial_stat(self):
        with tempfile.TemporaryDirectory() as folder:
            source, output, pin = self.fixture(folder)
            open_original = Path.open
            class GrowingReader:
                def __init__(self, reader): self.reader = reader; self.grown = False
                def __enter__(self): return self
                def __exit__(self, *args): self.reader.close()
                def fileno(self): return self.reader.fileno()
                def read(self, limit):
                    self.assert_limit = limit
                    if not self.grown:
                        with open_original(source, "ab") as writer: writer.write(b"overflow")
                        self.grown = True
                    return self.reader.read(limit)
            def opening(path, mode="r", *args, **kwargs):
                result = open_original(path, mode, *args, **kwargs)
                return GrowingReader(result) if path == source and mode == "rb" else result
            with mock.patch.object(Path, "open", opening):
                with self.assertRaisesRegex(ValueError, "grew beyond"):
                    prepare._prepare(source, output, pin)
            self.assertFalse(output.exists())

if __name__ == "__main__": unittest.main()
