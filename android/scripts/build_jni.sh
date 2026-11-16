#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SDK_DIR="${ANDROID_HOME:-$HOME/android-sdk}"
NDK_DIR="${ANDROID_NDK_HOME:-$(ls -d "$SDK_DIR"/ndk/* | sort -V | tail -n 1)}"
TOOLCHAIN="$NDK_DIR/toolchains/llvm/prebuilt/linux-x86_64/bin"
API_LEVEL=26

export CC_aarch64_linux_android="$TOOLCHAIN/aarch64-linux-android${API_LEVEL}-clang"
export CXX_aarch64_linux_android="$TOOLCHAIN/aarch64-linux-android${API_LEVEL}-clang++"
export AR_aarch64_linux_android="$TOOLCHAIN/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$TOOLCHAIN/aarch64-linux-android${API_LEVEL}-clang"

cd "$ROOT_DIR"
cargo build --release -p portico-jni --target aarch64-linux-android

OUT_DIR="$ROOT_DIR/android/app/src/main/jniLibs/arm64-v8a"
mkdir -p "$OUT_DIR"
cp "$ROOT_DIR/target/aarch64-linux-android/release/libportico_jni.so" "$OUT_DIR/libportico_jni.so"
echo "Built $OUT_DIR/libportico_jni.so"
