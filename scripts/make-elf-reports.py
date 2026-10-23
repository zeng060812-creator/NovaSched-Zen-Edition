"""Windows host helper: write readelf reports for package-release.py.

build.sh does this with shell redirection on Linux; keep the same artifacts so
packaging assertions stay identical across hosts. All arguments are derived
from ANDROID_NDK_HOME or the repository layout; nothing is interpolated from
user input and no shell is involved (subprocess argument-list form).
"""
from pathlib import Path
import os
import subprocess

root = Path(__file__).resolve().parents[1]
out_dir = root / "native/target-static"
binary = root / "native/target-static/aarch64-linux-android/release/novasched"

ndk_home = os.environ.get("ANDROID_NDK_HOME")
if not ndk_home:
    raise SystemExit("Set ANDROID_NDK_HOME (NDK r27d) to write readelf reports")
readelf_exe = Path(ndk_home) / "toolchains/llvm/prebuilt/windows-x86_64/bin/llvm-readelf.exe"
if not readelf_exe.is_file():
    raise SystemExit(f"llvm-readelf not found: {readelf_exe}")

for flag, name in [("-h", "elf-header.txt"), ("-l", "elf-programs.txt"), ("-d", "elf-dynamic.txt")]:
    result = subprocess.run(
        [str(readelf_exe), flag, str(binary)],
        capture_output=True,
        check=True,
        shell=False,
    )
    data = result.stdout.decode("utf-8", errors="strict")
    (out_dir / name).write_text(data, encoding="utf-8")
    print(name, len(data), "bytes")
print("ok")
