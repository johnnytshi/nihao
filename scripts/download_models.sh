#!/bin/bash
# Download face detection and embedding models from InsightFace (buffalo_l)

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
MODEL_DIR="$SCRIPT_DIR/../models"

GREEN='\033[0;32m'
NC='\033[0m'

mkdir -p "$MODEL_DIR"

URL="https://github.com/deepinsight/insightface/releases/download/v0.7/buffalo_l.zip"
TMP_ZIP=$(mktemp /tmp/buffalo_l_XXXXXX.zip)

cleanup() { rm -f "$TMP_ZIP"; }
trap cleanup EXIT

echo "Downloading models from InsightFace..."
curl -L --progress-bar -o "$TMP_ZIP" "$URL"

echo "Extracting models..."
unzip -jo "$TMP_ZIP" "buffalo_l/det_10g.onnx" "buffalo_l/w600k_r50.onnx" -d "$MODEL_DIR"

mv "$MODEL_DIR/det_10g.onnx" "$MODEL_DIR/scrfd_500m.onnx"
mv "$MODEL_DIR/w600k_r50.onnx" "$MODEL_DIR/arcface_mobilefacenet.onnx"

echo -e "${GREEN}Models downloaded to $MODEL_DIR${NC}"
ls -lh "$MODEL_DIR"/scrfd_500m.onnx "$MODEL_DIR"/arcface_mobilefacenet.onnx
