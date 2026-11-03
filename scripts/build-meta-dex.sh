#!/usr/bin/env bash
# 构建 NovaMeta.dex（十七：应用名称与图标读取）。
# 可复现构建：源码在 scripts/meta-dex/NovaMeta.java，工具为官方
# JDK (Temurin 17) 与 Android build-tools (r34) 的 javac/d8；
# 不下载任何预编译 DEX。工具路径可用环境变量覆盖。
set -euo pipefail
cd "$(dirname "$0")/.."

JAVAC="${JAVAC:-C:/Users/22310/NovaSched-tools/downloads/jdk17/jdk-17.0.20.1+1/bin/javac.exe}"
JAVA="${JAVA:-C:/Users/22310/NovaSched-tools/downloads/jdk17/jdk-17.0.20.1+1/bin/java.exe}"
D8_JAR="${D8_JAR:-C:/Users/22310/NovaSched-tools/downloads/build-tools-r34/android-14/lib/d8.jar}"
ANDROID_JAR="${ANDROID_JAR:-C:/Users/22310/NovaSched-tools/downloads/platform-34/android-34/android.jar}"

OUT_DIR="native/meta-dex-build"
mkdir -p "$OUT_DIR"

"$JAVAC" -source 11 -target 11 -bootclasspath "$ANDROID_JAR" \
  -d "$OUT_DIR" scripts/meta-dex/NovaMeta.java 2>&1 | grep -v 'bootstrap class path' || true

"$JAVA" -cp "$D8_JAR" com.android.tools.r8.D8 \
  --release --min-api 26 --output "$OUT_DIR" "$OUT_DIR"/*.class

# d8 默认输出 classes.dex；统一命名后交给打包脚本。
mv "$OUT_DIR/classes.dex" module-template/bin/NovaMeta.dex
echo "NovaMeta.dex $(stat -c%s module-template/bin/NovaMeta.dex) bytes"
