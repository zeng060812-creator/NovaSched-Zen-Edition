#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
: "${ANDROID_NDK_HOME:?请设置 ANDROID_NDK_HOME（NDK r27d）}"
: "${CARGO:?请设置 CARGO 为 cargo 可执行文件路径}"
export PATH="$(dirname "$CARGO"):$PATH"

unset RUSTFLAGS
TEST_ARGS=()
if [ ! -r /proc/self/stat ]; then
  echo "Build host hides /proc; live-process identity regression requires a real Linux/Android host and is skipped here." >&2
  TEST_ARGS=(-- --skip process_identity::regression_tests::live_unrelated_process_is_not_accepted_as_daemon)
fi
"$CARGO" test --manifest-path "$PROJECT_DIR/native/Cargo.toml" --locked --offline "${TEST_ARGS[@]}"

export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android26-clang"
# 使用 bionic 静态 CRT；不使用早期会在 main() 前崩溃的 static-pie 链接脚本。
export RUSTFLAGS="-C target-feature=+crt-static -C relocation-model=static -C link-arg=-static -C link-arg=-Wl,-z,max-page-size=16384"
"$CARGO" build --manifest-path "$PROJECT_DIR/native/Cargo.toml" --target aarch64-linux-android --target-dir "$PROJECT_DIR/native/target-static" --release --locked --offline

BINARY="$PROJECT_DIR/native/target-static/aarch64-linux-android/release/novasched"
readelf -h "$BINARY" > "$PROJECT_DIR/native/target-static/elf-header.txt"
readelf -l "$BINARY" > "$PROJECT_DIR/native/target-static/elf-programs.txt"
readelf -d "$BINARY" > "$PROJECT_DIR/native/target-static/elf-dynamic.txt"
rg -q AArch64 "$PROJECT_DIR/native/target-static/elf-header.txt"
rg -q 'EXEC \(Executable file\)' "$PROJECT_DIR/native/target-static/elf-header.txt"
if rg -q INTERP "$PROJECT_DIR/native/target-static/elf-programs.txt"; then
  echo "ELF unexpectedly needs a dynamic interpreter" >&2
  exit 1
fi
if rg -q NEEDED "$PROJECT_DIR/native/target-static/elf-dynamic.txt"; then
  echo "ELF unexpectedly needs a shared library" >&2
  exit 1
fi
mkdir -p "$PROJECT_DIR/module-template/bin"
install -m 0755 "$BINARY" "$PROJECT_DIR/module-template/bin/novasched"
chmod 0755 "$PROJECT_DIR/module-template/"*.sh "$PROJECT_DIR/module-template/vtools/powercfg.sh"

python3 "$PROJECT_DIR/scripts/package-release.py"
