#!/usr/bin/env python3
"""Validate a built package without installing it or touching host PAM state."""
import io
from pathlib import Path, PurePosixPath
import subprocess
import sys
import tarfile
import tomllib


def validate(package):
    # Let libarchive handle the package compression, then inspect tar metadata.
    archive = subprocess.run(
        ["bsdtar", "--format=ustar", "-cf", "-", "@" + str(package.resolve())],
        check=True, capture_output=True,
    )
    with tarfile.open(fileobj=io.BytesIO(archive.stdout), mode="r:") as tar:
        members = {}
        for member in tar.getmembers():
            name = member.name.removeprefix("./").rstrip("/")
            assert not PurePosixPath(name).is_absolute(), name
            assert ".." not in PurePosixPath(name).parts, name
            assert name not in members, f"Duplicate archive path: {name}"
            members[name] = member
            assert member.uid == member.gid == 0, f"Non-root owner: {name}"
            assert not member.issym() and not member.islnk(), f"Unexpected link: {name}"
            assert not name.endswith((".onnx", ".key")), f"Private/model file: {name}"
            if name.startswith("var/lib/nihao/"):
                assert member.isdir(), f"Enrollment data bundled: {name}"
            if member.mode & 0o6000:
                assert name == "usr/lib/nihao/nihao-auth", f"Unexpected privilege bits: {name}"

        expected = {
            "usr/bin/nihao": 0o755,
            "usr/bin/nihao-download-models": 0o755,
            "usr/lib/nihao/nihao-auth": 0o4755,
            "usr/lib/security/pam_nihao.so": 0o755,
            "etc/nihao/nihao.toml": 0o644,
            "usr/share/doc/nihao/arch-linux.md": 0o644,
            "usr/share/doc/nihao/kde-face.pam": 0o644,
            "usr/share/licenses/nihao/LICENSE": 0o644,
        }
        for name, mode in expected.items():
            member = members[name]
            assert member.isfile(), f"Not a regular file: {name}"
            assert member.mode == mode, f"Wrong permissions: {name}: {member.mode:o}"
        assert members["var/lib/nihao/faces"].isdir()
        assert members["var/lib/nihao/faces"].mode == 0o700
        assert not any(name.startswith("etc/pam.d/") for name in members)
        assert not any(name.startswith("usr/local/") for name in members)

        def read(name):
            return tar.extractfile(members[name]).read()

        metadata = read(".PKGINFO").decode()
        assert "pkgname = nihao\n" in metadata
        assert "arch = x86_64\n" in metadata
        assert "backup = etc/nihao/nihao.toml\n" in metadata
        assert "depend = onnxruntime-cpu>=1.23\n" in metadata
        assert ".INSTALL" in members
        config = tomllib.loads(read("etc/nihao/nihao.toml").decode())
        for section, key in (("detection", "model_path"), ("embedding", "model_path"),
                             ("storage", "database_path")):
            assert PurePosixPath(config[section][key]).is_absolute()
        assert config["debug"]["save_screenshots"] is False
    print(f"Validated {package.name}: layout, ownership, setuid helper, config, dependencies, data exclusion")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("Usage: check_arch_package.py PATH.pkg.tar.zst")
    validate(Path(sys.argv[1]))
