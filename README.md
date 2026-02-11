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

### 1. Install Dependencies

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
cargo build --release
```

### 4. Enroll Your Face

```bash
./nihao.sh add
./nihao.sh test  # Verify it works
```

Faces are stored in `/var/lib/nihao/faces/`.

### 5. Install PAM Module

```bash
# Install PAM module
sudo cp target/release/libpam_nihao.so /lib/security/pam_nihao.so
sudo chmod 755 /lib/security/pam_nihao.so

# Configure PAM for sudo
sudo nano /etc/pam.d/sudo
```

Add this line at the **top** (before other auth lines):

```
auth sufficient pam_nihao.so
```

Save and exit. Now test:

```bash
sudo echo "Testing face auth..."
```

Your camera should activate and authenticate you in ~1 second! If face auth fails, you'll get a password prompt (fallback always works).

## Configuration

Config file: `~/.config/nihao/nihao.toml` or `/etc/nihao/nihao.toml`

```toml
[camera]
device = "/dev/video2"           # IR camera device
width = 640
height = 480
detection_scale = 0.5            # Use 320x240 for detection (4x faster)
dark_threshold = 80.0            # Filter bad IR frames

[detection]
model_path = "models/scrfd_500m.onnx"
confidence_threshold = 0.5

[embedding]
model_path = "models/arcface_mobilefacenet.onnx"

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

## How PAM Authentication Works

When you run `sudo`:

```
1. sudo sets environment variables:
   SUDO_USER=johnny (the real user)
   PAM_USER=root (target user - ignored)

2. PAM loads pam_nihao.so

3. Module reads SUDO_USER → "johnny"

4. Loads faces from /var/lib/nihao/faces/johnny/

5. Authenticates face → SUCCESS or FAIL

6. On SUCCESS: sudo grants access (no password!)
   On FAIL: Falls through to password prompt
```

**Security:**
- Authenticates the **invoking user** (you), not the target user (root)
- Password fallback always works
- All attempts logged to syslog
- No network access - 100% local

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
./nihao.sh add
./nihao.sh list  # Verify it saved
```

### "Camera not found"

Check your camera device:

```bash
ls -la /dev/video*
v4l2-ctl --list-devices
```

Update config to point to your camera (usually `/dev/video0` or `/dev/video2`).

### Getting Locked Out

You can't get locked out! The PAM config uses `sufficient`, meaning:
- ✅ Face succeeds → Authentication complete
- ❌ Face fails → Continue to password prompt

To disable face auth temporarily, edit `/etc/pam.d/sudo` and comment out the `pam_nihao.so` line with `#`.

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
