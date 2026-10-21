# v2.11.1 验证范围

作者 ZenJooo。基线为 v2.10.3，配置代际保持 390。

## 本次修订的验证

处理器模板扩展到 12 颗：新增 SM8845（骁龙 8 Gen 5）与 MT6989（天玑 9300+ 极限性能策略），四档配置共 48 组，Python 与 Node 双实现生成结果逐字节一致（`generate-profiles.py --check` 与 `check-profiles.cjs` 双向通过），出厂指纹已同步进 `STOCK_FINGERPRINTS`。

原生代码在 Windows 主机完成 aarch64-linux-android 目标的 `cargo check`（零错误）与 `cargo test` 编译（链接通过）；Windows 无法执行 Android ELF，单元测试的运行验证仍需 Linux/macOS 主机或真机。作者基线的 234 项主机回归覆盖目录迁移、持锁延迟、冲突保留、配置与规则、Scene 接管和恢复、认证、协议、CPU 策略事务及节点回读；本次新增骁龙 8 Gen 5 / 天玑 9300+ 识别、伪 cpuinfo 频率回退、无障碍精确匹配等回归用例。

脚本侧替身测试在本机执行：模板功耗护栏 48 组、安装器八种管理器身份与失败中断、开机等待/禁用取消/损坏退出、云更新校验与回退、WebUI 更新与回滚、DOM/宿主场景、玻璃材质 WebGL 调用预算。Windows 沙箱进程创建较慢，部分替身测试的子进程超时上限按注释放宽，被测逻辑未变；WebGL/AGSL 画面对比需要 Mesa EGL，本机无此库时明确跳过。

已完成 Android arm64 release 构建：NDK r27d、API 26、bionic 静态 CRT；ELF 为 AArch64 EXEC，无 INTERP 段与 NEEDED 依赖，LOAD 段 16 KiB 对齐。刷入包通过 CRC、版本、执行权限、资产、目录白名单和 ELF 一致性检查。

## 待真机验证

没有连接 Android 设备：首次解锁、厂商 SELinux 策略、管理器实装（Magisk/Alpha/KernelSU 及分支/APatch 仅做能力检测）、Scene 版本差异、硬件触感、能耗与帧率表现、SM8845/MT6989 的节点写入与 GPU devfreq 行为均需真机验证。骁龙 8 Gen 5 的 walt 调速器名称来自同平台惯例，若内核未提供将自动回退 OEM 调速器。

这些检查覆盖已识别问题，不代表所有隐藏问题已清零，也不构成全部 ROM 和处理器的兼容承诺。

## v2.11.1 发布验证（2026-10-21）

在 Windows 主机使用 Rust 1.99.0 与 Android NDK r27d 直接交叉编译本版 AArch64 release ELF；确认 ELF 为静态 `EXEC`、不含 `INTERP` / `NEEDED`，四个 `LOAD` 段均为 16 KiB 对齐。`package-release.py` 生成 v2.11.1（versionCode 412）刷入 ZIP、源码 ZIP 与 SHA256 清单；40 项刷入包通过 CRC、元数据、可执行权限、协议及 ELF 一致性检查。WebUI 28 项断言、DOM/宿主场景测试、12 份模板字节级生成检查、48 组策略护栏检查通过。

本次未运行 Android 实机验证。Rust 单元测试未执行：Windows 的宿主目标不支持代码所用的 Unix API，当前环境亦无 Linux/WSL；Android 目标的 Rust 测试代码已通过 `cargo check --tests`。安装器和更新器测试尝试在随附 `sh.exe` 下运行，但当前 POSIX 环境缺少测试所需的 `python3`、`sleep` 等命令，因此不报告这些脚本测试通过。构建成功不等于实机兼容或功耗表现得到验证。
