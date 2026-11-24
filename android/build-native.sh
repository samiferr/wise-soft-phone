#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
: "${ANDROID_NDK_HOME:?Set ANDROID_NDK_HOME to Android NDK r28c}"
case "$(uname -s)" in
  Linux) host=linux-x86_64 ;;
  Darwin) host=darwin-x86_64 ;;
  *) echo 'Build the native library on Linux or macOS.' >&2; exit 1 ;;
esac
llvm="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$host/bin"
export CC_aarch64_linux_android="$llvm/aarch64-linux-android28-clang"
export CXX_aarch64_linux_android="$llvm/aarch64-linux-android28-clang++"
export AR_aarch64_linux_android="$llvm/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC_aarch64_linux_android"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384"
export CARGO_TARGET_DIR="$PWD/target"
cargo build --locked --lib --release --target aarch64-linux-android
destination="$CARGO_TARGET_DIR/android-jni/arm64-v8a"
mkdir -p "$destination"
cp "$CARGO_TARGET_DIR/aarch64-linux-android/release/libsoftphone.so" "$destination/"
