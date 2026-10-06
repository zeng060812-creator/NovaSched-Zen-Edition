# NovaSched · Zen Edition

[![version](https://img.shields.io/badge/version-v1.3.1-green)](#安装)
[![platform](https://img.shields.io/badge/platform-arm64--v8a-blue)](#安装)
[![license](https://img.shields.io/badge/license-GPL--3.0--only-important)](LICENSE)

面向骁龙旗舰平台的用户态调度模块。按处理器与内核能力映射 CPU 策略，提供四档调度、应用单独规则、Scene 联动，以及一套自带液态玻璃视觉的 WebUI。不限制手机品牌或机型，支持 Magisk / Alpha / KernelSU 系列 / APatch，不按管理器版本号拦截。

> 作者：ZenJooo · 本仓库为完整源码；正式发布包（含预编译守护进程）随版本附 Release 或从 `scripts/build.sh` 自行构建。

## 支持的处理器

| 型号代码 | 平台 |
|---|---|
| SM8450 | 骁龙 8 Gen 1 |
| SM8475 | 骁龙 8+ Gen 1 |
| SM8550 | 骁龙 8 Gen 2 |
| SM8650 | 骁龙 8 Gen 3 |
| SM8750 | 骁龙 8 Elite |
| SM8850 | 骁龙 8 Elite Gen 5 |

安装时会自动识别处理器、CPU policy、cgroup 与可用调速器（只探测，不下发策略），不满足条件会中止安装并说明原因。

## 功能

- **四档调度**：省电 / 均衡 / 性能 / 极速，按 SoC 配置映射频率上下限与调速器。
- **应用单独规则**：给游戏或视频应用设定独立档位，进入前台时自动切换。
- **省电子选项**：流畅省电（保留限频，允许短时性能请求）与极限节能（进一步收紧资源上限），仅在省电档生效，默认关闭。
- **Scene 联动（双向同步）**：检测到已安装 Scene 时自动注册回调；调度执行由 Rust 守护进程完成，档位与应用规则在 Scene 与 WebUI 之间双向同步——任意一侧切换档位或修改规则，另一侧即时生效。仅当 Scene 的调度槽被其它调度器占用时，NovaSched 才暂停写入（外部接管）。
- **WebUI**：实时心跳、档位切换、应用规则管理、按需日志流、只读诊断；液态玻璃视觉，透明度可调，深浅色与减少动画跟随。
- **原厂快照**：首次启动自动保存原厂节点快照，卸载时 `restore` 恢复。

## 安装

1. 确认设备为 **arm64-v8a**，并已解锁 root（Magisk / Alpha / KernelSU 系列 / APatch 任一）。
2. 若安装过旧模块 `LittleYouran_CTS_Rust`，请先在管理器中停用并卸载，重启后再安装本模块。
 3. 在管理器中刷入 `NovaSched-v*-release.zip`。
4. 安装程序会依次执行二进制自检（`self-test`）、硬件探测（`probe`）、配置初始化（`prepare-config`）；任何一步失败都会中止安装并保留输出与原配置。
5. 覆盖安装后**必须重启手机**，以退出旧版进程并加载新版本。

安装产物与运行时目录：

| 路径 | 说明 |
|---|---|
| `/data/adb/novasched/novasched.log` | 内部运行日志 |
| `/data/adb/novasched/boot-entry.log` | 开机入口日志 |
| `/data/adb/novasched/options.txt` | 用户选项（覆盖安装保留） |
| `/sdcard/Android/NovaSched/log.txt` | 外部日志镜像（不可用时静默跳过） |

## WebUI 使用

在管理器的模块 WebUI 入口打开。没有内置 WebUI 入口的管理器，可用 **KsuWebUI 独立版**或 **WebUI X** 打开本模块，并授予宿主 root／Shell 权限。

- **概览**：当前生效档位、前台应用识别、控制来源（WebUI 接管 / Scene 接管 / Scene 回调）。
- **应用**：搜索已安装应用创建单独规则，支持应用名与包名搜索、手动输入包名。
- **日志**：进入日志页才按需连接，支持级别过滤、关键字搜索、复制与导出。
- **设置**：液态玻璃开关与**玻璃透明度**滑杆（0–100%，拖动实时预览、松手保存）、外观（跟随系统 / 浅色 / 深色）、减少动画。
- **诊断**：读取真实状态、最近下发错误，并可执行只读诊断命令。

连接架构：页面优先通过本地 WebSocket（`ws://127.0.0.1:31415`）直连守护进程；当 WebView 不允许本地回环连接时，自动改走已授权宿主的 root 备用通道（`webui-rpc`），无需任何手动配置。

## 命令行

守护进程二进制 `bin/novasched`（需要 root shell）：

```sh
novasched status           # 运行状态与真实心跳
novasched diagnose         # 读取开机入口、进程身份与启动失败记录
novasched game-diagnose    # 采样约 10 秒，读取实际节点与温度
novasched scene-diagnose   # Scene 检测与联动状态
novasched reload           # 重新加载配置（SIGHUP）
novasched stop / restore   # 停止调度 / 恢复原厂节点快照
novasched scene-install / scene-restore   # 手动注册 / 还原 Scene 联动
novasched probe / check-config / prepare-config / self-test / version
```

以上命令均可加 `--module-dir <模块目录>` 指定模块路径；诊断类命令同样适用于 MT 管理器终端。

## 安全设计

- WebUI 凭据（端口 + 64 位 token）由守护进程以 root 签发，仅保存在 `0600` 权限文件中，签发前双重校验进程身份与心跳；页面来源必须为受信任的 HTTPS 管理器来源或本机回环 HTTP，token 通过 WebSocket 子协议传递、不进入 URL。
- 本地 WebSocket 校验协议标识、token 与 Origin 白名单；不提供未认证的回退端点。UI 侧对所有失败路径保持 fail-closed：凭据不完整、来源不匹配、心跳过期一律拒绝上线。
- 源码中的 `KEY` / `AUTH_PREFIX` 为公开协议标识，不是认证秘密；真正的凭据每轮守护进程会话随机生成并轮换。

## 从源码构建

依赖：Rust（`rust-version = "1.75"`）、Android NDK（r27d 或更高）、Python 3、bash（Linux/WSL；Windows 亦可用 NDK 的 llvm-readelf 完成 ELF 校验）。

```sh
# 交叉编译 AArch64 静态 ELF（16KB 页对齐），并完成全部打包校验
ANDROID_NDK_HOME=/path/to/ndk CARGO=/path/to/cargo bash scripts/build.sh
```

`build.sh` 会：运行 Rust 单元测试 → 交叉编译静态链接的 `aarch64-linux-android` 二进制（`crt-static`、`max-page-size=16384`）→ 用 readelf 校验 ELF 结构（AArch64、EXEC、无解释器/动态依赖、LOAD 段 16KB 对齐）→ 将产物复制进 `module-template/bin/` → 执行 `scripts/package-release.py` 输出 `dist/` 下的刷机包、源码包与 SHA256SUMS，并校验 CRC、可执行权限位、协议密钥与配置完整性。

WebUI 回归测试（无需 Android 设备）：

```sh
npm install --ignore-scripts
npm run test:webui    # 89 项 DOM/协议/静态检查
```

## 仓库结构

```
module-template/   模块模板（刷机包内容：脚本、配置、WebUI、META-INF）
native/            Rust 调度核心与守护进程（含 WebSocket 服务、凭据签发、Scene 联动）
scripts/           构建、打包、配置生成与测试脚本
```

## 常见问题

- **WebUI 显示「未连接」**：先在管理器确认已授予模块宿主 root／Shell 权限；打开「诊断」执行只读诊断，或用终端查看 `novasched status` 与 `/data/adb/novasched/boot-entry.log`。若守护进程正常而页面提示「无法读取守护连接状态」，请更新到包含连接修复的最新版本。
- **覆盖安装后界面仍是旧版**：WebUI 文件随模块更新，重启后重开页面；`config.mmrl.json` 已关闭缓存。
- **Scene 已安装但未联动**：安装模块时若 Scene 已存在会自动注册；之后可用 `novasched scene-install` 手动注册、`novasched scene-restore` 还原。联动后档位与应用规则在 Scene 和 WebUI 之间双向同步；仅当 Scene 的调度槽被其它调度器占用（外部接管）时才需要先在 Scene 中停用它。

## 许可

GPL-3.0-only，详见 [LICENSE](LICENSE)；第三方图标授权见 [NOTICE](NOTICE) 与 `module-template/webroot/licenses/`。

**免责声明**：本模块需要 root 并直接读写 CPU 频率与调度节点。请理解所在设备的保修与刷机风险后再使用，作者不对不当使用造成的任何后果负责。
