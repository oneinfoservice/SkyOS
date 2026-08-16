#!/bin/bash
set -e

# Shell script to create bootimage-velox_kernel.bin
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )"
cd "$SCRIPT_DIR/kernel"

echo "--- Velox Kernel Bootimage Builder ---"

# 1. Build the kernel
echo "Building kernel..."
cd kernel
cargo build -Zbuild-std=core,alloc --target x86_64-unknown-none
cd ..

# 2. Run the image builder
echo "Running image builder..."
cargo run --manifest-path builder/Cargo.toml

# 3. Check output
OUTPUT_BINARY="bootimage-velox_kernel.bin"
if [ -f "$OUTPUT_BINARY" ]; then
    echo "SUCCESS: Created $OUTPUT_BINARY in the root folder."
else
    echo "ERROR: Could not find output at $OUTPUT_BINARY"
    exit 1
fi
