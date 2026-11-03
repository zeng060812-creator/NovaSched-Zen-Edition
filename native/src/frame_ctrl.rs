//! ZenJooo · NovaFAS 帧感知调度引擎。
//!
//! 读取前台应用的渲染帧信号（SurfaceFlinger timestats），在档位包络内动态
//! 抬升/回落 top-app 利用率下限与各簇最低频率：掉帧快速加压，稳定后缓慢
//! 回落，把功耗压到“刚好够用”。帧信号不可用时退化为 GPU 负载启发式。
//! 检测到 Scene 的 fas-rs 时按配置将其暂停（SIGSTOP），由本引擎接管；
//! 停止或退出时恢复（SIGCONT），其模式节点从不销毁。
//!
//! 引擎只写两类节点：top-app cpu.uclamp.min 与各 policy 的 scaling_min_freq。
//! 频率下限以守护进程下发的档位 floor 为底、以内核当前实际最高频率为顶，
//! 因此天然跟随档位切换与外部调度器的改动，无需感知配置文本。
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::config::{FasConfig, FasMode};
use crate::logging::Logger;
use crate::scheduler::{normalize_frequency, CpuctlLive};
use crate::snapshot::Snapshot;
use crate::util;

const SIGCONT: i32 = 18;
const SIGSTOP: i32 = 19;

/// 一次帧统计：前台图层的平均帧率与丢帧比例。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameSample {
    pub fps: f32,
    /// droppedFrames / totalFrames，0..1。
    pub dropped: f32,
}

/// 引擎运行状态，供 WebUI 与诊断展示。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EngineStatus {
    /// off（配置关闭）/ idle（待命）/ active（有有效信号）/ degraded（无有效信号）/ paused（外部调度接管）
    pub state: &'static str,
    /// timestats / heuristic / none
    pub source: &'static str,
    pub reason: &'static str,
    pub fps: f32,
    /// 帧信号不可用时的 GPU 忙碌百分比（0..100），供启发式展示。
    pub gpu: f32,
    pub boost: u32,
    pub override_active: bool,
    /// 用户设置的调度目标 FPS（0 = 未设置）。仅影响 NovaFAS 的加压判定，
    /// 不是强制限帧。
    pub target: u32,
    /// 引擎线程自启动以来的循环拍数：每拍 +1，WebUI 据此区分
    /// “引擎活着但数值稳定” 与 “引擎已经卡死”。
    pub ticks: u64,
}

/// 主循环每个 tick 发布的共享调度上下文。
#[derive(Clone, Debug, Default)]
pub struct FasShared {
    pub mode: String,
    pub package: String,
    pub allowed: bool,
    pub ready: bool,
    /// 当前档位各簇最低频率（内核适配后的 kHz 字符串），未启用簇为空。
    pub min_floor: Vec<String>,
    /// 引擎待命的具体原因（空串 = 无）：极限省电、下发失败、外部调度器
    /// 接管等。供 WebUI 与日志解释“为什么引擎没在调度”。
    pub blocked: &'static str,
    /// 用户为目标应用设置的调度目标 FPS（0 = 未设置）。
    pub target_fps: u32,
    /// 第二阶段·AI 临时策略的 FAS ceiling 上限（0 = 无）。只能**压低**档位
    /// ceiling（min 语义），AI 永远不能抬高配置上限——仲裁优先级见
    /// docs/arbitration-v2.13.md。
    pub ai_ceiling: u32,
}

pub struct EngineCtx {
    pub fas: Arc<RwLock<FasConfig>>,
    pub shared: Arc<Mutex<FasShared>>,
    pub live: Arc<CpuctlLive>,
    pub status: Arc<Mutex<EngineStatus>>,
    pub stop: Arc<AtomicBool>,
    pub snapshot: Snapshot,
    pub logger: Logger,
    pub policies: [i32; 4],
}

/// 匹配前台包名的一个渲染图层的累计帧计数。
#[derive(Clone, Debug, PartialEq)]
pub struct LayerFrames {
    /// 去掉尾部 `#N` 序号的图层标识：SurfaceFlinger 图层重建只改变序号，
    /// base 不变才能跨采样把前后两次 dump 对上。
    pub base: String,
    pub total: u64,
    pub dropped: u64,
}

/// 解析 `dumpsys SurfaceFlinger --timestats -dump` 输出，返回前台包名命中的
/// 每个图层的累计帧计数。真实设备（HyperOS / Android 16 实测）的图层块为：
/// `displayRefreshRate = 120 fps` → `uid = …` → `layerName = 包名/Activity#N`
/// → `packageName = `（常为空）→ `totalFrames = N` → `droppedFrames = N`。
/// 归属判定兼容三种来源：`packageName`（新版单数键）、`packageNames`
/// （旧版逗号列表），两者都为空/缺失时从 `layerName` 提取
/// （`包名/Activity#N`、`SurfaceView[包名/…]#0`；无包名的系统图层如
/// `InputMethod#N`、`none` 不归属任何应用）。
/// 游戏画面（SurfaceView/BLAST）与同应用的菜单、弹窗、Activity 窗口层
/// 会各自上报帧数，这里不做求和——把图层帧数相加会当成两倍 FPS；
/// 主图层选举在窗口差分后进行（见 `elect_primary`）。
/// 没有任何图层命中时返回空 Vec。
pub fn parse_timestats(text: &str, package: &str) -> Vec<LayerFrames> {
    let mut acc = Accumulator::default();
    let mut layers: Vec<LayerFrames> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("layerName") {
            acc.settle(package, &mut layers);
            let name = line.split_once('=').map(|(_, value)| value).unwrap_or("");
            acc.layer = layer_package(name);
            acc.layer_base = Some(layer_base_key(name));
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "packageName" | "packageNames" => {
                let owner = value.trim();
                if !owner.is_empty() {
                    acc.owner = Some(owner.split(',').map(str::trim).any(|name| name == package));
                }
            }
            "totalFrames" => acc.block_total = value.trim().parse().unwrap_or(0),
            "droppedFrames" => acc.block_dropped = value.trim().parse().unwrap_or(0),
            _ => {}
        }
    }
    acc.settle(package, &mut layers);
    layers
}

#[derive(Default)]
struct Accumulator {
    matched: bool,
    /// None: absent/empty owner; Some(false): explicitly another package.
    owner: Option<bool>,
    /// 当前块 layerName 提取的包名（packageName 为空时的回退线索）。
    layer: Option<String>,
    /// 当前块去掉序号后的图层标识。
    layer_base: Option<String>,
    block_total: u64,
    block_dropped: u64,
}

impl Accumulator {
    fn settle(&mut self, package: &str, layers: &mut Vec<LayerFrames>) {
        // packageName 命中优先；键为空/缺失时才回退到 layerName 的精确匹配，
        // 显式归属其它包的图层不会因图层名相似而误计。
        let owned = self.owner.unwrap_or_else(|| self.layer.as_deref() == Some(package));
        if owned {
            if let Some(base) = &self.layer_base {
                if let Some(existing) = layers.iter_mut().find(|l| &l.base == base) {
                    // 同名图层块（理论上不出现）合并计数，避免覆盖丢帧。
                    existing.total = existing.total.saturating_add(self.block_total);
                    existing.dropped = existing.dropped.saturating_add(self.block_dropped);
                } else {
                    layers.push(LayerFrames {
                        base: base.clone(),
                        total: self.block_total,
                        dropped: self.block_dropped,
                    });
                }
                self.matched = true;
            }
        }
        self.owner = None;
        self.layer = None;
        self.layer_base = None;
        self.block_total = 0;
        self.block_dropped = 0;
    }
}

/// 从 `layerName = …` 的值提取归属包名：`com.pkg/…#N`、
/// `SurfaceView[com.pkg/…]#0`；`InputMethod#N`、`none` 等无包名图层返回
/// None。
fn layer_package(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("none") {
        return None;
    }
    let candidate = match (value.find('['), value.find(']')) {
        (Some(start), Some(end)) if end > start => &value[start + 1..end],
        _ => value,
    };
    let base = candidate.split('/').next()?.trim();
    let base = base.split('#').next()?.trim();
    util::valid_package(base).then(|| base.to_string())
}

/// 从完整图层名去掉尾部 `#N` 序号，得到跨重建稳定的图层标识。
/// `SurfaceView[com.pkg/act](BLAST)#0` 与 `com.pkg/act#1226` 是不同图层，
/// 必须保留可区分的前缀；仅去掉序号即可容忍图层重建。
fn layer_base_key(full_name: &str) -> String {
    let trimmed = full_name.trim();
    match trimmed.rsplit_once('#') {
        Some((head, suffix)) if !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()) => {
            head.to_string()
        }
        _ => trimmed.to_string(),
    }
}

/// 窗口帧差分：`window_valid` 且当前累计不小于上次累计时用差分（clear 失败、
/// 计数连续的场景）；否则（clear 生效计数清零、SF 重启、图层重建、应用切换
/// 后首个窗口）当前累计本身就是本窗口帧数。saturating 处理计数回退。
fn window_delta(prev: u64, cur: u64, window_valid: bool) -> u64 {
    if window_valid && cur >= prev {
        cur - prev
    } else {
        cur
    }
}

/// 主渲染图层选举：游戏 SurfaceView 才是 FPS 的真实来源，同应用的菜单、
/// 弹窗、Activity 窗口层帧数少且间歇。规则：
/// - 候选 = 窗口帧数 > 0 的图层中帧数最多者；
/// - 已有主图层仍在本窗口渲染、且候选没有远超它（< 2 倍）时保持不变，
///   避免菜单动画短暂反超导致 FPS 读数跳变；
/// - 主图层消失（重建改名、被回收）或被明显更强的新图层取代时切换；
/// - 没有任何图层出帧时维持主图层身份（若仍在列表中），上层按“未渲染”
///   处理而不是当信号丢失。
fn elect_primary<'a>(windows: &'a [(String, u64)], primary: Option<&str>) -> Option<&'a String> {
    let candidate = windows
        .iter()
        .filter(|(_, frames)| *frames > 0)
        .max_by_key(|(_, frames)| *frames)
        .map(|(base, _)| base);
    let Some(candidate) = candidate else {
        return primary.and_then(|current| {
            windows.iter().find(|(base, _)| base == current).map(|(base, _)| base)
        });
    };
    if let Some(current) = primary {
        if current == candidate.as_str() {
            return Some(candidate);
        }
        let current_frames = windows
            .iter()
            .find(|(base, _)| base == current)
            .map(|(_, frames)| *frames)
            .unwrap_or(0);
        let candidate_frames = windows
            .iter()
            .find(|(base, _)| *base == *candidate)
            .map(|(_, frames)| *frames)
            .unwrap_or(0);
        if current_frames > 0 && candidate_frames < current_frames * 2 {
            return windows.iter().find(|(base, _)| base == current).map(|(base, _)| base);
        }
    }
    Some(candidate)
}

/// 帧驱动/负载驱动共用控制器。boost 0..=ceiling：
/// 掉帧或帧率跌破稳定基线 → 快速加压；连续稳定 → 缓慢回落。
/// 稳定基线只在 boost==0 时重学，避免“加压→回落→再加压”的极限环。
#[derive(Debug, Default)]
pub struct Controller {
    pub boost: u32,
    stable_fps: f32,
    bad_streak: u32,
    calm: u32,
    /// 连续低于基线的采样数。持续不足时在两个边界上重学基线：
    /// boost==0（未加压的实际帧率就是真实目标）或 boost 已到 ceiling
    /// （继续加压也没有空间，说明目标本来就低或性能不足，按更大权重
    /// 接受实际帧率并让加压自然回落）。中间加压区间不重学——那正是
    /// “目标更高、继续加压”的正常路径。
    slowed_streak: u32,
    /// 第二阶段·瓶颈识别：连续"帧率不足 + GPU 高/低负载"的采样计数与
    /// 结论（"" = 证据不足不下结论 / "cpu" / "gpu"）。供状态与日志解释
    /// “为什么加了压帧率还上不去”，并为 GPU 瓶颈收窄 CPU 加压幅度。
    gpu_bound_streak: u32,
    cpu_bound_streak: u32,
    pub bottleneck: &'static str,
}

const BOOST_STEP: u32 = 12;
const DECAY_STEP: u32 = 4;
const BAD_STREAK_TRIGGER: u32 = 2;
const CALM_DECAY_TICKS: u32 = 5;
/// 连续低于基线达到该采样数（默认采样周期下约 10 秒）才判定“目标本来就
/// 更低”，开始把基线缓慢拉回实际帧率；短暂抖动不会触发，加压行为不变。
const SUSTAINED_SLOW_TICKS: u32 = 10;
/// 基线下调的每采样权重：从 120 收敛到 60 需要约 30 个采样，期间瞬态
/// 掉帧仍正常加压。
const REBASE_WEIGHT: f32 = 0.1;
/// 已到上限仍持续不足时的快速收敛权重：性能不足的判定不能拖太久，
/// 否则加压一直挂在 ceiling 上白白耗电。
const CEILING_REBASE_WEIGHT: f32 = 0.25;
/// 应用停止渲染（fps<1）时的泄压步长：按 2 倍常规回落，暂停/结算画面
/// 不应把加压挂住半分钟。
const IDLE_DECAY_STEP: u32 = 8;
/// 第二阶段·CPU/GPU 协同瓶颈识别：连续 N 个"帧率不足"采样里 GPU 忙碌
/// ≥85% 判定 GPU 瓶颈——CPU 加压对帧时间收益有限，加压上限收窄到
/// LOAD_CAP_RATIO，不为 GPU 限制空耗电；GPU ≤60% 时判定 CPU 瓶颈，
/// 维持全幅度加压并在状态里如实标注。中间区间不轻率下结论。
const GPU_BOUND_STREAK: u32 = 3;
const GPU_BOUND_BUSY: f32 = 85.0;
const CPU_BOUND_BUSY: f32 = 60.0;

impl Controller {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// 帧信号可用时每次采样调用。target > 0 时为用户设置的调度目标：
    /// 基线固定为该目标（禁用重学，用户目标不被平均帧率悄悄改写）；
    /// 达到目标±容差视为满足，低于目标 88% 才加压——应用本身被限制在
    /// 目标帧率时不会为追逐更高刷新率持续加压。
    pub fn on_frame(&mut self, ceiling: u32, sample: FrameSample) -> u32 {
        self.on_frame_with(ceiling, sample, 0)
    }

    pub fn on_frame_with(&mut self, ceiling: u32, sample: FrameSample, target: u32) -> u32 {
        self.on_frame_evaluated(ceiling, sample, target, None)
    }

    /// 同 on_frame_with，附带本采样窗口的 GPU 忙碌百分比（可能为 None），
    /// 用于瓶颈识别与加压上限收窄。
    pub fn on_frame_evaluated(
        &mut self,
        ceiling: u32,
        sample: FrameSample,
        target: u32,
        gpu_busy: Option<f32>,
    ) -> u32 {
        let ceiling = ceiling.min(100);
        if !sample.fps.is_finite()
            || !sample.dropped.is_finite()
            || !(0.0..=1.0).contains(&sample.dropped)
        {
            self.bad_streak = 0;
            self.calm = 0;
            return self.boost;
        }
        if sample.fps < 1.0 {
            // 应用停止渲染（暂停/静态画面）时按稳定路径回落，步长加倍：
            // 暂停几分钟不应一直保持游戏加压。
            self.calm += 1;
            if self.calm >= CALM_DECAY_TICKS {
                self.boost = self.boost.saturating_sub(IDLE_DECAY_STEP);
                self.calm = 0;
            }
            self.bottleneck = "";
            self.gpu_bound_streak = 0;
            self.cpu_bound_streak = 0;
            return self.boost;
        }
        let dropping = sample.dropped >= 0.02;
        if target > 0 {
            // 目标模式：基线=用户目标，禁用基线学习与上限重学。
            self.stable_fps = target as f32;
            self.slowed_streak = 0;
            let slowed = sample.fps < self.stable_fps * 0.88;
            self.track_bottleneck(slowed, gpu_busy);
            if dropping || slowed {
                self.bad_streak += 1;
                self.calm = 0;
                if dropping || self.bad_streak >= BAD_STREAK_TRIGGER {
                    self.boost = (self.boost + BOOST_STEP).min(self.pressure_ceiling(ceiling));
                    self.bad_streak = 0;
                }
            } else {
                // 达到目标（±容差）即为满足：按常规节奏回落加压。
                self.bad_streak = 0;
                self.calm += 1;
                if self.calm >= CALM_DECAY_TICKS {
                    self.boost = self.boost.saturating_sub(DECAY_STEP);
                    self.calm = 0;
                }
            }
            return self.boost;
        }
        if self.stable_fps < 1.0 {
            self.stable_fps = sample.fps;
        }
        let slowed = sample.fps < self.stable_fps * 0.88;
        if dropping || slowed {
            self.bad_streak += 1;
            self.calm = 0;
            if slowed {
                self.slowed_streak += 1;
                // 基线重学只在两个无歧义的边界进行（见结构体注释）：
                // 未加压时的帧率即真实目标；已到上限仍持续不足时接受
                // 实际帧率（快速收敛），让回落路径重新可用。
                if self.slowed_streak >= SUSTAINED_SLOW_TICKS {
                    if self.boost == 0 {
                        self.stable_fps =
                            self.stable_fps * (1.0 - REBASE_WEIGHT) + sample.fps * REBASE_WEIGHT;
                    } else if self.boost >= ceiling {
                        self.stable_fps = self.stable_fps * (1.0 - CEILING_REBASE_WEIGHT)
                            + sample.fps * CEILING_REBASE_WEIGHT;
                    }
                }
            } else {
                self.slowed_streak = 0;
            }
            self.track_bottleneck(slowed, gpu_busy);
            if dropping || self.bad_streak >= BAD_STREAK_TRIGGER {
                self.boost = (self.boost + BOOST_STEP).min(self.pressure_ceiling(ceiling));
                self.bad_streak = 0;
            }
        } else {
            self.bad_streak = 0;
            self.slowed_streak = 0;
            self.track_bottleneck(false, gpu_busy);
            self.calm += 1;
            if self.boost == 0 {
                // 未加压时的帧率就是真实基线，向新值缓慢收敛。
                self.stable_fps = self.stable_fps * 0.8 + sample.fps * 0.2;
            } else if self.calm >= CALM_DECAY_TICKS {
                self.boost = self.boost.saturating_sub(DECAY_STEP);
                self.calm = 0;
            }
        }
        self.boost
    }

    /// 瓶颈计数：只在"帧率不足"的采样上累计，健康/无数据清零。三连才
    /// 下结论，避免瞬时 GPU 尖峰把加压幅度永久压低。
    fn track_bottleneck(&mut self, slowed: bool, gpu_busy: Option<f32>) {
        if !slowed {
            self.gpu_bound_streak = 0;
            self.cpu_bound_streak = 0;
            self.bottleneck = "";
            return;
        }
        match gpu_busy {
            Some(busy) if busy >= GPU_BOUND_BUSY => {
                self.gpu_bound_streak += 1;
                self.cpu_bound_streak = 0;
            }
            Some(busy) if busy <= CPU_BOUND_BUSY => {
                self.cpu_bound_streak += 1;
                self.gpu_bound_streak = 0;
            }
            _ => {
                self.gpu_bound_streak = 0;
                self.cpu_bound_streak = 0;
            }
        }
        if self.gpu_bound_streak >= GPU_BOUND_STREAK {
            self.bottleneck = "gpu";
        } else if self.cpu_bound_streak >= GPU_BOUND_STREAK {
            self.bottleneck = "cpu";
        } else {
            self.bottleneck = "";
        }
    }

    /// 本拍允许的加压上限：GPU 瓶颈证据充分时收窄到启发式上限比例
    /// （CPU 提频对 GPU 受限的帧时间无收益），其余维持档位 ceiling。
    fn pressure_ceiling(&self, ceiling: u32) -> u32 {
        if self.bottleneck == "gpu" {
            (ceiling * LOAD_CAP_RATIO / 100).max(1).min(ceiling)
        } else {
            ceiling
        }
    }

    /// 帧信号不可用时的 GPU 负载启发式（busy 百分比 0..100）。
    /// 没有帧时间证据时加压必须有上限：GPU 忙不代表 CPU 需要 uclamp，
    /// 负载高只把 boost 推到启发式上限（档位 ceiling 的一小半），
    /// 不允许单凭负载推到档位顶格。中间负载区间（45..70）不再无限期
    /// 保留 boost——按低速档节奏继续回落，配合迟滞防止振荡。
    pub fn on_load(&mut self, ceiling: u32, gpu_busy: f32) -> u32 {
        let ceiling = ceiling.min(100);
        if !gpu_busy.is_finite() || !(0.0..=100.0).contains(&gpu_busy) {
            return self.boost;
        }
        let cap = ceiling * LOAD_CAP_RATIO / 100;
        if gpu_busy >= 70.0 {
            self.bad_streak += 1;
            self.calm = 0;
            if self.bad_streak >= BAD_STREAK_TRIGGER {
                self.boost = (self.boost + BOOST_STEP).min(cap);
                self.bad_streak = 0;
            }
        } else {
            // 低负载（<=45）与中间负载（45..70）都按低速档节奏回落，
            // 区别只是中间档代表“负载还在”，衰减节奏一致以简化迟滞；
            // 这样负载在阈值附近抖动时 boost 不会被长期挂高。
            self.bad_streak = 0;
            self.calm += 1;
            if self.calm >= CALM_DECAY_TICKS * 2 {
                self.boost = self.boost.saturating_sub(DECAY_STEP);
                self.calm = 0;
            }
        }
        self.boost
    }
}

/// 无帧时间证据时（GPU 负载启发式）允许的加压占档位 ceiling 的比例。
pub const LOAD_CAP_RATIO: u32 = 60;

/// GPU 忙碌百分比：Adreno kgsl 百分比节点优先，其次 kgsl `gpubusy`
/// （"<busy_us> <total_us>" 微秒对，OPlus/小米等 ColorOS 系内核只提供它）、
/// MTK GED 与 Mali kbase。单个节点缺失或格式不识时继续尝试下一个来源，
/// 绝不因一个节点缺席放弃全部启发式输入。
pub fn read_gpu_busy() -> Option<f32> {
    for path in [
        "/sys/class/kgsl/kgsl-3d0/gpu_busy_percentage",
        "/sys/kernel/ged/hw/gpu_meter",
        "/sys/kernel/gpu/gpu_load",
        "/sys/class/misc/mali0/device/gpu_utilisation",
    ] {
        let Ok(raw) = util::read_trimmed(std::path::Path::new(path)) else {
            continue;
        };
        // 兼容 "42"、"42 %"、"GPU loading: 42 %" 等写法：取第一个数字 token。
        let parsed = raw
            .split(|c: char| !(c.is_ascii_digit() || c == '.'))
            .filter(|token| !token.is_empty())
            .find_map(|token| token.parse::<f32>().ok());
        if let Some(value) = parsed {
            if (0.0..=100.0).contains(&value) {
                return Some(value);
            }
        }
    }
    // ColorOS / 一加 / OPPO 的 kgsl 内核只暴露 gpubusy 微秒对。
    if let Ok(raw) = util::read_trimmed(std::path::Path::new(
        "/sys/class/kgsl/kgsl-3d0/gpubusy",
    )) {
        let mut fields = raw
            .split_whitespace()
            .filter_map(|token| token.parse::<f64>().ok());
        if let (Some(busy), Some(total)) = (fields.next(), fields.next()) {
            if total > 0.0 {
                let value = (busy * 100.0 / total) as f32;
                if (0.0..=100.0).contains(&value) {
                    return Some(value);
                }
            }
        }
    }
    None
}

/// SurfaceFlinger timestats 会话：enable → 周期 dump+clear → 停止时 disable。
/// 窗口帧数用“restart 感知差分”：clear 生效时下次 dump 的累计从 0 重来
/// （当前值小于上次值），SF 重启、图层重建同理——这些场景当前累计就是本
/// 窗口帧数；clear 不生效时累计连续，用差分。两种内核行为下窗口都正确，
/// 不再依赖“clear 一定生效”的假设。
/// FPS 只取主渲染图层（`elect_primary`），不把同应用多图层帧数求和。
/// 所有 dumpsys 调用都带超时：SurfaceFlinger 繁忙时 dump 可能长时间不返回，
/// 无超时的阻塞调用会让引擎线程连同 WebUI 状态一起冻结。
struct Timestats {
    enabled: bool,
    /// 上一次 dump 各图层的累计计数，用于窗口差分。
    last_layers: Vec<LayerFrames>,
    /// 当前主渲染图层 base；None 表示尚未选举。
    primary: Option<String>,
    /// 窗口差分是否可信（首窗、应用切换、图层全换后为 false）。
    window_valid: bool,
    last_at: Instant,
    /// enable 失败后的退避重试时刻；避免每个 tick 都 fork 一次注定失败的
    /// dumpsys。
    retry_at: Instant,
    /// 首次失败原因只记一次，便于诊断“为什么一直在用启发式”。
    logged_failure: bool,
}

const DUMPSYS_TIMEOUT: Duration = Duration::from_secs(2);
const ENABLE_BACKOFF: Duration = Duration::from_secs(30);
/// 连续这么多次采样失败后退回 GPU 启发式；期间保留最后一帧数据。
const SAMPLE_FAIL_STREAK: u32 = 3;
/// 引擎激活但帧信号与负载信号都不可用的宽限拍数（约 5 秒）：超过后回到
/// 待命并恢复 fas-rs，避免“占用接管却不产出”。
const NO_SIGNAL_RELEASE_TICKS: u32 = 25;
/// 加压 > 0 而帧/负载信号停更超过此时长：按 2 倍速率受控泄压，加压值
/// 绝不悬挂在高位。
const STALE_BOOST_AFTER: Duration = Duration::from_secs(8);
/// 信号源确认失效后的低频探测周期：降级待命期间按此节奏尝试恢复采样，
/// 不再按采样周期持续 fork dumpsys——省电调度不能自己制造负载。
const SIGNAL_DEAD_REPROBE: Duration = Duration::from_secs(10);
/// fas-rs 进程扫描的缓存周期：override_scene 关闭时用它判断是否需要让位。
const FOREIGN_FAS_SCAN: Duration = Duration::from_secs(1);
/// boost 不变时按去重跳过节点写入；每 25 拍（默认步长下约 5 秒）强制
/// 校验一轮，恢复被调度器 force apply 覆盖的底频与 uclamp。
const REASSERT_TICKS: u32 = 25;

impl Timestats {
    fn new() -> Self {
        Self {
            enabled: false,
            last_layers: Vec::new(),
            primary: None,
            window_valid: false,
            last_at: Instant::now(),
            retry_at: Instant::now(),
            logged_failure: false,
        }
    }

    fn dumpsys(args: &[&str]) -> Result<String, String> {
        util::query_command_limited("/system/bin/dumpsys", args, DUMPSYS_TIMEOUT, 1_048_576)
    }

    fn ensure_enabled(&mut self, logger: &Logger) {
        if self.enabled || Instant::now() < self.retry_at {
            return;
        }
        self.retry_at = Instant::now() + ENABLE_BACKOFF;
        match Self::dumpsys(&["SurfaceFlinger", "--timestats", "-enable", "-maxlayers", "128"]) {
            Ok(_) => {
                self.enabled = true;
                if !self.logged_failure {
                    logger.debug("NovaFAS 帧信号源：SurfaceFlinger timestats");
                }
                self.last_layers.clear();
                self.primary = None;
                self.window_valid = false;
                self.last_at = Instant::now();
            }
            Err(error) => {
                if !self.logged_failure {
                    self.logged_failure = true;
                    logger.info(format!(
                        "NovaFAS 帧信号源不可用（SurfaceFlinger timestats 启用失败: {error}），退回 GPU 负载启发式"
                    ));
                } else {
                    logger.debug(format!("timestats 重新启用仍失败: {error}"));
                }
            }
        }
    }

    /// SurfaceFlinger 可能重启导致会话丢失；连续采样失败时立即重试启用。
    fn force_retry(&mut self) {
        self.enabled = false;
        self.retry_at = Instant::now();
    }

    /// 应用切换：旧应用的窗口计数、主图层归属对新区块毫无意义，
    /// 立即作废；下一窗以新应用的累计为窗口帧数重新开始。
    fn invalidate_window(&mut self) {
        self.last_layers.clear();
        self.primary = None;
        self.window_valid = false;
    }

    /// 返回自上次采样以来的帧样本；窗口异常或输出不可解析返回 None。
    /// 图层都在但本窗口零出帧时返回 fps=0 的样本——这是“前台应用存在
    /// 但未渲染”（暂停/静态画面），不是信号丢失。
    fn sample(&mut self, package: &str) -> Option<FrameSample> {
        if !self.enabled {
            return None;
        }
        let output = Self::dumpsys(&["SurfaceFlinger", "--timestats", "-dump", "-maxlayers", "128"])
            .ok()?;
        let _ = Self::dumpsys(&["SurfaceFlinger", "--timestats", "-clear"]);
        let layers = parse_timestats(&output, package);
        let elapsed = self.last_at.elapsed().as_secs_f32().max(0.1);
        self.last_at = Instant::now();
        if layers.is_empty() {
            self.invalidate_window();
            return None;
        }
        // 逐图层窗口帧数：restart 感知差分（见结构体注释）。
        let mut windows: Vec<(String, u64)> = Vec::with_capacity(layers.len());
        let mut dropped_by_base: Vec<(String, u64)> = Vec::with_capacity(layers.len());
        for layer in &layers {
            let prev = self
                .last_layers
                .iter()
                .find(|last| last.base == layer.base);
            let (prev_total, prev_dropped) = match prev {
                Some(prev) => (prev.total, prev.dropped),
                None => (0, 0),
            };
            let frames = window_delta(prev_total, layer.total, self.window_valid);
            let dropped_now = window_delta(prev_dropped, layer.dropped, self.window_valid);
            windows.push((layer.base.clone(), frames));
            dropped_by_base.push((layer.base.clone(), dropped_now));
        }
        self.last_layers = layers;
        self.window_valid = true;
        let chosen = elect_primary(&windows, self.primary.as_deref())?;
        self.primary = Some(chosen.clone());
        let frames = windows
            .iter()
            .find(|(base, _)| base == chosen)
            .map(|(_, frames)| *frames)
            .unwrap_or(0);
        let dropped_now = dropped_by_base
            .iter()
            .find(|(base, _)| base == chosen)
            .map(|(_, dropped)| *dropped)
            .unwrap_or(0);
        Some(FrameSample {
            fps: frames as f32 / elapsed,
            // 用窗口内的丢帧占比，而不是自启动以来的累计值：长时间运行的
            // 游戏累计丢帧率会钝化，刚发生的掉帧反应不上来。
            dropped: if frames > 0 {
                dropped_now.min(frames) as f32 / frames as f32
            } else {
                0.0
            },
        })
    }

    fn disable(&mut self) {
        if self.enabled {
            let _ = Self::dumpsys(&["SurfaceFlinger", "--timestats", "-disable"]);
            self.enabled = false;
        }
    }
}

/// 暂停/恢复 fas-rs 进程集合。引擎激活时 SIGSTOP，释放时 SIGCONT。
/// 信号一律通过 pidfd 发送：进程在扫描与停止之间死亡、PID 被复用时，
/// 信号落到旧 pidfd 上只影响已不存在的目标，绝不会误停无辜进程
/// （内核 <5.3 无 pidfd 时退回 kill，但发送前重验 /proc 身份）。
struct SceneOverride {
    suspended: Vec<SuspendedFas>,
    enabled: bool,
    /// 下一次允许扫描 /proc 的时刻；接管目标变化是低频事件，每秒一次
    /// 足够，避免每拍（step_ms=200ms）全量扫描进程表。
    scan_at: Instant,
}

struct SuspendedFas {
    pid: i32,
    pidfd: Option<std::fs::File>,
}

fn pidfd_open(pid: i32) -> Option<std::fs::File> {
    let fd = unsafe { crate::ffi::syscall(434, pid, 0u32) };
    if fd < 0 {
        return None;
    }
    // SAFETY: fd 由 pidfd_open 返回，独占所有权。
    Some(unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd as i32) })
}

fn pidfd_signal(pidfd: &std::fs::File, signal: i32) -> bool {
    // SAFETY: FFI 调用，参数为合法 fd 与信号号。
    let result = unsafe {
        crate::ffi::syscall(424, pidfd.as_raw_fd(), signal, std::ptr::null::<u8>(), 0u32)
    };
    result == 0
}

/// pidfd 存活探测：进程退出后内核把 pidfd 置为可读挂断（POLLHUP）。
/// 查询失败按存活处理——保守不删条目，宁可多留一拍也不误删正在运行的
/// fas-rs 的接管记录（审计 P2-1）。
fn pidfd_alive(pidfd: &std::fs::File) -> bool {
    let mut fds = [crate::ffi::PollFd {
        fd: pidfd.as_raw_fd(),
        events: 0,
        revents: 0,
    }];
    // SAFETY: FFI 调用，fds 为合法指针且长度为 1。
    let result = unsafe { crate::ffi::poll(fds.as_mut_ptr(), 1, 0) };
    if result < 0 {
        return true;
    }
    fds[0].revents & (crate::ffi::POLLHUP | crate::ffi::POLLERR | crate::ffi::POLLNVAL) == 0
}

/// 打开 pidfd 并确认 /proc 身份仍是 fas-rs（与扫描同一判定标准，减少
/// 无谓的 fd 打开）；PID 复用防护由 pidfd 本身保证。内核不支持 pidfd 时
/// 退回裸 PID，并在发送前后用进程身份校验缩小复用窗口。
fn open_verified_fas(pid: i32) -> Option<SuspendedFas> {
    if !crate::fas::looks_like_fas_pid(pid) {
        return None;
    }
    match pidfd_open(pid) {
        Some(pidfd) => Some(SuspendedFas {
            pid,
            pidfd: Some(pidfd),
        }),
        None => {
            if crate::fas::looks_like_fas_pid(pid) {
                Some(SuspendedFas {
                    pid,
                    pidfd: None,
                })
            } else {
                None
            }
        }
    }
}

impl SuspendedFas {
    fn signal(&self, signal: i32) -> bool {
        match &self.pidfd {
            Some(pidfd) => pidfd_signal(pidfd, signal),
            None => {
                let result = unsafe { crate::ffi::kill(self.pid, signal) };
                result == 0
            }
        }
    }
}

impl Drop for SuspendedFas {
    fn drop(&mut self) {
        // 无论从哪条路径移除（正常恢复、panic 展开），都尝试解冻；
        // 进程已退出时信号失败是预期的，静默即可。
        self.signal(SIGCONT);
    }
}

impl SceneOverride {
    fn refresh(&mut self, wanted: bool, logger: &Logger) {
        if !self.enabled {
            // 运行中关闭接管（override_scene=false）时必须立即恢复被暂停的
            // fas-rs；旧实现提前 return，冻结的进程要等引擎线程退出才解冻。
            if !self.suspended.is_empty() {
                // 先摘下集合再发信号：SuspendedFas 落盘时 Drop 也会发
                // SIGCONT，双发无害，但日志只需要一条。
                let suspended = std::mem::take(&mut self.suspended);
                for entry in &suspended {
                    entry.signal(SIGCONT);
                }
                drop(suspended);
                logger.info("NovaFAS 已恢复 fas-rs 进程运行（帧信号接管已关闭）");
            }
            return;
        }
        if wanted {
            let now = Instant::now();
            if now < self.scan_at {
                return;
            }
            self.scan_at = now + FOREIGN_FAS_SCAN;
            // 审计 P2-1：先复核已暂停条目。pidfd 存活探测处理"旧进程已死、
            // PID 被新的 fas-rs 复用"——此时旧条目必须让位，新进程走下方
            // 正常探测-暂停流程，否则它永远不会被暂停（双调度器窗口）。
            // 无 pidfd 的老内核兜底按 /proc 身份复核；移除条目时 Drop 发出的
            // SIGCONT 落在旧 pidfd 或已退出的目标上，无害。
            self.suspended.retain(|entry| match &entry.pidfd {
                Some(pidfd) => pidfd_alive(pidfd),
                None => crate::fas::looks_like_fas_pid(entry.pid),
            });
            for pid in crate::fas::running_pids() {
                if self.suspended.iter().any(|entry| entry.pid == pid) {
                    continue;
                }
                let Some(entry) = open_verified_fas(pid) else {
                    continue;
                };
                if entry.signal(SIGSTOP) {
                    logger.info(format!(
                        "NovaFAS 已暂停 fas-rs 进程 {pid}，帧感知调度由本引擎接管"
                    ));
                    self.suspended.push(entry);
                }
            }
        } else if !self.suspended.is_empty() {
            let suspended = std::mem::take(&mut self.suspended);
            for entry in &suspended {
                entry.signal(SIGCONT);
            }
            drop(suspended);
            logger.info("NovaFAS 已恢复 fas-rs 进程运行");
        }
    }
}

impl Drop for SceneOverride {
    fn drop(&mut self) {
        // SuspendedFas::drop 已发 SIGCONT；这里兜底遍历一次，保证
        // 任何早退路径都不会把 fas-rs 留在冻结状态。
        for entry in &self.suspended {
            entry.signal(SIGCONT);
        }
    }
}

/// 每个策略簇的写入状态：最低频率去重与档位 floor 缓存。
/// 审计 CF-1：小核折扣权重在引擎启动时按物理频率识别（见
/// little_core_weights），不再依赖配置槽序。
struct ClusterState {
    policy: i32,
    last_min: Option<String>,
    floor: Option<u64>,
    floor_key: String,
    weight: f32,
}

/// 无帧时间证据时的加压上限比例之外，小核簇的频率折扣：抬小核底频
/// 的能效收益最差，×0.6 优先保证大中核有空间（原 v2.12.1 行为）。
const LITTLE_CORE_WEIGHT: f32 = 0.6;

/// 簇物理最高频率：cpuinfo_max_freq 优先（物理上限，不受温控影响），
/// 缺失回退频表最大值；都读不到返回 None，不猜（审计"不捏造频率表"）。
fn physical_cluster_max(base: Option<&std::path::Path>) -> Option<u64> {
    let base = base?;
    if let Some(value) = util::read_trimmed(base.join("cpuinfo_max_freq"))
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|value| *value > 0)
    {
        return Some(value);
    }
    util::read_trimmed(base.join("scaling_available_frequencies"))
        .ok()?
        .split_whitespace()
        .filter_map(|token| token.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .max()
}

/// 审计 CF-1：小核折扣（×0.6）是节能意图，落错簇会反转成"压小核放大核"
/// ——adapt_config 保留用户自定义 policy 槽序，"index==0 即小核"没有结构
/// 保证。改为按物理最高频率识别：已知簇 ≥2 且唯一最低者视为小核；并列、
/// 缺失识别不了时全部 1.0，不猜。
fn little_core_weights(max_khz: &[Option<u64>]) -> Vec<f32> {
    let mut weights = vec![1.0f32; max_khz.len()];
    let known: Vec<(usize, u64)> = max_khz
        .iter()
        .enumerate()
        .filter_map(|(index, value)| value.map(|value| (index, value)))
        .collect();
    if known.len() >= 2 {
        let min = known.iter().map(|(_, value)| *value).min().unwrap_or(0);
        let lowest: Vec<usize> = known
            .iter()
            .filter(|(_, value)| *value == min)
            .map(|(index, _)| *index)
            .collect();
        if lowest.len() == 1 {
            weights[lowest[0]] = LITTLE_CORE_WEIGHT;
        }
    }
    weights
}

pub fn spawn(ctx: EngineCtx) -> Option<std::thread::JoinHandle<()>> {
    if !ctx.policies.iter().any(|policy| *policy >= 0) {
        return None;
    }
    std::thread::Builder::new()
        .name("novasched-fas".into())
        .spawn(move || {
            // 引擎线程 panic 不能把 scaling_min_freq / uclamp 加压留在节点
            // 上：捕获展开后按快照原值释放全部控制，再记录失败。release
            // 构建配置为 panic=abort 时本捕获不生效，进程退出由监督进程的
            // 崩溃恢复路径兜底。
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&ctx)));
            if outcome.is_err() {
                ctx.logger.error("NovaFAS 引擎线程异常 panic，已按快照恢复全部节点控制");
                emergency_release(&ctx);
            }
        })
        .ok()
}

/// panic / 异常退出路径的兜底释放：不依赖任何运行中缓存，直接把
/// 每个簇的 scaling_min_freq 与 top-app uclamp.min 恢复为快照原值，
/// 并解冻全部被暂停的 fas-rs、停用 timestats 会话。
fn emergency_release(ctx: &EngineCtx) {
    {
        let scene = SceneOverride {
            suspended: Vec::new(),
            enabled: false,
            scan_at: Instant::now(),
        };
        drop(scene);
    }
    for policy in ctx.policies.iter().copied().filter(|policy| *policy >= 0) {
        let Some(base) = policy_base(policy) else { continue };
        let node = base.join("scaling_min_freq");
        if let Ok(original) = ctx.snapshot.original_value(&node) {
            let value = original
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| "0".into());
            let _ = ctx.snapshot.write(&node, value.trim());
        }
    }
    let node = crate::scheduler::cpuctl_node("top-app", "min");
    if node.exists() {
        if let Ok(original) = ctx.snapshot.original_value(&node) {
            let value = original
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| "0".into());
            let _ = ctx.snapshot.write(&node, value.trim());
        }
    }
    let _ = util::query_command_limited(
        "/system/bin/dumpsys",
        &["SurfaceFlinger", "--timestats", "-disable"],
        DUMPSYS_TIMEOUT,
        4096,
    );
}

fn run(ctx: &EngineCtx) {
    let mut controller = Controller::default();
    let mut stats = Timestats::new();
    let mut scene = SceneOverride {
        suspended: Vec::new(),
        enabled: ctx.fas.read().map(|cfg| cfg.override_scene).unwrap_or(true),
        scan_at: Instant::now(),
    };
    let mut clusters: Vec<ClusterState> = ctx
        .policies
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, policy)| *policy >= 0)
        .map(|(index, policy)| ClusterState {
            policy,
            last_min: None,
            floor: None,
            floor_key: format!("\0cluster{index}"),
            weight: 1.0,
        })
        .collect();
    // 审计 CF-1：物理频率识别小核簇（读不到时不猜，全部 1.0）。
    let physical_max: Vec<Option<u64>> = clusters
        .iter()
        .map(|cluster| physical_cluster_max(policy_base(cluster.policy).as_deref()))
        .collect();
    let weights = little_core_weights(&physical_max);
    for (index, cluster) in clusters.iter_mut().enumerate() {
        cluster.weight = weights[index];
    }
    let mut status = EngineStatus::default();
    let mut last_boost: u32 = 0;
    let mut last_mode = String::new();
    let mut last_package = String::new();
    let mut last_progress = Instant::now();
    let mut last_sample = Instant::now() - Duration::from_secs(60);
    let mut frame_ok = false;
    let mut fail_streak: u32 = 0;
    let mut no_signal: u32 = 0;
    let mut signal_dead = false;
    // 定期重断言计数：调度器 force apply（配置重载/审计修复/档位重发）会把
    // 底频写回档位基线，而 boost 未变时本引擎按去重逻辑不重写。每
    // REASSERT_TICKS 拍强制校验一轮节点，堵住这个被覆盖后不自愈的窗口。
    let mut reassert: u32 = 0;
    let mut foreign_scan_at = Instant::now() - FOREIGN_FAS_SCAN;
    let mut foreign_fas = false;
    loop {
        // 每拍递增：WebUI 看到它变化即证明引擎线程活着，即使帧率与
        // 加压值都稳定不动。
        status.ticks = status.ticks.wrapping_add(1);
        if ctx.stop.load(Ordering::Relaxed) {
            break;
        }
        let cfg = ctx
            .fas
            .read()
            .map(|cfg| cfg.clone())
            .unwrap_or_else(|_| FasConfig::default());
        scene.enabled = cfg.override_scene;
        let shared = ctx
            .shared
            .lock()
            .map(|state| state.clone())
            .unwrap_or_default();
        if shared.mode != last_mode {
            // 档位切换：丢弃旧 floor 缓存（调度器已写入新档位的最低频率），
            // 控制器归零并在下一拍强制重写目标。
            last_mode = shared.mode.clone();
            for (index, cluster) in clusters.iter_mut().enumerate() {
                cluster.floor = None;
                cluster.floor_key = format!("\0cluster{index}");
                cluster.last_min = None;
            }
            controller.reset();
            last_boost = u32::MAX;
        }
        if shared.package != last_package {
            // 应用切换：旧应用的加压对新区块毫无意义，不清零会把上一
            // 个游戏的 boost 原样带到新应用（炸功耗的隐性路径）。
            // 窗口计数与主图层归属一并作废：旧应用的帧累计差分到新应用
            // 只会产生伪造的 0 帧或虚高帧；frame_ok 同步复位，下一窗有效
            // 样本到来前不启用 timestats 路径。
            last_package = shared.package.clone();
            if !last_package.is_empty() {
                controller.reset();
                last_boost = u32::MAX;
                stats.invalidate_window();
                status.fps = 0.0;
                frame_ok = false;
                fail_streak = 0;
            }
        }
        let mode_cfg = cfg.modes.get(&shared.mode).cloned();
        // 接管开关关闭而 Scene fas-rs 仍在运行：它才是帧感知调度器，本引擎
        // 必须让位，避免两个调度器争抢同一批 uclamp/最低频率节点。
        if !cfg.override_scene && Instant::now() >= foreign_scan_at {
            foreign_scan_at = Instant::now() + FOREIGN_FAS_SCAN;
            foreign_fas = !crate::fas::running_pids().is_empty();
        }
        let foreign_active = !cfg.override_scene && foreign_fas;
        let want_active = shared.allowed
            && shared.ready
            && !shared.package.is_empty()
            && crate::config::MODES.contains(&shared.mode.as_str())
            && mode_cfg.as_ref().is_some_and(|mode| mode.ceiling > 0)
            && !foreign_active;
        if !cfg.enabled || !want_active {
            // 外部调度器接管（!shared.allowed）时节点已不归本模块管理：
            // 跳过恢复写，避免覆盖对方刚设置的值。
            let reason = if !shared.allowed {
                "外部调度器接管中，NovaFAS 已让位"
            } else if foreign_active {
                "Scene fas-rs 正在运行且未开启接管，NovaFAS 待命避免双调度"
            } else if !shared.blocked.is_empty() {
                shared.blocked
            } else {
                "NovaFAS 当前未执行调度"
            };
            release(
                ctx,
                &mut controller,
                &mut clusters,
                &mut scene,
                &mut status,
                shared.allowed,
                reason,
            );
            publish(&ctx.status, &status);
            std::thread::sleep(Duration::from_millis(cfg.step_ms.max(300)));
            continue;
        }
        let mode_cfg = mode_cfg.unwrap_or(FasMode::default());
        // AI 临时策略：ceiling 只能被压低（min 语义），不能抬高配置。
        // 统一在档位副本上钳制，控制器与 apply_targets 下游全部生效。
        let effective_ceiling = if shared.ai_ceiling > 0 {
            mode_cfg.ceiling.min(shared.ai_ceiling)
        } else {
            mode_cfg.ceiling
        };
        let mode_cfg = FasMode { ceiling: effective_ceiling, ..mode_cfg };
        // 已确认无任何信号源时保持 fas-rs 运行，不再空占接管位。
        scene.refresh(!signal_dead, &ctx.logger);
        if signal_dead && last_sample.elapsed() < SIGNAL_DEAD_REPROBE {
            // 降级待命：低频探测恢复（约 10 秒一次），不按采样周期持续
            // fork dumpsys；恢复信号后正常采样路径会清掉 signal_dead。
            publish(&ctx.status, &status);
            std::thread::sleep(Duration::from_millis(cfg.step_ms.max(500)));
            continue;
        }
        stats.ensure_enabled(&ctx.logger);

        status.gpu = 0.0;
        // 采样时钟：帧信号按 sample_ms 周期；无帧信号时每 tick 用 GPU 负载。
        if last_sample.elapsed().as_millis() as u64 >= cfg.sample_ms {
            last_sample = Instant::now();
            match stats.sample(&shared.package) {
                Some(sample) => {
                    // fps 0 是合法样本（前台存在、图层都在但本窗口零出帧）：
                    // 交给控制器按“停止渲染”路径回落，而不是当成信号失败
                    // 去反复 fork dumpsys 重试。
                    if sample.fps.is_finite()
                        && (0.0..=240.0).contains(&sample.fps)
                        && sample.dropped.is_finite()
                        && (0.0..=1.0).contains(&sample.dropped)
                    {
                        frame_ok = true;
                        fail_streak = 0;
                        last_progress = Instant::now();
                        // 第二阶段·CPU/GPU 协同：帧路径同时读一次 GPU 忙碌
                        // 节点做瓶颈识别（单文件读，代价与启发式同量级）。
                        let gpu_sample = read_gpu_busy();
                        controller.on_frame_evaluated(
                            effective_ceiling,
                            sample,
                            shared.target_fps,
                            gpu_sample,
                        );
                        status.fps = sample.fps;
                        status.target = shared.target_fps;
                        status.gpu = gpu_sample.unwrap_or(0.0);
                    } else {
                        fail_streak += 1;
                        if fail_streak >= SAMPLE_FAIL_STREAK {
                            frame_ok = false;
                            fail_streak = 0;
                            status.fps = 0.0;
                            stats.force_retry();
                        }
                    }
                }
                None => {
                    if frame_ok {
                        fail_streak += 1;
                        if fail_streak >= SAMPLE_FAIL_STREAK {
                            // 连续失败说明会话已失效（SurfaceFlinger 重启或
                            // 输出格式变化）：退回启发式并安排重新启用。
                            frame_ok = false;
                            fail_streak = 0;
                            status.fps = 0.0;
                            stats.force_retry();
                        }
                    }
                }
            }
        }
        let mut gpu_available = false;
        if !frame_ok {
            if let Some(busy) = read_gpu_busy() {
                gpu_available = true;
                status.gpu = busy;
                last_progress = Instant::now();
                controller.on_load(effective_ceiling, busy);
            }
        }
        // 防炸功耗守门：加压 > 0 而帧/负载信号都停更超过 8 秒（应用停止
        // 渲染、采样全部失效），按 2 倍速率受控泄压到 0，绝不让加压值
        // 悬挂在高位。恢复信号后 last_progress 会被刷新，泄压自动停止。
        if controller.boost > 0 && last_progress.elapsed() > STALE_BOOST_AFTER {
            controller.boost = controller.boost.saturating_sub(BOOST_STEP * 2);
            if controller.boost == 0 {
                controller.bad_streak = 0;
                controller.calm = 0;
            }
        }
        status.source = if frame_ok {
            "timestats"
        } else if gpu_available {
            "heuristic"
        } else {
            "none"
        };
        status.reason = match status.source {
            "timestats" => match controller.bottleneck {
                // 第二阶段·闭环反馈：加压了帧率还上不去时，解释瓶颈在哪。
                "gpu" => "帧率不足且 GPU 高负载：瓶颈在 GPU，CPU 加压已限制幅度（继续提频无帧收益）",
                "cpu" => "帧率不足且 GPU 负载不高：瓶颈在 CPU，正在按档位包络加压",
                _ => "",
            },
            "heuristic" => "帧时间信号不可用或已过期，当前使用 GPU 负载启发式",
            _ => "帧时间与 GPU 负载信号均不可用，调度已回退到当前档位",
        };
        if status.source == "none" {
            no_signal += 1;
        } else {
            no_signal = 0;
            signal_dead = false;
        }
        if no_signal >= NO_SIGNAL_RELEASE_TICKS {
            no_signal = 0;
            signal_dead = true;
            release(
                ctx,
                &mut controller,
                &mut clusters,
                &mut scene,
                &mut status,
                shared.allowed,
                "NovaFAS 当前未执行调度",
            );
            status.state = "degraded";
            status.reason = "连续采样无有效帧或负载信号，已释放 NovaFAS 加压并回退到当前档位";
            publish(&ctx.status, &status);
            std::thread::sleep(Duration::from_millis(cfg.step_ms.max(300)));
            continue;
        }
        status.state = if status.source == "none" {
            "degraded"
        } else {
            "active"
        };
        status.boost = controller.boost;
        status.override_active = !scene.suspended.is_empty();
        reassert = reassert.wrapping_add(1);
        let force_reassert = reassert >= REASSERT_TICKS;
        if force_reassert {
            reassert = 0;
        }
        apply_targets(
            ctx,
            &shared,
            &mode_cfg,
            controller.boost,
            &mut clusters,
            &mut last_boost,
            force_reassert,
        );
        publish(&ctx.status, &status);
        std::thread::sleep(Duration::from_millis(cfg.step_ms));
    }
    release(
        ctx,
        &mut controller,
        &mut clusters,
        &mut scene,
        &mut status,
        true,
        "NovaFAS 已停止，节点与 fas-rs 状态已恢复",
    );
    stats.disable();
}

fn release(
    ctx: &EngineCtx,
    controller: &mut Controller,
    clusters: &mut [ClusterState],
    scene: &mut SceneOverride,
    status: &mut EngineStatus,
    restore_nodes: bool,
    idle_reason: &'static str,
) {
    scene.refresh(false, &ctx.logger);
    if restore_nodes && (status.state == "active" || controller.boost > 0) {
        restore_floors(ctx, clusters);
        restore_uclamp_baseline(ctx);
    }
    if status.state == "active" || controller.boost > 0 {
        controller.reset();
    }
    status.state = if ctx.fas.read().map(|cfg| cfg.enabled).unwrap_or(false) {
        "idle"
    } else {
        "off"
    };
    status.source = "none";
    status.reason = idle_reason;
    status.fps = 0.0;
    status.gpu = 0.0;
    status.boost = 0;
    status.override_active = false;
    publish(&ctx.status, status);
}

/// 把状态发布到共享槽位。守护进程按自己的 2 秒节奏取值转发给 WebUI，
/// 这里无需再去抖：之前的门限（boost 仅在 0/24/48… 时发布）会让 12%、
/// 36% 的加压永远到不了界面，看起来就像引擎冻结。
fn publish(sink: &Arc<Mutex<EngineStatus>>, status: &EngineStatus) {
    if let Ok(mut sink) = sink.lock() {
        *sink = status.clone();
    }
}

fn restore_floors(ctx: &EngineCtx, clusters: &mut [ClusterState]) {
    for cluster in clusters.iter_mut() {
        cluster.last_min = None;
        let Some(floor) = cluster.floor else { continue };
        let Some(base) = policy_base(cluster.policy) else { continue };
        let value = floor.to_string();
        if util::read_trimmed(&base.join("scaling_min_freq"))
            .ok()
            .as_deref()
            != Some(value.as_str())
        {
            let _ = ctx.snapshot.write(&base.join("scaling_min_freq"), &value);
        }
    }
}

fn restore_uclamp_baseline(ctx: &EngineCtx) {
    let node = crate::scheduler::cpuctl_node("top-app", "min");
    if !node.exists() {
        return;
    }
    let baseline = {
        let Ok(mut state) = ctx.live.inner.lock() else { return };
        // 引擎释放后不再拥有该节点：清掉自己的加压值，恢复调度器基线；
        // 基线为空（档位/规则都未设置 floor）时回落到快照原值。
        state.fas_min.clear();
        if state.top_app_min.trim().is_empty() {
            ctx.snapshot
                .original_value(&node)
                .ok()
                .flatten()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "0".into())
        } else {
            state.top_app_min.trim().to_string()
        }
    };
    let _ = ctx.snapshot.write(&node, baseline.trim());
}

/// 把控制器 boost 映射为节点写入：top-app uclamp.min 与各簇最低频率。
/// 最低频率上限 = 当前实际最高频率 × freq_scale% × boost/100（首簇按小核
/// 处理，×0.6），永低于档位 floor、不高于当前实际最高频率。
fn apply_targets(
    ctx: &EngineCtx,
    shared: &FasShared,
    mode_cfg: &FasMode,
    boost: u32,
    clusters: &mut [ClusterState],
    last_boost: &mut u32,
    force_reassert: bool,
) {
    if boost == *last_boost && !force_reassert {
        return;
    }
    *last_boost = boost;
    let node = crate::scheduler::cpuctl_node("top-app", "min");
    let uclamp_target = mode_cfg.ceiling.min(100) * boost / 100;
    if node.exists() {
        let mut bounded = uclamp_target.to_string();
        if uclamp_target > 0 {
            if let Some(maximum) = util::read_trimmed(node.with_file_name("cpu.uclamp.max"))
                .ok()
                .and_then(|value| parse_percent(&value))
            {
                if uclamp_target > maximum {
                    bounded = maximum.to_string();
                }
            }
        }
        // 所有权分离：top_app_min 归调度器（档位/应用规则基线），fas_min 归
        // 本引擎。boost==0 时恢复基线而不是写 0；基线高于加压目标时保持
        // 基线（规则地板不允许被引擎降挡）。
        let baseline = ctx
            .live
            .inner
            .lock()
            .map(|state| state.top_app_min.clone())
            .unwrap_or_default();
        let (write_value, next_fas_min) =
            resolve_uclamp_write(baseline.trim(), uclamp_target, &bounded);
        if let Ok(mut state) = ctx.live.inner.lock() {
            state.fas_min = next_fas_min;
        }
        if util::read_trimmed(&node).ok().as_deref() != Some(write_value.as_str()) {
            if let Err(error) = ctx.snapshot.write(&node, &write_value) {
                ctx.logger.debug(format!("NovaFAS uclamp 写入失败: {error}"));
            }
            // 写后读回（审计 P2-3）：目标未落地就作废 last_boost，下一拍
            // 直接重写，对齐 freq 路径“读回不一致下一拍重试”的语义——
            // 否则 boost 不变时去重会让失败目标悬挂到强制校验轮才补。
            if util::read_trimmed(&node).ok().as_deref() != Some(write_value.as_str()) {
                *last_boost = u32::MAX;
                ctx.logger
                    .debug(format!("NovaFAS uclamp 写后读回不一致（目标 {write_value}）"));
            }
        }
    }

    for (index, cluster) in clusters.iter_mut().enumerate() {
        let Some(base) = policy_base(cluster.policy) else {
            continue;
        };
        let max_now = match util::read_trimmed(&base.join("scaling_max_freq"))
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
        {
            Some(value) => value,
            None => continue,
        };
        // floor 有缓存，而 scaling_max_freq 随温控/外部限频动态下降：
        // 缓存的 floor 可能高于最新上限，clamp(floor, max) 会直接 panic。
        // 每拍都以当前实际上限为界；缓存放大上限时作废缓存重新解析。
        if cluster.floor.is_some_and(|cached| cached > max_now) {
            cluster.floor = None;
        }
        let Some(floor_khz) =
            resolve_floor(&mut cluster.floor, &mut cluster.floor_key, index, shared, &base, max_now)
        else {
            continue;
        };
        let floor_khz = floor_khz.min(max_now);
        // 审计 CF-1：权重在引擎启动时按物理频率识别（little_core_weights），
        // 不再假设槽 0 是小核——用户自定义槽序时旧假设会把节能意图反转。
        let weight = cluster.weight;
        let target = (max_now as f32
            * mode_cfg.freq_scale.min(100) as f32
            * boost as f32
            / 10000.0
            * weight) as u64;
        let target = target.max(floor_khz).min(max_now);
        let resolved = normalize_frequency(&base, &target.to_string(), true);
        let resolved = resolved
            .parse::<u64>()
            .unwrap_or(target)
            .min(max_now)
            .to_string();
        // 强制校验轮必须绕过 last_min 去重：调度器 force apply 可能把节点
        // 写回档位基线而 boost 未变，去重会跳过真实读回，漂移无法自愈。
        if !force_reassert && cluster.last_min.as_deref() == Some(resolved.as_str()) {
            continue;
        }
        let node = base.join("scaling_min_freq");
        if util::read_trimmed(&node).ok().as_deref() != Some(resolved.as_str()) {
            if let Err(error) = ctx.snapshot.write(&node, &resolved) {
                ctx.logger.debug(format!(
                    "NovaFAS policy{} 最低频率写入失败: {error}",
                    cluster.policy
                ));
                continue;
            }
            // 写后读回：内核拒绝或被厂商服务立即改写时作废去重缓存，
            // 下一拍重试，而不是在缓存里维持一个从未生效的目标值。
            if util::read_trimmed(&node).ok().as_deref() != Some(resolved.as_str()) {
                cluster.last_min = None;
                ctx.logger.debug(format!(
                    "NovaFAS policy{} 最低频率写后读回不一致（目标 {resolved}）",
                    cluster.policy
                ));
                continue;
            }
        }
        cluster.last_min = Some(resolved);
    }
}

/// 解析并缓存簇最低频率下限。守护进程发布的是内核适配后的 kHz 字符串；
/// 兼容 "0%" / "50%" 等原始写法（按当前实际最高频率换算）。
fn resolve_floor(
    floor: &mut Option<u64>,
    floor_key: &mut String,
    index: usize,
    shared: &FasShared,
    base: &std::path::Path,
    max_now: u64,
) -> Option<u64> {
    let raw = shared.min_floor.get(index)?;
    let raw = raw.trim();
    if floor_key != raw {
        let target = if let Some(percent) = raw.strip_suffix('%') {
            max_now.checked_mul(percent.parse::<u64>().ok()?)? / 100
        } else {
            raw.parse::<u64>().ok()?
        };
        let resolved = normalize_frequency(base, &target.to_string(), true)
            .parse::<u64>()
            .unwrap_or(target)
            .min(max_now);
        *floor_key = raw.to_string();
        *floor = Some(resolved);
    }
    *floor
}

fn policy_base(policy: i32) -> Option<PathBuf> {
    Some(PathBuf::from(format!(
        "/sys/devices/system/cpu/cpufreq/policy{policy}"
    )))
}

/// 计算 top-app uclamp.min 本拍应写入的值与引擎的 fas_min 状态。
/// 返回 (节点写入值, 新 fas_min)：
/// - 加压为 0：恢复调度器基线（无基线写 0），fas_min 清空；
/// - 基线高于加压目标：保持基线，fas_min 清空（引擎此时不拥有节点）；
/// - 其余：写入加压目标，fas_min 记录之（输入加速脉冲到期回到该值）。
fn resolve_uclamp_write(baseline: &str, uclamp_target: u32, bounded: &str) -> (String, String) {
    if uclamp_target == 0 {
        let restore = if baseline.trim().is_empty() {
            "0".to_string()
        } else {
            baseline.trim().to_string()
        };
        return (restore, String::new());
    }
    let baseline_pct = parse_percent(baseline).unwrap_or(0);
    let target_pct = parse_percent(bounded).unwrap_or(uclamp_target);
    if baseline_pct > target_pct {
        (baseline.trim().to_string(), String::new())
    } else {
        (bounded.to_string(), bounded.to_string())
    }
}

/// cgroup 返回的百分比可能是 "30"、"30.00" 或 "max"。
fn parse_percent(raw: &str) -> Option<u32> {
    if raw == "max" {
        return Some(100);
    }
    let (whole, fraction) = raw.split_once('.').unwrap_or((raw, ""));
    let decimal = if fraction.is_empty() {
        0
    } else if fraction.len() <= 2 {
        fraction.parse::<u32>().ok()? * if fraction.len() == 1 { 10 } else { 1 }
    } else {
        return None;
    };
    let value = whole
        .parse::<u32>()
        .ok()?
        .checked_mul(100)?
        .checked_add(decimal)?;
    (value <= 10000).then_some(value / 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(fps: f32, dropped: f32) -> FrameSample {
        FrameSample { fps, dropped }
    }

    #[test]
    fn publish_forwards_every_change_without_thresholds() {
        let sink = Arc::new(Mutex::new(EngineStatus::default()));
        let mut status = EngineStatus {
            state: "active",
            source: "timestats",
            reason: "",
            fps: 59.5,
            gpu: 12.0,
            boost: 12,
            override_active: true,
            target: 60,
            ticks: 1,
        };
        publish(&sink, &status);
        assert_eq!(sink.lock().unwrap().boost, 12);
        // 12 与 36 这类历史门限会吞掉的加压档位现在必须照常发布。
        status.boost = 24;
        status.ticks = 2;
        publish(&sink, &status);
        assert_eq!(sink.lock().unwrap().boost, 24);
        // 帧率小幅变化也转发：守护进程自带 2 秒节奏，引擎层不再去抖。
        status.fps = 60.4;
        publish(&sink, &status);
        assert_eq!(sink.lock().unwrap().fps, 60.4);
        assert_eq!(sink.lock().unwrap().ticks, 2);
    }

    #[test]
    fn invalid_frame_samples_do_not_raise_performance_boost() {
        let mut controller = Controller::default();
        assert_eq!(controller.on_frame(80, sample(f32::NAN, 0.0)), 0);
        assert_eq!(controller.on_frame(80, sample(60.0, f32::NAN)), 0);
        assert_eq!(controller.on_frame(80, sample(60.0, 1.5)), 0);
    }

    #[test]
    fn timestats_lists_only_matching_layers() {
        let text = "timeStats: enabled\n\
            layerName = SurfaceView[com.miHoYo.Yuanshen/com.miHoYo.GetMobileInfo.MainActivity](BLAST)#0\n\
            packageNames = com.miHoYo.Yuanshen\n\
            totalFrames = 120\n\
            droppedFrames = 3\n\
            averageFPS = 59.87\n\
            layerName = com.android.systemui.ImageWallpaper#0\n\
            packageNames = com.android.systemui\n\
            totalFrames = 4\n\
            droppedFrames = 0\n";
        assert_eq!(
            parse_timestats(text, "com.miHoYo.Yuanshen"),
            vec![LayerFrames {
                base: "SurfaceView[com.miHoYo.Yuanshen/com.miHoYo.GetMobileInfo.MainActivity](BLAST)"
                    .into(),
                total: 120,
                dropped: 3,
            }]
        );
        assert_eq!(parse_timestats(text, "com.other.app"), Vec::new());
        assert_eq!(parse_timestats("timeStats: enabled\n", "com.x"), Vec::new());
    }

    #[test]
    fn timestats_keeps_layers_separate_and_tolerates_missing_fields() {
        // 同包名的 SurfaceView 与 Activity 窗口层保持独立条目，不再求和：
        // 主图层（游戏画面）由 elect_primary 按窗口帧数选举。
        let text = "layerName = SurfaceView[com.game/com.game.MainActivity](BLAST)#0\npackageNames = com.game\ntotalFrames = 10\n\
            layerName = com.game/com.game.PopupMenu#7\npackageNames = com.game\ntotalFrames = 5\ndroppedFrames = 1\n\
            layerName = c\npackageNames = \ntotalFrames = 9\n";
        let layers = parse_timestats(text, "com.game");
        assert_eq!(layers.len(), 2);
        assert_eq!(
            layers[0].base,
            "SurfaceView[com.game/com.game.MainActivity](BLAST)"
        );
        assert_eq!(layers[0].total, 10);
        assert_eq!(layers[1].base, "com.game/com.game.PopupMenu");
        assert_eq!(layers[1].total, 5);
        assert_eq!(layers[1].dropped, 1);
        // packageNames 为空行后的块不属于任何已知包。
        assert_eq!(parse_timestats(text, "com.launcher"), Vec::new());
    }

    #[test]
    fn layer_base_keys_survive_rebuild_suffix_changes() {
        assert_eq!(
            layer_base_key("com.game/com.game.MainActivity#1226"),
            "com.game/com.game.MainActivity"
        );
        assert_eq!(
            layer_base_key("SurfaceView[com.game/com.game.MainActivity](BLAST)#0"),
            "SurfaceView[com.game/com.game.MainActivity](BLAST)"
        );
        // 无序号或非数字后缀保持原样。
        assert_eq!(
            layer_base_key("com.game/com.game.MainActivity"),
            "com.game/com.game.MainActivity"
        );
        assert_eq!(layer_base_key("none"), "none");
    }

    #[test]
    fn window_delta_switches_between_diff_and_restart_models() {
        // clear 不生效：累计连续，用差分。
        assert_eq!(window_delta(600, 900, true), 300);
        // clear 生效：下次 dump 从 0 重来（当前值小于上次值），当前累计
        // 就是本窗口帧数。
        assert_eq!(window_delta(600, 240, true), 240);
        // 首窗或应用切换后：不作差分。
        assert_eq!(window_delta(0, 240, false), 240);
        // 相等累计（整窗零出帧）视为 0。
        assert_eq!(window_delta(600, 600, true), 0);
    }

    #[test]
    fn primary_layer_election_is_sticky_and_switches_on_real_changes() {
        let windows = vec![
            ("SurfaceView[com.game/act](BLAST)".to_string(), 60u64),
            ("com.game/act".to_string(), 4u64),
        ];
        // 帧数最多的 SurfaceView 是主图层；菜单层帧数少不参选。
        assert_eq!(
            elect_primary(&windows, None).map(String::as_str),
            Some("SurfaceView[com.game/act](BLAST)")
        );
        // 菜单层短暂出帧较多（如过场动画 < 2 倍）不切换主图层。
        let wobble = vec![
            ("SurfaceView[com.game/act](BLAST)".to_string(), 30u64),
            ("com.game/act".to_string(), 55u64),
        ];
        assert_eq!(
            elect_primary(&wobble, Some("SurfaceView[com.game/act](BLAST)")).map(String::as_str),
            Some("SurfaceView[com.game/act](BLAST)")
        );
        // 新图层帧数远超（≥2 倍）才真正切换。
        let switched = vec![
            ("SurfaceView[com.game/act](BLAST)".to_string(), 5u64),
            ("com.game/act".to_string(), 55u64),
        ];
        assert_eq!(
            elect_primary(&switched, Some("SurfaceView[com.game/act](BLAST)")).map(String::as_str),
            Some("com.game/act")
        );
        // 整窗零出帧：维持主图层身份，按“未渲染”处理而不是丢信号。
        let idle = vec![
            ("SurfaceView[com.game/act](BLAST)".to_string(), 0u64),
            ("com.game/act".to_string(), 0u64),
        ];
        assert_eq!(
            elect_primary(&idle, Some("SurfaceView[com.game/act](BLAST)")).map(String::as_str),
            Some("SurfaceView[com.game/act](BLAST)")
        );
        assert_eq!(elect_primary(&idle, None), None);
    }

    #[test]
    fn hyperos_android16_blocks_match_by_packagename_and_layer_name() {
        // 小米 14（HyperOS / Android 16，SM8650）真实输出结构：
        // 图层块以 displayRefreshRate 开头、packageName 为单数键且常为空、
        // 包名藏在 layerName 里；开头还有一段不带图层的 legacy 统计。
        let text = "SurfaceFlinger TimeStats:\n\
            totalFrames = 89\n\
            missedFrames = 10\n\
            displayOnTime = 2178 ms\n\
            displayRefreshRate = 120 fps\n\
            renderRate = 120 fps\n\
            uid = 10343\n\
            layerName = com.eg.android.AlipayGphone/com.alipay.mobile.nebulax.xriver.activity.XRiverTransActivity$Main#1226\n\
            packageName = \n\
            gameMode = Unsupported\n\
            totalFrames = 39\n\
            droppedFrames = 0\n\
            averageFPS = 23.651\n\
            displayRefreshRate = 120 fps\n\
            uid = 10332\n\
            layerName = InputMethod#1232\n\
            packageName = \n\
            totalFrames = 42\n\
            droppedFrames = 0\n\
            displayRefreshRate = 120 fps\n\
            uid = 10332\n\
            layerName = none\n\
            totalFrames = 0\n\
            droppedFrames = 0\n";
        // packageName 为空时回退到 layerName 提取，精确匹配前台包；
        // base 去掉 #1226 序号。
        assert_eq!(
            parse_timestats(text, "com.eg.android.AlipayGphone"),
            vec![LayerFrames {
                base: "com.eg.android.AlipayGphone/com.alipay.mobile.nebulax.xriver.activity.XRiverTransActivity$Main".into(),
                total: 39,
                dropped: 0,
            }]
        );
        // 无包名系统图层（InputMethod/none）不归属任何应用；legacy 段不计。
        assert_eq!(parse_timestats(text, "com.android.systemui"), Vec::new());
    }

    #[test]
    fn surface_view_bracket_layers_match_their_package() {
        let text = "layerName = SurfaceView[com.miHoYo.Yuanshen/com.miHoYo.GetMobileInfo.MainActivity](BLAST)#0\n\
            packageName = \n\
            totalFrames = 600\n\
            droppedFrames = 12\n";
        assert_eq!(
            parse_timestats(text, "com.miHoYo.Yuanshen"),
            vec![LayerFrames {
                base: "SurfaceView[com.miHoYo.Yuanshen/com.miHoYo.GetMobileInfo.MainActivity](BLAST)"
                    .into(),
                total: 600,
                dropped: 12,
            }]
        );
        assert_eq!(parse_timestats(text, "com.miHoYo.other"), Vec::new());
    }

    #[test]
    fn explicit_packagename_outranks_a_matching_layer_name() {
        // 显式归属其它包的图层不因图层名相似而误计。
        let text = "layerName = com.target.pkg/com.target.pkg.MainActivity#9\n\
            packageName = com.other.pkg\n\
            totalFrames = 30\n";
        assert_eq!(parse_timestats(text, "com.target.pkg"), Vec::new());
        assert_eq!(
            parse_timestats(text, "com.other.pkg"),
            vec![LayerFrames {
                base: "com.target.pkg/com.target.pkg.MainActivity".into(),
                total: 30,
                dropped: 0,
            }]
        );
    }

    #[test]
    fn boost_rises_fast_on_drops_and_decays_slowly_when_calm() {
        let mut controller = Controller::default();
        assert_eq!(controller.on_frame(80, sample(60.0, 0.0)), 0);
        assert_eq!(controller.on_frame(80, sample(60.0, 0.0)), 0);
        // 丢帧立即加压，不受连击去抖限制。
        assert_eq!(controller.on_frame(80, sample(58.0, 0.05)), 12);
        // 帧率跌破基线需要连续两拍才加压。
        controller.reset();
        assert_eq!(controller.on_frame(80, sample(60.0, 0.0)), 0);
        assert_eq!(controller.on_frame(80, sample(48.0, 0.0)), 0);
        assert_eq!(controller.on_frame(80, sample(48.0, 0.0)), 12);
    }

    #[test]
    fn calm_decay_returns_boost_to_zero_without_hunting() {
        let mut controller = Controller::default();
        for _ in 0..3 {
            controller.on_frame(80, sample(60.0, 0.0));
        }
        assert_eq!(controller.on_frame(80, sample(58.5, 0.03)), 12);
        // 稳定 5 拍回落一档：12 → 8 → 4 → 0，逐级缓降。
        for _ in 0..4 {
            assert_eq!(controller.on_frame(80, sample(60.0, 0.0)), 12);
        }
        assert_eq!(controller.on_frame(80, sample(60.0, 0.0)), 8);
        for _ in 0..5 {
            controller.on_frame(80, sample(60.0, 0.0));
        }
        assert_eq!(controller.boost, 4);
        for _ in 0..5 {
            controller.on_frame(80, sample(60.0, 0.0));
        }
        assert_eq!(controller.boost, 0);
    }

    #[test]
    fn boost_never_exceeds_mode_ceiling_and_resets_fully() {
        let mut controller = Controller::default();
        for _ in 0..40 {
            controller.on_frame(35, sample(10.0, 0.5));
        }
        assert_eq!(controller.boost, 35);
        controller.reset();
        assert_eq!(controller.boost, 0);
        assert_eq!(controller.stable_fps, 0.0);
        assert_eq!(controller.calm, 0);
    }

    #[test]
    fn sustained_shortfall_rebaselines_and_transient_dips_do_not() {
        // 开局抓到 120fps 高刷过场后游戏锁定 60：持续不足必须把基线拉回
        // 实际帧率，让回落路径重新可用，而不是永久锁死在上限。
        let mut controller = Controller::default();
        assert_eq!(controller.on_frame(80, sample(120.0, 0.0)), 0);
        let mut boost = 0;
        for _ in 0..200 {
            boost = controller.on_frame(80, sample(60.0, 0.0));
        }
        assert_eq!(boost, 0, "基线必须重新收敛到 60fps，加压完全回落");
        // 短暂抖动（<10 个采样）不改变基线：恢复 60fps 基线后，几次低于
        // 基线的抖动仍按原节奏加压。
        assert_eq!(controller.on_frame(80, sample(50.0, 0.0)), 0);
        assert_eq!(controller.on_frame(80, sample(50.0, 0.0)), 12);
    }

    #[test]
    fn sustained_shortfall_at_full_pressure_accepts_actual_and_releases() {
        // 性能不足：加压到上限仍达不到目标时，基线快速收敛到实际帧率，
        // 加压自然回落——上限加压无收益不得长期挂满空耗电。
        let mut controller = Controller::default();
        assert_eq!(controller.on_frame(80, sample(90.0, 0.0)), 0);
        let mut boost = 0;
        for _ in 0..300 {
            boost = controller.on_frame(80, sample(45.0, 0.0));
        }
        assert_eq!(boost, 0, "上限加压无收益必须回落，不得长期挂满");
    }

    #[test]
    fn mid_pressure_shortfall_keeps_pushing_instead_of_learning_low_target() {
        // 目标 120、实际 90、加压途中（未到上限）时不得把 90 学成目标：
        // 那会在还有加压空间时提前放弃。必须先推到上限，重学才被允许。
        let mut controller = Controller::default();
        assert_eq!(controller.on_frame(95, sample(120.0, 0.0)), 0);
        for _ in 0..12 {
            controller.on_frame(95, sample(90.0, 0.0));
        }
        assert!(controller.boost > 0, "持续不足应继续加压");
        for _ in 0..12 {
            controller.on_frame(95, sample(90.0, 0.0));
        }
        assert!(controller.boost >= 80, "应加压到上限而不是提前放弃");
    }

    #[test]
    fn paused_app_decays_pressure_faster_than_calm_path() {
        // 前台存在但停止渲染（fps<1）：按加倍步长回落，暂停几分钟不应
        // 保持游戏加压。
        let mut controller = Controller::default();
        for _ in 0..8 {
            controller.on_frame(80, sample(60.0, 0.5));
        }
        assert!(controller.boost > 0);
        for _ in 0..(CALM_DECAY_TICKS * 10) {
            controller.on_frame(80, sample(0.0, 0.0));
        }
        assert_eq!(controller.boost, 0, "停止渲染后加压必须回落到 0");
    }

    #[test]
    fn uclamp_write_respects_ownership_and_never_lowers_a_baseline() {
        // 加压归零：恢复调度器基线而不是写 0；基线为空回落 0。
        assert_eq!(resolve_uclamp_write("40", 0, "0"), ("40".into(), "".into()));
        assert_eq!(resolve_uclamp_write("", 0, "0"), ("0".into(), "".into()));
        // 规则地板 40 高于本拍加压 24：保持地板，引擎不拥有节点。
        assert_eq!(
            resolve_uclamp_write("40", 24, "24"),
            ("40".into(), "".into())
        );
        // 加压高于基线：写加压值并记录 fas_min，脉冲到期回到加压值。
        assert_eq!(
            resolve_uclamp_write("15", 36, "36"),
            ("36".into(), "36".into())
        );
        // 基线允许带小数（cgroup 读回 "40.00"）。
        assert_eq!(
            resolve_uclamp_write("40.00", 24, "24"),
            ("40.00".into(), "".into())
        );
    }

    #[test]
    fn heuristic_boosts_only_on_sustained_gpu_load() {
        let mut controller = Controller::default();
        assert_eq!(controller.on_load(80, 90.0), 0);
        assert_eq!(controller.on_load(80, 90.0), 12);
        assert_eq!(controller.on_load(80, 20.0), 12);
        for _ in 0..(CALM_DECAY_TICKS * 2) {
            controller.on_load(80, 20.0);
        }
        assert_eq!(controller.boost, 8);
    }

    #[test]
    fn heuristic_load_boost_is_capped_and_mid_band_decays() {
        // 无帧时间证据时 GPU 高负载不得把加压推到档位顶格：uclamp 与
        // 底频加压在没有帧收益证据时必须有界。
        let mut controller = Controller::default();
        let cap = 80 * LOAD_CAP_RATIO / 100;
        for _ in 0..40 {
            controller.on_load(80, 95.0);
        }
        assert_eq!(controller.boost, cap);
        // 中间负载区间（45..70）按低速档节奏回落，不再无限期保留高 boost。
        for _ in 0..(CALM_DECAY_TICKS * 2) {
            controller.on_load(80, 60.0);
        }
        assert_eq!(controller.boost, cap - DECAY_STEP);
    }

    #[test]
    fn heuristic_rejects_invalid_load_readings() {
        let mut controller = Controller::default();
        controller.on_load(80, 90.0);
        controller.on_load(80, 90.0);
        let boosted = controller.boost;
        assert!(boosted > 0);
        assert_eq!(controller.on_load(80, f32::NAN), boosted);
        assert_eq!(controller.on_load(80, 150.0), boosted);
    }

    #[test]
    fn percent_parsing_matches_cgroup_semantics() {
        assert_eq!(parse_percent("30"), Some(30));
        assert_eq!(parse_percent("30.00"), Some(30));
        assert_eq!(parse_percent("max"), Some(100));
        for invalid in ["101", "1.234", "NaN", "", "-1"] {
            assert_eq!(parse_percent(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn little_core_discount_follows_physical_frequency_not_slot_order() {
        // 审计 CF-1 回归：常规布局槽0=小核，折扣仍在槽0。
        assert_eq!(
            little_core_weights(&[Some(1_800_000), Some(2_800_000), Some(3_300_000)]),
            vec![0.6, 1.0, 1.0]
        );
        // 用户自定义槽序（大核在前）：折扣必须跟物理小核走，不能落在槽0。
        assert_eq!(
            little_core_weights(&[Some(3_300_000), Some(1_800_000)]),
            vec![1.0, 0.6]
        );
        // 首槽禁用 + 两簇并列最低：识别不了，全部 1.0，不猜。
        assert_eq!(
            little_core_weights(&[None, Some(2_000_000), Some(2_000_000)]),
            vec![1.0, 1.0, 1.0]
        );
        // 单簇没有折扣对象；读不到物理频率同样不猜。
        assert_eq!(little_core_weights(&[Some(2_800_000)]), vec![1.0]);
        assert_eq!(little_core_weights(&[None, None]), vec![1.0, 1.0]);
    }

    #[test]
    fn pidfd_liveness_tracks_child_exit() {
        // 审计 P2-1 回归：pidfd 存活探测必须随子进程退出翻转；PID 复用
        // 场景依赖该判定把旧条目让位给新进程。内核无 pidfd 时本测试空转。
        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("sleep 30")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn sleep child");
        let pid = child.id() as i32;
        let Some(pidfd) = pidfd_open(pid) else {
            child.kill().ok();
            let _ = child.wait();
            return;
        };
        assert!(pidfd_alive(&pidfd), "存活子进程的 pidfd 必须报告存活");
        child.kill().expect("kill child");
        child.wait().expect("reap child");
        assert!(!pidfd_alive(&pidfd), "已退出子进程的 pidfd 必须报告挂断");
    }

    #[test]
    fn gpu_bottleneck_caps_boost_and_is_tagged_after_three_slowed_samples() {
        // 第二阶段·CPU/GPU 协同：帧率不足 + GPU ≥85% 连续三拍 → 判定 GPU
        // 瓶颈，CPU 加压上限收窄到 ceiling×60%，状态标注瓶颈。
        let mut controller = Controller::default();
        assert_eq!(controller.on_frame(80, sample(60.0, 0.0)), 0);
        for _ in 0..2 {
            controller.on_frame_evaluated(80, sample(50.0, 0.0), 0, Some(92.0));
        }
        assert_eq!(controller.bottleneck, "", "不足三拍不下结论");
        let boost = controller.on_frame_evaluated(80, sample(50.0, 0.0), 0, Some(92.0));
        assert_eq!(controller.bottleneck, "gpu");
        let mut boost = boost;
        for _ in 0..40 {
            boost = controller.on_frame_evaluated(80, sample(50.0, 0.0), 0, Some(92.0));
        }
        assert_eq!(boost, 80 * LOAD_CAP_RATIO / 100, "GPU 瓶颈时加压收窄");
        // GPU 回落后计数清零，瓶颈解除，CPU 恢复全幅度（突破 GPU 期的收窄上限；
        // 之后到达 ceiling 会触发既有的上限重学回落，属另一路径，这里不验证）。
        for _ in 0..(GPU_BOUND_STREAK + 1) {
            controller.on_frame_evaluated(80, sample(50.0, 0.0), 0, Some(30.0));
        }
        assert_eq!(controller.bottleneck, "cpu");
        let mut boost = 48;
        for _ in 0..6 {
            boost = controller.on_frame_evaluated(80, sample(50.0, 0.0), 0, Some(30.0));
        }
        assert!(
            boost > 80 * LOAD_CAP_RATIO / 100,
            "CPU 瓶颈不得被 GPU 收窄限制（实际 {boost}）"
        );
    }

    #[test]
    fn mid_gpu_load_or_missing_gpu_does_not_conclude_bottleneck() {
        let mut controller = Controller::default();
        // 先以健康帧率建立基线（首采样会初始化基线），再进入持续不足。
        assert_eq!(controller.on_frame_evaluated(80, sample(60.0, 0.0), 0, Some(20.0)), 0);
        let mut boost = 0;
        for _ in 0..20 {
            boost = controller.on_frame_evaluated(80, sample(50.0, 0.0), 0, Some(70.0));
        }
        assert_eq!(controller.bottleneck, "", "中间负载不轻率下结论");
        assert!(boost > 0);
        let mut no_gpu = Controller::default();
        assert_eq!(no_gpu.on_frame_evaluated(80, sample(60.0, 0.0), 0, None), 0);
        let mut boost = 0;
        for _ in 0..20 {
            boost = no_gpu.on_frame_evaluated(80, sample(50.0, 0.0), 0, None);
        }
        assert_eq!(no_gpu.bottleneck, "");
        assert!(boost > 0);
    }

    #[test]
    fn healthy_or_paused_frames_clear_bottleneck_state() {
        let mut controller = Controller::default();
        assert_eq!(controller.on_frame_evaluated(80, sample(60.0, 0.0), 0, Some(20.0)), 0);
        for _ in 0..5 {
            controller.on_frame_evaluated(80, sample(50.0, 0.0), 0, Some(92.0));
        }
        assert_eq!(controller.bottleneck, "gpu");
        // 恢复健康帧率：瓶颈与计数一并清零。
        controller.on_frame_evaluated(80, sample(60.0, 0.0), 0, Some(20.0));
        assert_eq!(controller.bottleneck, "");
        assert_eq!(controller.gpu_bound_streak, 0);
        // 停止渲染路径同样清零。
        for _ in 0..5 {
            controller.on_frame_evaluated(80, sample(50.0, 0.0), 0, Some(92.0));
        }
        assert_eq!(controller.bottleneck, "gpu");
        controller.on_frame_evaluated(80, sample(0.0, 0.0), 0, Some(92.0));
        assert_eq!(controller.bottleneck, "");
        assert_eq!(controller.gpu_bound_streak, 0);
    }

    #[test]
    fn floor_resolution_supports_khz_and_percent_notation() {
        let dir = std::env::temp_dir().join(format!("nova-fas-floor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("scaling_available_frequencies"), "300000 1000000 2000000\n")
            .unwrap();
        let mut floor = None;
        let mut key = String::new();
        let shared = FasShared {
            min_floor: vec!["0".into()],
            ..Default::default()
        };
        assert_eq!(
            resolve_floor(&mut floor, &mut key, 0, &shared, &dir, 3000000),
            Some(0)
        );
        let shared = FasShared {
            min_floor: vec!["50%".into()],
            ..Default::default()
        };
        assert_eq!(
            resolve_floor(&mut floor, &mut key, 0, &shared, &dir, 3000000),
            Some(2000000)
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
