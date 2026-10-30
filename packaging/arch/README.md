# Nihao on Arch Linux

## Install

Download the x86_64 `.pkg.tar.zst` asset from a
[Nihao release](https://github.com/johnnytshi/nihao/releases), then install it:

```sh
sudo pacman -U ./nihao-0.1.0-1-x86_64.pkg.tar.zst
```

The package declares its ONNX Runtime and PAM dependencies. It installs the CLI,
PAM module, root-owned setuid verifier, and default configuration. Installation
does not edit existing PAM services or KDE preferences. The package supports
Arch Linux x86_64; other distributions and architectures are not tested.

When the AUR submission has been published, the source-build command is
`paru -S nihao`. Until then, build the recipe from this repository:

```sh
git clone --branch v0.1.0 https://github.com/johnnytshi/nihao.git
cd nihao/packaging/arch
makepkg -si
```

`pacman -S nihao` requires an enabled repository containing Nihao. Publishing a
GitHub release or an AUR recipe does not add it to Arch's official repositories.

## Setup

1. Install SCRFD detection and ArcFace embedding models you have permission to
   use, with root ownership and no group/other write permissions, at the paths
   in `/etc/nihao/nihao.toml`.

   The optional downloader obtains InsightFace's buffalo_l models. Upstream
   restricts these weights to **noncommercial research** unless separately
   licensed; the MIT license for Nihao does not cover them. Review
   [InsightFace's model terms](https://github.com/deepinsight/insightface#license)
   before explicitly acknowledging them:

   ```sh
   sudo pacman -S --needed curl unzip
   sudo nihao-download-models --install --accept-license
   ```

   The historical installed filenames are `scrfd_500m.onnx` and
   `arcface_mobilefacenet.onnx`; the downloader supplies `det_10g.onnx` and
   `w600k_r50.onnx` under these names.

2. Find the camera with `v4l2-ctl --list-devices` (from `v4l-utils`) and set its
   device path in `/etc/nihao/nihao.toml`. Keep this configuration and all model
   paths root-owned and unwritable by ordinary users.

3. Enroll and test before changing PAM:

   ```sh
   sudo nihao add "$USER"
   sudo nihao test "$USER"
   ```

4. Keep an authenticated administrator terminal open while testing PAM changes.
   Back up the target PAM service (for example `/etc/pam.d/sudo`), then add this
   line before its existing authentication lines to enable face authentication:

   ```pam
   auth sufficient pam_nihao.so
   ```

   Preserve the existing password authentication lines for fallback. Test a
   fresh authentication before closing the administrator terminal. For all
   services, the same line can be added to `/etc/pam.d/system-auth`; that has a
   broader effect than enabling an individual service.

5. For KDE's separate face authentication option, first check whether
   `/etc/pam.d/kde-face` already exists. Back up any existing file, then install
   the supplied example and enable Face in the current user's preferences:

   ```sh
   sudo install -m644 /usr/share/doc/nihao/kde-face.pam /etc/pam.d/kde-face
   kwriteconfig6 --file kscreenlockerrc --group Authenticators --key Face true
   ```

## Upgrades, removal, and source-install migration

Pacman preserves modified `/etc/nihao/nihao.toml` using its normal `.pacnew`
handling. The package does not delete enrollment, manually downloaded models,
or `/etc/nihao/*.key` password records.

Before `pacman -R nihao`, remove your `pam_nihao.so` references and your manually
installed `/etc/pam.d/kde-face`, and disable KDE Face if enabled. Restore the PAM
backups when appropriate. Use pacman for package removal; the source checkout's
`uninstall.sh` belongs to source installations.

For an existing source installation, preserve a root-only backup of
`/etc/nihao`, `/var/lib/nihao`, and your PAM configuration. Pacman will report
conflicts for unowned files installed by the source scripts. After inspecting
those conflicts, move only the overlapping program files and system config to
your backup, install the package, and restore your existing configuration with
root ownership and mode `0644`. Keep the administrator terminal open throughout.
Move the old `/usr/local/bin/nihao` aside so it cannot shadow `/usr/bin/nihao`.
Leave enrollment, keys, models, and working PAM services in place. Avoid broad
`--overwrite` options and do not run the source uninstaller as migration cleanup.

## Maintainer publishing

Commit `Cargo.lock` and tag a tested release as `v0.1.0`. The GitHub Actions
workflow builds the checked-out revision in Arch Linux, validates the package,
and publishes the package and SHA-256 file on release tags. No models or user
data belong in the release assets.

To submit to the AUR, register an AUR account and add the publishing SSH public
key to its account settings. With that authentication configured:

```sh
git -c init.defaultBranch=master clone ssh://aur@aur.archlinux.org/nihao.git /tmp/nihao-aur
cp packaging/arch/PKGBUILD packaging/arch/nihao.install /tmp/nihao-aur/
cd /tmp/nihao-aur
makepkg --printsrcinfo > .SRCINFO
git add PKGBUILD nihao.install .SRCINFO
git commit -m 'Publish nihao 0.1.0'
git push origin master
```

Generate `.SRCINFO` again whenever the recipe changes. Confirm that the public
release tag is available before submitting. Stable tags select the exact source
version; builds use the committed Cargo lockfile with frozen dependency versions.
