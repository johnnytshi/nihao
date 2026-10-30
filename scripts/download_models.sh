#!/bin/bash
# Fetch the separately licensed InsightFace buffalo_l detection/embedding weights.
set -euo pipefail
umask 077

usage() {
    cat <<'EOF'
Usage: download_models.sh [--accept-license] [--output-dir DIRECTORY]
       nihao-download-models --install [--accept-license]

Download and verify the InsightFace buffalo_l models used by NiHao.
  --install          Install root-owned models into /usr/share/nihao/models (root only).
  --output-dir DIR   Download into DIR instead of the source checkout's models directory.
  --accept-license  Acknowledge the upstream model terms without an interactive prompt.
  --help            Show this help.

InsightFace pretrained weights are for non-commercial research purposes only;
they are separate from NiHao's MIT-licensed source code. Review the terms at:
https://github.com/deepinsight/insightface/blob/master/README.md#license
EOF
}

die() { printf 'Error: %s\n' "$*" >&2; exit 1; }

INSTALL_SYSTEM=false
ACCEPT_LICENSE=false
OUTPUT_DIR=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --install) INSTALL_SYSTEM=true ;;
        --accept-license) ACCEPT_LICENSE=true ;;
        --output-dir)
            [ "$#" -ge 2 ] && [ -n "$2" ] || die '--output-dir requires a directory'
            OUTPUT_DIR="$2"
            shift
            ;;
        --help|-h) usage; exit 0 ;;
        *) usage >&2; die "Unknown option: $1" ;;
    esac
    shift
done

if "$INSTALL_SYSTEM"; then
    [ -z "$OUTPUT_DIR" ] || die '--install and --output-dir cannot be combined'
    [ "$EUID" -eq 0 ] || die 'Use sudo nihao-download-models --install to install system models'
    MODEL_DIR=/usr/share/nihao/models
elif [ -n "$OUTPUT_DIR" ]; then
    MODEL_DIR="$OUTPUT_DIR"
else
    SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
    if [ "$(basename -- "${BASH_SOURCE[0]}")" = nihao-download-models ]; then
        usage >&2
        die 'Choose --install for system models or --output-dir DIRECTORY'
    fi
    MODEL_DIR="$SCRIPT_DIR/../models"
fi

cat <<'EOF'
InsightFace pretrained model weights are available for non-commercial research
purposes only. These model terms are separate from NiHao's MIT code license.
https://github.com/deepinsight/insightface/blob/master/README.md#license
Continue only if your use is permitted by these terms or a separate InsightFace license.
EOF
if ! "$ACCEPT_LICENSE"; then
    [ -t 0 ] || die 'Explicit model license acknowledgement required: pass --accept-license after reviewing the terms'
    read -r -p 'Acknowledge the upstream model license terms? [y/N] ' answer
    case "$answer" in
        y|Y|yes|YES) ;;
        *) die 'Model download cancelled; no files changed' ;;
    esac
fi

for command in curl unzip sha256sum install mktemp mv; do
    command -v "$command" >/dev/null || die "Required command not found: $command"
done

# Do not let a writable directory or symlink redirect a privileged installation.
# Check every fixed parent before creating the next directory beneath it.
check_system_directory() {
    local directory="$1" owner mode
    [ -d "$directory" ] && [ ! -L "$directory" ] || die "Not a trusted system directory: $directory"
    owner=$(stat -c '%u' -- "$directory")
    mode=$(stat -c '%a' -- "$directory")
    [ "$owner" -eq 0 ] && (( (8#$mode & 022) == 0 )) || die "System directory must be root-owned and not group/other writable: $directory"
}
if "$INSTALL_SYSTEM"; then
    for directory in / /usr /usr/share; do
        check_system_directory "$directory"
    done
    for directory in /usr/share/nihao /usr/share/nihao/models; do
        if [ -e "$directory" ] || [ -L "$directory" ]; then
            check_system_directory "$directory"
        fi
    done
fi

if "$INSTALL_SYSTEM"; then
    # Ignore caller-selected TMPDIR for privileged writes. The sticky /tmp
    # parent protects this root-owned private directory from replacement.
    TMP_DIR=$(mktemp -d /tmp/nihao-models.XXXXXXXX)
else
    TMP_DIR=$(mktemp -d)
fi
STAGE_DIR=""
cleanup() {
    [ -z "$STAGE_DIR" ] || rm -rf -- "$STAGE_DIR"
    rm -rf -- "$TMP_DIR"
}
trap cleanup EXIT

URL=https://github.com/deepinsight/insightface/releases/download/v0.7/buffalo_l.zip
printf 'Downloading InsightFace buffalo_l models...\n'
curl --fail --location --retry 3 --show-error --output "$TMP_DIR/buffalo_l.zip" "$URL"

# Some published archives use flat entries; others have a buffalo_l/ prefix.
# Stream only the two exact allowed members, never archive-provided paths.
entries=$(unzip -Z1 "$TMP_DIR/buffalo_l.zip")
extract_model() {
    local name="$1" entry found=""
    while IFS= read -r entry; do
        case "$entry" in
            "$name"|"buffalo_l/$name")
                [ -z "$found" ] || die "Duplicate model in archive: $name"
                found="$entry"
                ;;
        esac
    done <<< "$entries"
    [ -n "$found" ] || die "Model missing from archive: $name"
    unzip -p "$TMP_DIR/buffalo_l.zip" "$found" > "$TMP_DIR/$name"
}
extract_model det_10g.onnx
extract_model w600k_r50.onnx

# Verified against a fresh upstream v0.7 buffalo_l.zip and the existing models.
# The historical NiHao filenames are aliases for SCRFD-10G and ResNet50 weights.
(
    cd -- "$TMP_DIR"
    sha256sum --check <<'EOF'
5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91  det_10g.onnx
4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43  w600k_r50.onnx
EOF
)

if "$INSTALL_SYSTEM"; then
    for directory in /usr/share/nihao /usr/share/nihao/models; do
        if [ ! -e "$directory" ] && [ ! -L "$directory" ]; then
            mkdir -m 0755 -- "$directory"
        fi
        check_system_directory "$directory"
    done
else
    mkdir -p -- "$MODEL_DIR"
fi

# Prepare both complete verified files on the destination filesystem, then
# atomically replace each name (including old aliases) without following it.
STAGE_DIR=$(mktemp -d "$MODEL_DIR/.nihao-models.XXXXXXXX")
if "$INSTALL_SYSTEM"; then
    install -o root -g root -m 0644 -- "$TMP_DIR/det_10g.onnx" "$STAGE_DIR/scrfd_500m.onnx"
    install -o root -g root -m 0644 -- "$TMP_DIR/w600k_r50.onnx" "$STAGE_DIR/arcface_mobilefacenet.onnx"
else
    install -m 0644 -- "$TMP_DIR/det_10g.onnx" "$STAGE_DIR/scrfd_500m.onnx"
    install -m 0644 -- "$TMP_DIR/w600k_r50.onnx" "$STAGE_DIR/arcface_mobilefacenet.onnx"
fi
mv -fT -- "$STAGE_DIR/scrfd_500m.onnx" "$MODEL_DIR/scrfd_500m.onnx"
mv -fT -- "$STAGE_DIR/arcface_mobilefacenet.onnx" "$MODEL_DIR/arcface_mobilefacenet.onnx"
printf 'Verified models installed in %s\n' "$MODEL_DIR"
