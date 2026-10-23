# NovaSched Zen Edition v2.12.0

作者：**ZenJooo**。Android arm64 root 模块，提供四档 CPU 策略、应用规则、自动调度、NovaFAS 与 iOS 液态玻璃风格的 WebUI。

## 安装

在 Magisk / Alpha、KernelSU 及其兼容分支、APatch 管理器中刷入 `NovaSched-v2.12.0-release.zip`，然后重启。兼容性根据实际执行与命令能力判断，不使用管理器名称或版本白名单。首次解锁后按有界门控等待稳定再启动服务；解锁确认异常时放弃本次自启并保留诊断，WebUI 打开会显示具体原因并可补启动。

升级保留配置、应用规则与自动调度偏好。旧服务仍在运行时，目录迁移推迟到重启后；重复文件会保留并报告，避免覆盖用户数据。

处理器模板：SM8450、SM8475、SM8550、SM8650、SM8750、SM8850、SM8845（骁龙 8 Gen 5）、天玑 8350 / 8400 / 9300+（极限性能策略）/ 9400+、玄戒 O3。型号和拓扑均需符合准入规则，共享平台码不能单独证明型号。三星使用已有的保守策略。骁龙 8 Gen 5 与天玑 9300+ 默认启用 GPU devfreq 档位（节点缺失时仅告警降级，不影响 CPU 调度）；SM8650 携带默认停用的 GPU 候选包，验证状态与数据来源见[处理器适配表](docs/processor-adaptation.md)。

O3 安装修复：v2.11.2 支持检测 CPU8、CPU9，并从内核读取十核设备的实际 policy。优先按 CPU capacity 映射，缺少该接口时会在安装记录中注明回退。旧八核参考配置保留兼容，不代表已完成所有 O3 固件的实机调优。

## Scene

检测到 Scene 后自动注册 NovaSched 调度入口，四档和应用规则继续可用。**底座模式已移除**，底座请求不会强制切到极速，也不会执行原来的 Scene LP 调度脚本。首次原入口只用于卸载恢复。

## 目录

模块本体保留管理器要求的标准结构，运行数据集中到 **`/data/adb/novasched/`**。

| 目录 | 内容 |
| --- | --- |
| `config/` | 调度配置、应用规则、自动调度偏好 |
| `run/` | 状态、锁、会话及操作期间的临时文件 |
| `logs/` | 启动、监督、安装与运行记录 |
| `restore/` | 当前开机原厂快照、首次 Scene 原入口、最近一份配置回滚副本 |

运行和安装日志各限 1 MiB，不再生成轮转备份，也不向共享存储重复写日志。升级整理已知历史文件，用户自行加入的未知文件不会随意删除。安装票据和更新临时文件在完成或可捕获的失败后移除。卸载恢复失败时保留恢复数据并报告原因，供重试。

## WebUI

提供四页导航、逐页滚动位置、玻璃外观、壁纸取景、日志和诊断。切页、弹窗和面板手势支持中断；后台和减少动态效果时释放动画。弹窗按当前层级管理交互与焦点，旧请求不会误关新弹窗。

触感优先使用宿主原生接口；宿主与 Android 提供兼容能力时，使用遵循系统设置的触感命令，不强制开启振动。其它环境回退到 Web Vibration，实际效果由设备和宿主决定。

云更新脚本随模块打包，校验官方 SHA256、模块身份和目标版本后，再交给管理器安装。无法读取官方校验文件时不会自动安装，可打开 Release 页面手动下载。

## 诊断

在 root 终端执行：

```sh
/data/adb/modules/NovaSched_Zen_Edition/bin/novasched diagnose
```

管理器使用不同模块路径时，换成实际安装目录。WebUI 设置也提供诊断入口。

## 构建

需要 Python 3.11+、Node.js 20+、Rust 1.75+、Android arm64 标准库和 NDK r27d：

```sh
bash scripts/validate.sh
CARGO=/path/to/cargo ANDROID_NDK_HOME=/path/to/android-ndk-r27d bash scripts/build.sh
```

`dist/` 输出刷入包、单目录源码包及 SHA256 文件。源码包不包含目标文件、依赖目录、历史测试报告和设备运行数据。验证范围见 [docs/validation.md](docs/validation.md)，本次修订的审计结论与未验证问题见 [docs/v2.12.0-report.md](docs/v2.12.0-report.md)，SM8650 专项调整与对比测试见 [docs/sm8650-ab-test.md](docs/sm8650-ab-test.md)。

核心许可 GPL-3.0-only；界面玻璃折射公式与 Apache-2.0 署名见 NOTICE 和随包许可文件。
