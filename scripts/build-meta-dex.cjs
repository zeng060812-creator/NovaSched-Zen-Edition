// 构建 NovaMeta.dex：javac + d8（官方 JDK17 + build-tools r34）。
// 与 scripts/build-meta-dex.sh 等价；Windows 下由 node 驱动以规避 bash 环境问题。
const fs = require('fs');
const path = require('path');
const {execFileSync} = require('child_process');

const ROOT = path.join(__dirname, '..');
const DL = 'C:/Users/22310/NovaSched-tools/downloads';
const JAVAC = process.env.JAVAC || `${DL}/jdk17/jdk-17.0.20.1+1/bin/javac.exe`;
const JAVA = process.env.JAVA || `${DL}/jdk17/jdk-17.0.20.1+1/bin/java.exe`;
const D8_JAR = process.env.D8_JAR || `${DL}/build-tools-r34/android-14/lib/d8.jar`;
const ANDROID_JAR = process.env.ANDROID_JAR || `${DL}/platform-34/android-34/android.jar`;
const OUT_DIR = path.join(ROOT, 'native/meta-dex-build');
const SRC = path.join(ROOT, 'scripts/meta-dex/NovaMeta.java');
const DEST = path.join(ROOT, 'module-template/bin/NovaMeta.dex');

fs.mkdirSync(OUT_DIR, {recursive: true});
console.log('javac…');
// JDK17 javac 禁用 -bootclasspath；用 -classpath 挂 android.jar，
// -source/-target 11 + 警告容忍（android.jar 提供android.* API）。
execFileSync(JAVAC, ['-source', '11', '-target', '11', '-classpath', ANDROID_JAR,
  '-encoding', 'UTF-8', '-nowarn', '-d', OUT_DIR, SRC], {stdio: ['ignore', 'ignore', 'inherit']});
console.log('d8…');
const classes = fs.readdirSync(OUT_DIR).filter(f => f.endsWith('.class')).map(f => path.join(OUT_DIR, f));
execFileSync(JAVA, ['-cp', D8_JAR, 'com.android.tools.r8.D8',
  '--release', '--min-api', '26', '--output', OUT_DIR, ...classes],
  {stdio: ['ignore', 'ignore', 'inherit']});
const dex = path.join(OUT_DIR, 'classes.dex');
fs.copyFileSync(dex, DEST);
console.log('NovaMeta.dex', fs.statSync(DEST).size, 'bytes');
