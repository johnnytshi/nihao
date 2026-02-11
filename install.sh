#!/bin/bash
# NiHao Face Recognition - Install Script

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}NiHao Face Recognition - Installation${NC}"
echo

# Check if running as root
if [ "$EUID" -ne 0 ]; then
    echo -e "${RED}Error: This script must be run as root (use sudo)${NC}"
    exit 1
fi

# Check if already installed
if [ -f /lib/security/pam_nihao.so ]; then
    echo -e "${YELLOW}Warning: NiHao appears to be already installed.${NC}"
    echo "PAM module exists at: /lib/security/pam_nihao.so"
    echo
    read -p "Continue anyway? (y/N) " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo -e "${YELLOW}Installation cancelled.${NC}"
        exit 0
    fi
    echo
fi

echo "This will install NiHao face authentication on your system."
echo
echo "Prerequisites:"
echo "  - Rust toolchain (cargo)"
echo "  - ONNX Runtime (onnxruntime-cpu package)"
echo "  - V4L2 utilities (v4l-utils)"
echo "  - PAM development headers (pam-devel or libpam0g-dev)"
echo
echo "Installation will:"
echo "  1. Build the project with Cargo"
echo "  2. Install models to /usr/share/nihao/models/"
echo "  3. Create system config at /etc/nihao/nihao.toml"
echo "  4. Create face storage at /var/lib/nihao/faces/"
echo "  5. Install PAM module to /lib/security/pam_nihao.so"
echo "  6. Install CLI binary to /usr/local/bin/nihao"
echo "  7. Configure PAM in /etc/pam.d/system-auth"
echo

read -p "Continue with installation? (y/N) " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo -e "${YELLOW}Installation cancelled.${NC}"
    exit 0
fi

echo

# Get the actual user who invoked sudo
ACTUAL_USER="${SUDO_USER:-$USER}"
if [ "$ACTUAL_USER" = "root" ]; then
    echo -e "${RED}Error: Cannot determine the actual user. Please run with sudo, not as root.${NC}"
    exit 1
fi

# Get user's home directory
USER_HOME=$(getent passwd "$ACTUAL_USER" | cut -d: -f6)

# 0. Check prerequisites
echo -e "${BLUE}[0/7] Checking prerequisites...${NC}"

if ! command -v cargo &> /dev/null; then
    echo -e "${RED}✗ Rust/Cargo not found. Please install: sudo pacman -S rust${NC}"
    exit 1
fi

if ! ldconfig -p | grep -q libonnxruntime; then
    echo -e "${RED}✗ ONNX Runtime not found. Please install: sudo pacman -S onnxruntime-cpu${NC}"
    exit 1
fi

if ! command -v v4l2-ctl &> /dev/null; then
    echo -e "${RED}✗ V4L2 utilities not found. Please install: sudo pacman -S v4l-utils${NC}"
    exit 1
fi

echo -e "${GREEN}✓ All prerequisites met${NC}"

# 1. Build project
echo -e "${BLUE}[1/7] Building project...${NC}"
sudo -u "$ACTUAL_USER" cargo build --release
echo -e "${GREEN}✓ Build complete${NC}"

# 2. Download and install models
echo -e "${BLUE}[2/7] Downloading and installing models...${NC}"

if [ ! -f scripts/download_models.sh ]; then
    echo -e "${RED}✗ Download script not found at scripts/download_models.sh${NC}"
    exit 1
fi

# Always download to ensure we have the latest models
sudo -u "$ACTUAL_USER" bash scripts/download_models.sh

mkdir -p /usr/share/nihao/models
cp -L models/scrfd_500m.onnx /usr/share/nihao/models/
cp -L models/arcface_mobilefacenet.onnx /usr/share/nihao/models/
chmod 644 /usr/share/nihao/models/*.onnx
echo -e "${GREEN}✓ Models installed to /usr/share/nihao/models/${NC}"

# 3. Create system config
echo -e "${BLUE}[3/7] Creating system configuration...${NC}"

# Detect camera device
CAMERA_DEVICE="/dev/video0"
if [ -e "/dev/video2" ]; then
    CAMERA_DEVICE="/dev/video2"
fi

mkdir -p /etc/nihao
cat > /etc/nihao/nihao.toml <<EOF
[camera]
device = "$CAMERA_DEVICE"
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

chmod 644 /etc/nihao/nihao.toml
echo -e "${GREEN}✓ Config created at /etc/nihao/nihao.toml${NC}"
echo -e "   Camera device: $CAMERA_DEVICE"

# 4. Create face storage directory
echo -e "${BLUE}[4/7] Creating face storage...${NC}"
mkdir -p /var/lib/nihao/faces
chmod 755 /var/lib/nihao/faces
echo -e "${GREEN}✓ Face storage created at /var/lib/nihao/faces/${NC}"

# 5. Install PAM module
echo -e "${BLUE}[5/7] Installing PAM module...${NC}"
cp target/release/libpam_nihao.so /lib/security/pam_nihao.so
chmod 755 /lib/security/pam_nihao.so
echo -e "${GREEN}✓ PAM module installed to /lib/security/pam_nihao.so${NC}"

# 6. Install CLI binary
echo -e "${BLUE}[6/7] Installing CLI binary...${NC}"
cp target/release/nihao /usr/local/bin/nihao
chmod 755 /usr/local/bin/nihao
echo -e "${GREEN}✓ CLI binary installed to /usr/local/bin/nihao${NC}"

# 7. Configure PAM
echo -e "${BLUE}[7/7] Configuring PAM...${NC}"

if [ ! -f /etc/pam.d/system-auth ]; then
    echo -e "${YELLOW}⚠ /etc/pam.d/system-auth not found (might be different on your distro)${NC}"
    echo "You'll need to manually add this line to your PAM config:"
    echo "auth       sufficient                  pam_nihao.so"
else
    if grep -q "pam_nihao.so" /etc/pam.d/system-auth; then
        echo -e "${YELLOW}⚠ PAM configuration already exists${NC}"
    else
        # Create backup
        cp /etc/pam.d/system-auth /etc/pam.d/system-auth.backup

        # Add pam_nihao.so after #%PAM-1.0 line
        sed -i '/^#%PAM-1.0/a auth       sufficient                  pam_nihao.so' /etc/pam.d/system-auth

        echo -e "${GREEN}✓ PAM configured in /etc/pam.d/system-auth${NC}"
        echo -e "   (Backup saved to /etc/pam.d/system-auth.backup)"
    fi
fi

echo
echo -e "${GREEN}✓ Installation complete!${NC}"
echo
echo -e "${YELLOW}Next steps:${NC}"
echo
echo "1. Enroll your face:"
echo "   sudo -u $ACTUAL_USER nihao add"
echo
echo "2. Test authentication:"
echo "   sudo -u $ACTUAL_USER nihao test"
echo
echo "3. Try sudo with face auth:"
echo "   sudo -k  # Clear credential cache"
echo "   sudo echo 'Testing face auth...'"
echo
echo "Your camera should activate and authenticate you in ~1 second!"
echo "If face auth fails, you'll get a password prompt (fallback always works)."
echo
echo -e "${BLUE}Camera detected:${NC} $CAMERA_DEVICE"
echo -e "${BLUE}Models location:${NC} /usr/share/nihao/models/"
echo -e "${BLUE}Config location:${NC} /etc/nihao/nihao.toml"
echo -e "${BLUE}Face storage:${NC} /var/lib/nihao/faces/"
echo
