"""Exercise model downloads with local ZIP fixtures, never network or system writes."""
import hashlib
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import zipfile


PROJECT = Path(__file__).resolve().parents[1]
DETECTOR = b"test detector model"
EMBEDDER = b"test embedding model"
UPSTREAM_HASHES = (
    "5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91",
    "4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43",
)


class ModelDownloadTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="nihao-model-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.output = self.root / "models"
        self.archive = self.root / "models.zip"
        self.curl_log = self.root / "curl.log"
        self.tmp = self.root / "tmp"
        self.tmp.mkdir()
        self.script = self.root / "scripts" / "download_models.sh"
        self.script.parent.mkdir()
        source = (PROJECT / "scripts/download_models.sh").read_text()
        # Keep all checksum control flow intact while substituting tiny known
        # fixture weights for the 166 MiB upstream models.
        for upstream_hash, model in zip(UPSTREAM_HASHES, (DETECTOR, EMBEDDER)):
            source = source.replace(upstream_hash, hashlib.sha256(model).hexdigest())
        self.script.write_text(source)
        binary_dir = self.root / "bin"
        binary_dir.mkdir()
        curl = binary_dir / "curl"
        curl.write_text(
            "#!/usr/bin/env python3\n"
            "import os, pathlib, shutil, sys\n"
            "pathlib.Path(os.environ['MODEL_TEST_CURL_LOG']).write_text('called')\n"
            "if os.environ.get('MODEL_TEST_CURL_FAIL'):\n"
            "    sys.exit(22)\n"
            "output = sys.argv[sys.argv.index('--output') + 1]\n"
            "shutil.copyfile(os.environ['MODEL_TEST_ARCHIVE'], output)\n"
        )
        curl.chmod(0o755)
        self.env = os.environ.copy()
        self.env.update(
            PATH=str(binary_dir) + os.pathsep + os.environ["PATH"],
            TMPDIR=str(self.tmp),
            MODEL_TEST_ARCHIVE=str(self.archive),
            MODEL_TEST_CURL_LOG=str(self.curl_log),
        )
        self.make_archive()

    def make_archive(self, prefix="", detector=DETECTOR, embedder=EMBEDDER):
        with zipfile.ZipFile(self.archive, "w") as archive:
            archive.writestr(prefix + "det_10g.onnx", detector)
            archive.writestr(prefix + "w600k_r50.onnx", embedder)
            archive.writestr("../../unrelated", b"must never be extracted")

    def run_script(self, *arguments, script=None, **env):
        return subprocess.run(
            ["bash", str(script or self.script), *map(str, arguments)],
            input="", text=True, capture_output=True, env=self.env | env,
        )

    def test_noninteractive_download_requires_license_acknowledgement(self):
        result = self.run_script("--output-dir", self.output)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("--accept-license", result.stderr)
        self.assertIn("non-commercial research", result.stdout)
        self.assertFalse(self.curl_log.exists())
        self.assertFalse(self.output.exists())

    def test_flat_and_prefixed_archives_install_verified_models(self):
        for prefix in ("", "buffalo_l/"):
            with self.subTest(prefix=prefix):
                self.make_archive(prefix)
                result = self.run_script("--accept-license", "--output-dir", self.output)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual((self.output / "scrfd_500m.onnx").read_bytes(), DETECTOR)
                self.assertEqual((self.output / "arcface_mobilefacenet.onnx").read_bytes(), EMBEDDER)
                self.assertEqual((self.output / "scrfd_500m.onnx").stat().st_mode & 0o777, 0o644)
                self.assertFalse((self.root / "unrelated").exists())
                self.assertEqual(list(self.tmp.iterdir()), [])
                self.assertEqual(list(self.output.glob(".nihao-models.*")), [])

    def test_default_source_directory_is_preserved(self):
        result = self.run_script("--accept-license")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.output / "scrfd_500m.onnx").read_bytes(), DETECTOR)

    def test_installed_command_requires_an_explicit_destination(self):
        installed = self.root / "nihao-download-models"
        installed.write_text(self.script.read_text())
        result = self.run_script("--accept-license", script=installed)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Choose --install", result.stderr)
        self.assertFalse(self.curl_log.exists())

    def test_corrupt_model_preserves_both_existing_models(self):
        self.output.mkdir()
        detector = self.output / "scrfd_500m.onnx"
        embedder = self.output / "arcface_mobilefacenet.onnx"
        detector.write_bytes(b"existing detector")
        embedder.write_bytes(b"existing embedder")
        self.make_archive(embedder=b"corrupt embedding")
        result = self.run_script("--accept-license", "--output-dir", self.output)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("FAILED", result.stdout)
        self.assertEqual(detector.read_bytes(), b"existing detector")
        self.assertEqual(embedder.read_bytes(), b"existing embedder")
        self.assertEqual(list(self.tmp.iterdir()), [])

    def test_replacement_does_not_follow_existing_model_symlink(self):
        self.output.mkdir()
        outside = self.root / "outside.onnx"
        outside.write_bytes(b"keep this symlink target")
        destination = self.output / "scrfd_500m.onnx"
        destination.symlink_to(outside)
        result = self.run_script("--accept-license", "--output-dir", self.output)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(destination.is_symlink())
        self.assertEqual(destination.read_bytes(), DETECTOR)
        self.assertEqual(outside.read_bytes(), b"keep this symlink target")

    def test_http_error_does_not_create_destination(self):
        result = self.run_script(
            "--accept-license", "--output-dir", self.output, MODEL_TEST_CURL_FAIL="1"
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.output.exists())
        self.assertEqual(list(self.tmp.iterdir()), [])

    def test_missing_or_duplicate_models_fail_without_installing(self):
        for names in (
            ("det_10g.onnx",),
            ("det_10g.onnx", "buffalo_l/det_10g.onnx", "w600k_r50.onnx"),
        ):
            with self.subTest(names=names):
                with zipfile.ZipFile(self.archive, "w") as archive:
                    for name in names:
                        archive.writestr(name, DETECTOR)
                result = self.run_script("--accept-license", "--output-dir", self.output)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(self.output.exists())
                self.assertEqual(list(self.tmp.iterdir()), [])

    @unittest.skipIf(os.geteuid() == 0, "root already meets the privilege requirement")
    def test_system_install_requires_root(self):
        result = self.run_script("--install", "--accept-license")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Use sudo", result.stderr)
        self.assertFalse(self.curl_log.exists())

    def test_system_install_rejects_a_symlink_before_downloading(self):
        # Redirect only NiHao's system destination into this fixture and bypass
        # the EUID gate; real parent directory checks remain read-only. No
        # privileged installation branch can run because the symlink is rejected.
        redirected = self.root / "system-models.sh"
        destination = self.root / "nihao-system"
        destination.symlink_to(self.output)
        source = self.script.read_text().replace(
            '[ "$EUID" -eq 0 ]', "true"
        ).replace("/usr/share/nihao", str(destination))
        redirected.write_text(source)
        result = self.run_script("--install", "--accept-license", script=redirected)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Not a trusted system directory", result.stderr)
        self.assertFalse(self.curl_log.exists())


if __name__ == "__main__":
    unittest.main()
