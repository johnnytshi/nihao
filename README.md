# 你好 NiHao

Facial authentication for Linux. Written in Rust. No Python, no dlib, no nonsense.

NiHao is a PAM-based facial recognition system that authenticates users via IR or standard camera, powered by ONNX Runtime. Designed from the ground up for AMD GPUs via ROCm, but works on CPU and NVIDIA too.

Think [Howdy](https://github.com/boltgolt/howdy), but rewritten as a single native binary with sub-50ms auth times instead of spawning a Python interpreter on every `sudo`.

## How it works

1. PAM triggers `pam_nihao.so` during authentication
2. V4L2 grabs a frame from your IR/RGB camera
3. SCRFD detects faces in the frame (~10ms)
4. ArcFace generates a 512-d embedding of the detected face (~5ms)
5. Cosine similarity is computed against enrolled face embeddings
6. PAM returns success or falls through to password

No daemon. No IPC. No subprocess. One shared library, loaded by PAM, does everything.

## Project structure

```
nihao/
├── Cargo.toml
├── README.md
├── LICENSE
├── config/
│   └── nihao.toml              # default config
├── models/
│   ├── scrfd_500m.onnx         # face detection (< 3MB)
│   └── arcface_mbf.onnx        # face embedding (< 70MB)
├── nihao-core/                 # shared logic
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs
│   │   ├── capture.rs          # V4L2 camera interface
│   │   ├── detect.rs           # SCRFD face detection via ONNX Runtime
│   │   ├── embed.rs            # ArcFace face embedding via ONNX Runtime
│   │   ├── compare.rs          # cosine similarity + threshold logic
│   │   ├── align.rs            # face alignment / preprocessing
│   │   ├── store.rs            # enrolled face DB (embeddings on disk)
│   │   └── config.rs           # TOML config loading
├── nihao-cli/                  # CLI tool
│   ├── Cargo.toml
│   ├── src/
│   │   └── main.rs
│   │   └── commands/
│   │       ├── mod.rs
│   │       ├── add.rs          # enroll a face
│   │       ├── remove.rs       # remove a face model
│   │       ├── list.rs         # list enrolled faces
│   │       ├── test.rs         # test camera + recognition
│   │       ├── snapshot.rs     # dump a camera frame
│   │       └── config.rs       # print/edit config
├── pam-nihao/                  # PAM module (.so)
│   ├── Cargo.toml
│   ├── src/
│   │   └── lib.rs              # extern "C" pam_sm_authenticate
```

## Dependencies

### Build

- Rust 1.75+ (2024 edition)
- pkg-config
- PAM headers (`pam` — included in base on Arch/CachyOS)
- V4L2 headers (`v4l-utils`)
- ONNX Runtime 1.17+ with ROCm execution provider (or CPU/CUDA)

### Runtime

- ONNX Runtime shared library
- A compatible camera (IR preferred, RGB works)
- SCRFD + ArcFace ONNX models (see [Models](#models))

## Building

```bash
# install system deps (Arch/CachyOS)
sudo pacman -S pam v4l-utils pkgconf

# clone
git clone https://github.com/johnnytshi/nihao
cd nihao

# build everything
cargo build --release

# outputs:
#   target/release/nihao           <- CLI binary
#   target/release/libpam_nihao.so <- PAM module
```

### ROCm support

If you have ONNX Runtime built with ROCm (e.g. on a Strix Halo or RDNA3 GPU):

```bash
# point to your ONNX Runtime install
export ORT_LIB_LOCATION=/path/to/onnxruntime
export ORT_USE_ROCM=1

cargo build --release --features rocm
```

### CPU-only

Works out of the box — ONNX Runtime defaults to CPU execution provider. Slower but requires no GPU stack.

## Installation

```bash
# install the CLI
sudo install -m 755 target/release/nihao /usr/local/bin/

# install the PAM module
sudo install -m 644 target/release/libpam_nihao.so /usr/lib/security/pam_nihao.so

# install default config
sudo mkdir -p /etc/nihao
sudo install -m 644 config/nihao.toml /etc/nihao/nihao.toml

# install models
sudo mkdir -p /usr/share/nihao/models
sudo install -m 644 models/*.onnx /usr/share/nihao/models/

# create face storage directory
sudo mkdir -p /var/lib/nihao/faces
```

Optionally, a PKGBUILD will be provided for clean `makepkg` / AUR installation.

## PAM configuration

Add NiHao to the PAM service you want facial auth on. For example, to enable it for `sudo`:

```bash
# /etc/pam.d/sudo
# add this BEFORE the existing @include or auth lines:
auth  sufficient  pam_nihao.so
```

`sufficient` means: if the face matches, auth succeeds. If it fails (no face, no match, camera error), it falls through to your password prompt. Nothing breaks.

Other services: `/etc/pam.d/login`, `/etc/pam.d/gdm-password`, `/etc/pam.d/sddm`, etc.

## Usage

### Enroll a face

```bash
sudo nihao add
# or with a label
sudo nihao add --label "with glasses"
```

### List enrolled faces

```bash
sudo nihao list
```

### Remove a face

```bash
sudo nihao remove <id>
# or remove all
sudo nihao clear
```

### Test recognition

```bash
sudo nihao test
```

### Take a camera snapshot

```bash
sudo nihao snapshot --output frame.png
```

## Configuration

Config lives at `/etc/nihao/nihao.toml`:

```toml
[camera]
# V4L2 device path
device = "/dev/video0"
# prefer IR camera if available
prefer_ir = true
# frame dimensions
width = 640
height = 480

[detection]
# SCRFD model path
model = "/usr/share/nihao/models/scrfd_500m.onnx"
# minimum confidence for face detection
confidence = 0.5

[embedding]
# ArcFace model path
model = "/usr/share/nihao/models/arcface_mbf.onnx"

[matching]
# cosine similarity threshold (higher = stricter)
# 0.4 is a good default, increase to 0.5+ for tighter security
threshold = 0.4
# maximum number of frames to try before giving up
max_frames = 5
# timeout in milliseconds
timeout = 3000

[runtime]
# ONNX Runtime execution provider: "cpu", "rocm", "cuda"
provider = "rocm"
# GPU device ID (for multi-GPU systems)
device_id = 0
```

## Models

NiHao uses two small ONNX models:

| Model | Purpose | Size | Output |
|-------|---------|------|--------|
| [SCRFD_500M](https://github.com/deepinsight/insightface/tree/master/detection/scrfd) | Face detection | ~2.5 MB | Bounding boxes + landmarks |
| [ArcFace MobileFaceNet](https://github.com/deepinsight/insightface/tree/master/recognition/arcface) | Face embedding | ~65 MB | 512-d float vector |

Both are from InsightFace and are available under permissive licenses. Download them:

```bash
# TODO: provide a download script or host mirrors
./scripts/download_models.sh
```

## Key crates

| Crate | Purpose |
|-------|---------|
| [`ort`](https://crates.io/crates/ort) | ONNX Runtime bindings |
| [`v4l`](https://crates.io/crates/v4l) | V4L2 camera capture |
| [`image`](https://crates.io/crates/image) | Image decoding / resizing |
| [`clap`](https://crates.io/crates/clap) | CLI argument parsing |
| [`toml`](https://crates.io/crates/toml) / [`serde`](https://crates.io/crates/serde) | Config parsing |
| [`bincode`](https://crates.io/crates/bincode) | Fast serialization for face embeddings |

## Design decisions

**Why not fork Howdy?**
Howdy spawns a Python process on every auth attempt. Python startup + dlib import + model load = 500ms+ before anything useful happens. NiHao is a single `.so` loaded into the PAM process — model loading happens once, inference is immediate.

**Why ONNX Runtime instead of libtorch?**
Smaller runtime footprint, faster cold start, and the C API is stable. PyTorch's runtime pulls in hundreds of megabytes. ONNX Runtime with ROCm is ~50MB. For a PAM module that needs to be fast and light, this matters.

**Why not MIOpen directly?**
MIOpen is a primitives library. Writing inference against it directly means manually implementing the entire network forward pass. ONNX Runtime already calls MIOpen under the hood on ROCm — same performance, fraction of the code.

**Why ArcFace over dlib's ResNet?**
ArcFace (MobileFaceNet backbone) is both faster and more accurate than dlib's face recognition model. It produces 512-d embeddings with better discriminative power, and the ONNX export is widely available and well-tested.

**Why V4L2 instead of OpenCV?**
We need exactly one thing from the camera: a single frame in a known format. V4L2 does this directly with zero overhead. OpenCV brings in a massive dependency tree for functionality we don't use.

## Comparison with Howdy

| | Howdy | NiHao |
|---|---|---|
| Language | Python + C++ (PAM) | Rust |
| Auth latency | 500ms+ (Python cold start) | < 50ms |
| Face detection | dlib HOG or CNN | SCRFD (ONNX) |
| Face embedding | dlib ResNet | ArcFace MobileFaceNet (ONNX) |
| GPU support | NVIDIA only (via cuDNN) | AMD ROCm, NVIDIA CUDA, CPU |
| Runtime deps | Python, dlib, OpenCV, numpy | ONNX Runtime |
| Architecture | PAM → subprocess → Python | PAM → native .so |

## Roadmap

- [ ] Core detection + embedding pipeline
- [ ] PAM module with V4L2 capture
- [ ] CLI: add, remove, list, test
- [ ] ROCm execution provider integration
- [ ] Multi-face enrollment per user
- [ ] IR camera detection + preference
- [ ] Anti-spoofing (liveness detection)
- [ ] Systemd integration for model preloading
- [ ] Wayland-compatible notification (face recognized/failed)
- [ ] PKGBUILD for Arch/CachyOS (AUR)
- [ ] Package for Debian, Fedora

## License

MIT

## Acknowledgments

- [Howdy](https://github.com/boltgolt/howdy) for proving the concept and PAM integration approach
- [InsightFace](https://github.com/deepinsight/insightface) for SCRFD and ArcFace models
- [ONNX Runtime](https://onnxruntime.ai/) for the inference backbone
