"""Integration checks confined to temporary files, never the host PAM stack."""
import ctypes
import os
import re
from pathlib import Path
import subprocess
import tempfile
import unittest

PROJECT = Path(__file__).resolve().parents[1]


class ScriptTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="nihao-script-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def fixture_script(self, relative):
        script = (PROJECT / relative).read_text()
        # Exercise real script control flow against a filesystem fixture. Tests
        # need neither root ownership nor any setuid executable.
        script = script.replace('if [ "$EUID" -ne 0 ]; then', 'if false; then')
        script = script.replace("-o root -g root", "")
        script = script.replace("nihao-auth 4755", "nihao-auth 0755")
        script = re.sub(r"/(?:etc|usr|var|lib)/", lambda match: str(self.root) + match[0], script)
        destination = self.root / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(script)
        return destination

    def write(self, relative, contents):
        target = self.root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(contents)
        return target

    def run_script(self, script, *arguments, answers="", env_overrides=None):
        env = os.environ.copy()
        env.pop("SUDO_USER", None)
        env.update(env_overrides or {})
        return subprocess.run(
            ["bash", str(script), *map(str, arguments)], input=answers,
            text=True, capture_output=True, check=True, env=env,
        )

    def prepare_uninstall(self):
        script = self.fixture_script("uninstall.sh")
        key = self.write("etc/nihao/fake.key", "fake encrypted password fixture")
        self.write("etc/nihao/nihao.toml", "fixture configuration")
        face = self.write("var/lib/nihao/faces/fake/face.bin", "fake embedding")
        return script, key, face

    def test_uninstall_preserves_password_when_declined(self):
        script, key, face = self.prepare_uninstall()
        self.run_script(script, answers="ynn")
        self.assertTrue(key.exists())
        self.assertTrue(face.exists())
        self.assertFalse((key.parent / "nihao.toml").exists())

    def test_uninstall_removes_password_only_when_requested(self):
        script, key, face = self.prepare_uninstall()
        self.run_script(script, answers="yyn")
        self.assertFalse(key.exists())
        self.assertTrue(face.exists())

    def test_auth_update_preserves_data_and_can_roll_back(self):
        script = self.fixture_script("scripts/install_auth.sh")
        module = self.write("lib/security/pam_nihao.so", "old module")
        helper = self.write("usr/lib/nihao/nihao-auth", "old helper")
        cli = self.write("usr/local/bin/nihao", "old CLI")
        face_service = self.write("etc/pam.d/kde-face", "old service")
        key = self.write("etc/nihao/fake.key", "fake password")
        enrollment = self.write("var/lib/nihao/faces/fake/face.bin", "fake embedding")
        self.write("target/release/libpam_nihao.so", "new module")
        self.write("target/release/nihao-auth", "new helper")
        self.write("target/release/nihao", "new CLI")
        self.write("config/kde-face.pam", "new face service")
        preference = self.write("face-preference", "false")
        tools = {
            "runuser": '#!/bin/sh\nshift 3\nexec "$@"\n',
            "kreadconfig6": '#!/bin/sh\ncat "$NIHAO_TEST_PREFERENCE"\n',
            "kwriteconfig6": '#!/usr/bin/env python3\nimport os, pathlib, sys\np = pathlib.Path(os.environ["NIHAO_TEST_PREFERENCE"])\nif sys.argv[-1] == "--delete":\n    p.unlink(missing_ok=True)\nelse:\n    p.write_text(sys.argv[-1])\n',
        }
        for name, source in tools.items():
            self.write("bin/" + name, source).chmod(0o755)
        env = {
            "PATH": str(self.root / "bin") + os.pathsep + os.environ["PATH"],
            "SUDO_USER": "fixture-user",
            "NIHAO_TEST_PREFERENCE": str(preference),
        }
        self.run_script(script, "--kde-face", env_overrides=env)
        self.assertEqual(module.read_text(), "new module")
        self.assertEqual(helper.read_text(), "new helper")
        self.assertEqual(cli.read_text(), "new CLI")
        self.assertEqual(face_service.read_text(), "new face service")
        self.assertEqual(key.read_text(), "fake password")
        self.assertEqual(enrollment.read_text(), "fake embedding")
        self.assertEqual(preference.read_text(), "true")
        backups = list((self.root / "var/lib/nihao/install-backups").iterdir())
        self.assertEqual(len(backups), 1)
        self.run_script(script, "--rollback", backups[0], env_overrides=env)
        self.assertEqual(module.read_text(), "old module")
        self.assertEqual(helper.read_text(), "old helper")
        self.assertEqual(cli.read_text(), "old CLI")
        self.assertEqual(face_service.read_text(), "old service")
        self.assertEqual(preference.read_text(), "false")


@unittest.skipUnless((PROJECT / "target/debug/libpam_nihao.so").exists(), "build PAM module first")
class NativePamTests(unittest.TestCase):
    def test_c_integer_abi_accepts_zero_and_combined_flags(self):
        module = ctypes.CDLL(str(PROJECT / "target/debug/libpam_nihao.so"))
        signature = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_void_p]
        module.pam_sm_authenticate.argtypes = signature
        module.pam_sm_authenticate.restype = ctypes.c_int
        module.pam_sm_setcred.argtypes = signature
        module.pam_sm_setcred.restype = ctypes.c_int
        for flags in (0, 0x8002):
            self.assertEqual(module.pam_sm_authenticate(None, flags, 0, None), 7)
            self.assertEqual(module.pam_sm_setcred(None, flags, 0, None), 0)


if __name__ == "__main__":
    unittest.main()
