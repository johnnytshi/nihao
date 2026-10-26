# 你好 NiHao

Fast facial authentication for Linux using PAM. Written in Rust.

**Performance:** 0.7-1.2 seconds authentication time (CPU-only)

## Features

- 🔒 **PAM Integration** - Works with sudo, login, GDM, SDDM, etc.
- 🎥 **IR Camera Support** - Optimized for infrared cameras
- ⚡ **Fast** - Sub-second authentication after first model load
- 🦀 **Native Code** - No Python interpreter overhead
- 🛡️ **Safe Fallback** - Password always works if face recognition fails
- 📦 **Simple** - Single binary + PAM module

## How It Works

1. You run `sudo` → PAM loads `/lib/security/pam_nihao.so`
2. Camera captures frame → SCRFD detects face (~10ms)
3. ArcFace generates 512-d embedding (~5ms)
4. Compare with enrolled faces → Grant/deny access
5. Falls back to password if face auth fails

## Quick Start

### Automated Installation

Run the install script to set up everything automatically:

```bash
sudo ./install.sh
```

This will:
1. Build the project
2. Download and install models to `/usr/share/nihao/models/`
3. Create system config at `/etc/nihao/nihao.toml`
4. Install PAM module and CLI binary
5. Configure PAM for system-wide authentication
6. Guide you through face enrollment

**To uninstall:** `sudo ./uninstall.sh`

### Manual Installation

If you prefer to install manually or understand each step:

#### 1. Install Dependencies

```bash
# Arch Linux
sudo pacman -S rust onnxruntime-cpu v4l-utils

# Ubuntu/Debian
sudo apt install rustc cargo libonnxruntime v4l-utils libpam0g-dev

# Fedora
sudo dnf install rust cargo onnxruntime v4l-utils pam-devel
```

### 2. Download Models

```bash
./scripts/download_models.sh
```

Downloads SCRFD (face detection) and ArcFace (embedding) models to `models/`.

### 3. Build

```bash
cargo build --release --workspace
```

### 4. Install Models System-Wide

```bash
# Create model directory
sudo mkdir -p /usr/share/nihao/models

# Copy models (follow symlinks)
sudo cp -L models/scrfd_500m.onnx /usr/share/nihao/models/
sudo cp -L models/arcface_mobilefacenet.onnx /usr/share/nihao/models/

# Verify
ls -lh /usr/share/nihao/models/
```

### 5. Install System Configuration

```bash
# Create config directory
sudo mkdir -p /etc/nihao

# Create system config with absolute paths
sudo tee /etc/nihao/nihao.toml > /dev/null <<'EOF'
[camera]
device = "/dev/video2"
width = 640
height = 480
detection_scale = 0.5
dark_threshold = 80.0

[detection]
model_path = "/usr/share/nihao/models/scrfd_500m.onnx"
confidence_threshold = 0.5

[embedding]
model_path = "/usr/share/nihao/models/arcface_mobilefacenet.onnx"

[matching]
threshold = 0.4
max_frames = 10
timeout_secs = 4

[storage]
database_path = "/var/lib/nihao/faces"

[debug]
save_screenshots = false
output_dir = "~/.cache/nihao/debug"
EOF

# Create face storage directory
sudo mkdir -p /var/lib/nihao/faces
sudo chmod 755 /var/lib/nihao/faces
```

### 6. Install CLI Binary (Optional)

```bash
# Install CLI tool for enrolling faces
sudo cp target/release/nihao /usr/local/bin/nihao
sudo chmod 755 /usr/local/bin/nihao

# Now you can use it without ./nihao.sh
sudo nihao add "$USER"   # Enroll your face
sudo nihao test "$USER"  # Test authentication
sudo nihao list "$USER"  # List enrolled faces
```

Or keep using `./nihao.sh` wrapper script.

### 7. Enroll Your Face

```bash
sudo nihao add "$USER"
sudo nihao test "$USER"
```

Faces are stored in `/var/lib/nihao/faces/`.

### 8. Install PAM Module

```bash
# Install the helper and PAM module, with a rollback backup.
# --kde-face also installs KDE's dedicated face service and enables its UI.
sudo bash scripts/install_auth.sh --kde-face
```

### 9. Configure PAM (System-Wide)

Edit `/etc/pam.d/system-auth` to enable face auth for everything:

```bash
sudo nano /etc/pam.d/system-auth
```

Add this line at the **top** (after `#%PAM-1.0`, before other auth lines):

```
auth       sufficient                  pam_nihao.so
```

Save and exit. This enables face auth for:
- sudo
- login screen (SDDM/GDM)
- screen lock
- TTY console
- All system authentication

### 10. Test It!

```bash
sudo -k  # Clear credential cache
sudo echo "Testing face auth..."
```

Your camera should activate and authenticate you in ~1 second! If face auth fails, you'll get a password prompt (fallback always works).

Try locking your screen - it should unlock with your face! 🚀

## File Locations

After installation, files are organized as follows:

| Component | Location | Purpose |
|-----------|----------|---------|
| **Models** | `/usr/share/nihao/models/*.onnx` | Face detection & embedding models |
| **Config** | `/etc/nihao/nihao.toml` | System-wide configuration |
| **Faces** | `/var/lib/nihao/faces/` | Enrolled face embeddings per user |
| **Passwords** | `/etc/nihao/*.key` | Encrypted passwords for service unlock (optional) |
| **PAM Module** | `/lib/security/pam_nihao.so` | PAM authentication module |
| **Auth helper** | `/usr/lib/nihao/nihao-auth` | Root-owned setuid face verifier |
| **KDE face service** | `/etc/pam.d/kde-face` | Dedicated lock-screen face authentication |
| **CLI Binary** | `/usr/local/bin/nihao` (optional) | Command-line tool |
| **PAM Config** | `/etc/pam.d/system-auth` | PAM configuration |

**User-specific files:**
- `~/.config/nihao/nihao.toml` - User config (overrides system config)
- `~/.cache/nihao/debug/` - Debug screenshots (if enabled)

## Configuration

Config file: `/etc/nihao/nihao.toml` (system-wide) or `~/.config/nihao/nihao.toml` (user-specific)

```toml
[camera]
device = "/dev/video2"           # IR camera device
width = 640
height = 480
detection_scale = 0.5            # Use 320x240 for detection (4x faster)
dark_threshold = 80.0            # Filter bad IR frames

[detection]
model_path = "/usr/share/nihao/models/scrfd_500m.onnx"  # System-wide models
confidence_threshold = 0.5

[embedding]
model_path = "/usr/share/nihao/models/arcface_mobilefacenet.onnx"

[matching]
threshold = 0.4                  # Cosine similarity threshold
max_frames = 10
timeout_secs = 4

[storage]
database_path = "/var/lib/nihao/faces"

[debug]
save_screenshots = false
output_dir = "~/.cache/nihao/debug"
```

## Usage

```bash
./nihao.sh add              # Enroll your face
./nihao.sh add "with glasses"  # Enroll with label
./nihao.sh test             # Test authentication
./nihao.sh list             # List enrolled faces
./nihao.sh remove face_0    # Remove a face
./nihao.sh snapshot test.jpg   # Capture camera frame
```

## Automatic Service Unlock

NiHao can automatically unlock keyrings and encrypted services when you authenticate with your face, eliminating the need to manually enter your password after face authentication succeeds.

### How It Works

When face authentication succeeds, NiHao decrypts your stored password and sets `PAM_AUTHTOK`, which allows other PAM modules to automatically unlock their services without prompting:

- **KDE Wallet (KWallet5/6)** - via `pam_kwallet5.so`
- **GNOME Keyring** - via `pam_gnome_keyring.so`
- **Encrypted home directories** - via `pam_ecryptfs.so`
- **Encrypted volumes** - via `pam_mount.so`
- Any other PAM module that reads `PAM_AUTHTOK`

This uses standard PAM token passing mechanisms - no D-Bus complexity needed.

### Setup

1. **Store your login password** (encrypted with AES-256-GCM):
   ```bash
   sudo nihao store-password
   ```

2. **Verify it's stored**:
   ```bash
   sudo nihao check-password
   ```

3. **Configure PAM** to pass the token to other modules:

   Edit `/etc/pam.d/system-auth` and update the `pam_nihao.so` line to use stackable flags:

   ```
   auth       [success=ok default=ignore] pam_nihao.so
   auth       optional                    pam_kwallet5.so
   auth       required                    pam_unix.so
   ```

   This ensures:
   - If face auth succeeds → sets `PAM_AUTHTOK` and continues to `pam_kwallet5`
   - `pam_kwallet5` reads `PAM_AUTHTOK` and unlocks KWallet automatically
   - If face fails → falls through to password prompt (`pam_unix`)

4. **Test it**: Lock your screen and unlock with your face - services should unlock automatically!

### Security Notes

- Password is encrypted with **AES-256-GCM**
- Encryption key derived from `/etc/machine-id` (unique per machine)
- Stored in `/etc/nihao/{username}.key` with `0600` permissions (owner read/write only)
- Only accessible by root; the authentication helper keeps these files private
- Machine ID is public: encryption does not replace root-only file permissions
- Stored passwords are returned only to privileged PAM hosts, never to desktop callers

### Disable Auto-Unlock

To disable automatic service unlock while keeping face authentication:

```bash
sudo nihao remove-password
```

Face authentication will still work, but you'll need to manually enter your password for services.

## How PAM Authentication Works

The PAM module gets the account from PAM's `pam_get_user` API. It does not trust
`USER` or `SUDO_USER`. It calls `/usr/lib/nihao/nihao-auth`, which verifies the
face using root-owned templates and administrator configuration. A desktop caller
can authenticate only its own UID. Privileged PAM hosts may authenticate the
account requested by their PAM transaction and receive its optional service token.

The helper ignores user configuration and environment library overrides. It
serializes camera access, stops when its caller exits, and has a 15-second hard
deadline. Password authentication remains available through the configured PAM
fallback or KDE's separate Password method.

### KDE lock-screen face unlock

Recent KDE versions support a dedicated `kde-face` PAM service. The authentication
installer installs that service and enables `[Authenticators] Face=true` for the
user invoking sudo. On the next lock screen, choose **Face**; KDE remembers that
choice after successful authentication. The **Password** method remains available.
This does not require changing ownership or permissions of enrolled faces.

### Infrared illumination

For the ASUS IR camera with USB ID `0bda:636e`, NiHao enables the camera's
documented alternating IR illumination mode during capture and restores its
previous mode when the camera closes. It keeps the stream running across frame
retries so illuminated and ambient frames can arrive in sequence.

Recognition in a lit room does not verify illumination. After installing an IR
capture update, test face authentication with the room lights off and check the
camera's red IR emitter. The camera's JPEG output is also monochrome; JPEG
compression does not imply use of the color camera.

For an existing installation, update just the authentication components:

```bash
cargo build --release --workspace
sudo bash scripts/install_auth.sh --kde-face
```

The installer prints a backup location and rollback command. It preserves models,
configuration and enrollment. After upgrading from the old SCRFD decoder, enroll
a new sample with `sudo nihao add "$USER"` because corrected face alignment can
change embeddings. Existing samples remain until you explicitly remove them.

## Uninstall

To remove NiHao from your system:

```bash
sudo ./uninstall.sh
```

This will:
- Restore the PAM backup, or remove NiHao lines if no backup exists
- Remove PAM module: `/lib/security/pam_nihao.so`
- Remove CLI binary: `/usr/local/bin/nihao`
- Remove the helper, dedicated KDE face service, models and configuration
- Preserve stored passwords and face data unless you confirm their deletion

After uninstall, password authentication works normally. To reinstall, run `sudo ./install.sh`.

## Troubleshooting

### "Failed to load ONNX Runtime"

Install system ONNX Runtime:

```bash
sudo pacman -S onnxruntime-cpu  # Arch
sudo apt install libonnxruntime  # Ubuntu
sudo dnf install onnxruntime     # Fedora
```

### "No enrolled faces"

Enroll your face first:

```bash
sudo nihao add "$USER"
sudo nihao list "$USER"  # Verify it saved
```

### "Camera not found"

Check your camera device:

```bash
ls -la /dev/video*
v4l2-ctl --list-devices
```

Update config to point to your camera (usually `/dev/video0` or `/dev/video2`).

### Getting Locked Out

The simple integration uses `sufficient`, which falls through to password on a
face-authentication failure. Service-unlock mode continues through the password
stack using the stored token. KDE's Face and Password methods are separate.

To disable face auth temporarily, edit `/etc/pam.d/system-auth` and comment out the `pam_nihao.so` line with `#`.

### KWallet Still Prompts for Password

If KWallet still asks for a password after face authentication:

**1. Check if kwallet-pam is installed:**
```bash
sudo pacman -S kwallet-pam  # Arch Linux
sudo apt install libpam-kwallet5  # Ubuntu/Debian
```

**2. Verify PAM module location:**
```bash
find /lib /usr/lib -name "pam_kwallet*.so"
```

If the module is in `/usr/lib/security/`, PAM should find it automatically.

**3. Check if your KWallet password matches your login password:**

KWallet must be created with your **login password** (same as your face auth stored password) for PAM integration to work. If you used a different password when creating KWallet, you need to recreate it:

```bash
# Backup existing wallet
mkdir -p ~/kwallet-backup
cp ~/.local/share/kwalletd/kdewallet.* ~/kwallet-backup/

# Remove old wallet (it will be recreated)
rm ~/.local/share/kwalletd/kdewallet.*

# Lock and unlock - use your LOGIN PASSWORD when prompted
loginctl lock-session
# Unlock with face → KWallet will prompt → Use your LOGIN password
```

**4. Verify PAM_AUTHTOK is being set:**
```bash
# Enable verbose logging temporarily
sudo journalctl -t sudo -f &

# Test face auth
sudo -k
sudo echo "test"

# Look for: "NiHao: PAM_AUTHTOK set successfully"
```

**5. Check PAM configuration order:**

Ensure `pam_kwallet5.so` comes **after** `pam_unix.so` in `/etc/pam.d/system-auth`:
```
auth       [success=ok default=ignore] pam_nihao.so
auth       [success=1 default=bad]     pam_unix.so try_first_pass nullok
...
auth       optional                    pam_kwallet5.so  # ← Must be after pam_unix
```

## Performance

**Tested on AMD Ryzen with IR camera at 640x480:**

- First auth (cold start): ~3-4 seconds (model loading)
- Subsequent auths: 0.7-1.2 seconds
- Detection: ~10ms at 320x240
- Embedding: ~5ms
- CPU usage: Single core, brief spike

**Optimizations:**
- Uses half-resolution (320x240) for detection (4x faster)
- Models cached in memory after first load
- No preprocessing needed for good IR cameras
- Single-camera, IR-only configuration

## Architecture

```
nihao/
├── nihao-core/          # Shared library
│   ├── capture.rs       # V4L2 camera
│   ├── detect.rs        # SCRFD face detection
│   ├── embed.rs         # ArcFace embedding
│   ├── align.rs         # Face alignment
│   ├── compare.rs       # Similarity matching
│   └── store.rs         # Face database
├── nihao-cli/           # CLI tool
│   └── main.rs
├── pam-nihao/           # PAM module
│   └── lib.rs           # pam_sm_authenticate()
└── nihao.sh             # Wrapper script
```

## Models

| Model | Purpose | Size |
|-------|---------|------|
| SCRFD-500M | Face detection | ~2.5 MB |
| ArcFace MobileFaceNet | Face embedding | ~65 MB |

Both from [InsightFace](https://github.com/deepinsight/insightface) (Apache 2.0 license).

## Design Decisions

**Why not Howdy?**
Howdy spawns Python on every auth (~500ms overhead). NiHao is a native `.so` loaded once by PAM.

**Why ONNX Runtime?**
Smaller, faster, and more portable than PyTorch. Works everywhere.

**Why CPU-only?**
Testing showed CPU is 10-12% **faster** than GPU for these small models. GPU has overhead for small batches.

**Why V4L2 directly?**
We need one frame from a camera. V4L2 does this with zero dependencies. OpenCV is overkill.

**Why Rust?**
Memory safety, no GC pauses, and excellent crate ecosystem. Perfect for security-critical code.

## Comparison with Howdy

| | Howdy | NiHao |
|---|---|---|
| Language | Python + C++ | Rust |
| Auth latency | 500ms+ | 0.7-1.2s |
| First-time latency | 1-2s | 3-4s |
| Runtime | Python, dlib, OpenCV | ONNX Runtime only |
| Architecture | PAM → subprocess | PAM → native .so |
| GPU support | NVIDIA | Not needed (CPU is faster) |

## Contributing

See `CLEANUP_SUMMARY.md` for details on the optimized codebase (766 lines of unused features removed).

## License

MIT

## Acknowledgments

- [Howdy](https://github.com/boltgolt/howdy) - Inspiration and PAM approach
- [InsightFace](https://github.com/deepinsight/insightface) - SCRFD and ArcFace models
- [ONNX Runtime](https://onnxruntime.ai/) - Inference engine
