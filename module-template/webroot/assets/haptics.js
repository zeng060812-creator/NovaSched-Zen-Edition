/* ZenJooo · Semantic haptics. Native host, verified system command, then Web Vibration. */
(() => {
  "use strict";
  const STORAGE = "novasched.haptics.v1";
  const PATTERNS = {
    /* 借鉴 iOS 开关/选择的节奏：短促、干脆，不追求“完全等同 iOS”。 */
    select: [8],
    toggle: [12],
    tick: [5],
    success: [10, 60, 18],
    failure: [22, 50, 22],
    warning: [14, 40, 14],
  };
  /* 系统命令通道状态：idle 未探测 / ready 探测通过且上次提交成功 /
     failed 上次提交失败（有界重试后回退） / unavailable 探测确认不可用。 */
  let systemBridge = null, systemBusy = false;
  let systemState = { status: "idle", detail: "", hapticEnabled: null };
  let systemFailures = 0;
  let enabled = true;
  let vibrateSupported = false;
  let lastAt = 0;
  let lastSemantic = "";
  try { enabled = localStorage.getItem(STORAGE) !== "0"; } catch (_) { /* 宿主可能禁用存储 */ }
  try { vibrateSupported = typeof navigator.vibrate === "function"; } catch (_) { vibrateSupported = false; }

  const nativeBridge = () =>
    typeof window.NovaHostHaptics === "function" ? window.NovaHostHaptics : null;

  function setEnabled(value) {
    enabled = !!value;
    try { localStorage.setItem(STORAGE, enabled ? "1" : "0"); } catch (_) { /* 忽略 */ }
  }
  function isEnabled() {
    return enabled;
  }
  function isNativeAvailable() {
    return nativeBridge() !== null;
  }
  function isVibrateAvailable() {
    return vibrateSupported;
  }
  /* 当前实际生效的通道：native / system / vibrate / off。failed 的系统
     通道不再被选中（回退振动），但保留状态供诊断显示。 */
  function channel() {
    if (!enabled) return "off";
    if (isNativeAvailable()) return "native";
    if (systemBridge && systemState.status === "ready") return "system";
    if (vibrateSupported) return "vibrate";
    return "off";
  }
  function systemStatus() {
    return { ...systemState, wired: systemBridge !== null };
  }

  /* 提交一次系统触感：resolve 值为 "submitted"（命令已受理）或
     "skipped"（系统触感开关关闭，按设置静默）；reject 携带简短原因。
     注意：提交成功不代表用户实际感受到振动，软件侧无法确认体感。 */
  async function submitSystem(semantic) {
    if (!systemBridge) throw new Error("系统触感通道未接入");
    const outcome = await systemBridge(semantic);
    if (outcome === "skipped") return "skipped";
    if (outcome !== "submitted") throw new Error("系统触感命令未确认提交");
    return "submitted";
  }

  function trackSystemOutcome(semantic, promise) {
    promise
      .then(outcome => {
        systemFailures = 0;
        systemState = { ...systemState, status: "ready", detail: outcome === "skipped" ? "系统触感开关关闭" : "" };
      })
      .catch(error => {
        // 临时失败不清空通道：两次连续失败才降级，由探测入口负责恢复，
        // 避免一次 root 瞬断把通道永久误判为不可用。
        systemFailures += 1;
        systemState = {
          ...systemState,
          status: systemFailures >= 2 ? "unavailable" : "failed",
          detail: String(error?.message || error || "提交失败").slice(0, 120),
        };
        if (systemFailures >= 2) systemFailures = 2;
      })
      .finally(() => window.dispatchEvent(new Event("nova:haptics")));
    return promise;
  }

  /* 同语义最小间隔，防止列表滚动/连点把振动排成队列；不同语义不互相限频。 */
  const THROTTLE_MS = { tick: 60, select: 40, toggle: 40 };
  function fire(semantic) {
    if (!enabled) return false;
    if (document.hidden) return false;
    const pattern = PATTERNS[semantic];
    if (!pattern) return false;
    const now = performance.now();
    const throttle = THROTTLE_MS[semantic];
    if (throttle && semantic === lastSemantic && now - lastAt < throttle) return false;
    lastAt = now;
    lastSemantic = semantic;
    const bridge = nativeBridge();
    if (bridge) {
      try { bridge(semantic); return true; } catch (_) { /* 桥异常按降级处理 */ }
    }
    if (systemBridge && systemState.status === "ready") {
      if (systemBusy) return false; // 上一发还在路上：丢弃而不是积压排队
      systemBusy = true;
      trackSystemOutcome(semantic, Promise.resolve()
        .then(() => (!enabled || document.hidden) ? Promise.reject(new Error("页面已隐藏或触感已关闭")) : submitSystem(semantic))
        .catch(error => {
          // 系统通道未受理且并非静音跳过（skipped 走 resolve，不会进这里）：
          // 本次交互立即以 WebView 振动兜底，不让触感落空；通道状态仍由
          // trackSystemOutcome 记账，连续两次失败自动降级。
          if (enabled && !document.hidden && vibrateSupported) {
            try { navigator.vibrate(pattern); } catch (_) { /* 忽略 */ }
          }
          throw error;
        })
        .finally(() => { systemBusy = false; }));
      return true;
    }
    if (vibrateSupported) {
      try { return navigator.vibrate(pattern); } catch (_) { return false; }
    }
    return false;
  }

  /* 测试触感：走真实链路提交一次 select，返回简短可定位的结果文案。
     软件只能确认“请求已提交/被系统设置跳过/失败原因”，无法确认体感。 */
  async function test() {
    if (!enabled) return { ok: false, message: "触感已关闭，请先打开开关" };
    const bridge = nativeBridge();
    if (bridge) {
      try { bridge("select"); return { ok: true, message: "已通过宿主原生触感提交" }; }
      catch (error) { /* 原生桥异常时继续尝试其它通道 */ }
    }
    if (systemBridge && systemState.status !== "unavailable") {
      if (systemBusy) return { ok: false, message: "上一次触感仍在提交，请稍后再试" };
      try {
        const outcome = await submitSystem("select");
        systemFailures = 0;
        systemState = { ...systemState, status: "ready", detail: outcome === "skipped" ? "系统触感开关关闭" : "" };
        window.dispatchEvent(new Event("nova:haptics"));
        return outcome === "skipped"
          ? { ok: false, message: "系统触感已在系统设置中关闭：提交被静默跳过" }
          : { ok: true, message: "已提交系统触感请求（软件无法确认实际体感）" };
      } catch (error) {
        systemFailures += 1;
        systemState = {
          ...systemState,
          status: systemFailures >= 2 ? "unavailable" : "failed",
          detail: String(error?.message || error || "提交失败").slice(0, 120),
        };
        window.dispatchEvent(new Event("nova:haptics"));
        return { ok: false, message: `系统触感提交失败：${systemState.detail}` };
      }
    }
    if (vibrateSupported) {
      try {
        const played = navigator.vibrate(PATTERNS.select);
        return played
          ? { ok: true, message: "已触发设备振动（Web Vibration 回退）" }
          : { ok: false, message: "设备振动接口拒绝执行（可能被系统限制）" };
      } catch (error) {
        return { ok: false, message: `设备振动失败：${String(error?.message || error).slice(0, 80)}` };
      }
    }
    return { ok: false, message: systemState.detail ? `当前宿主未提供触感能力（${systemState.detail}）` : "当前宿主未提供触感能力" };
  }

  window.NovaHaptics = Object.freeze({
    fire,
    select: () => fire("select"),
    toggle: () => fire("toggle"),
    tick: () => fire("tick"),
    success: () => fire("success"),
    failure: () => fire("failure"),
    warning: () => fire("warning"),
    test,
    /* meta: { detail, hapticEnabled } —— 探测结果随通道一起登记，供设置页
       如实展示（含“系统触感开关已关闭”的区分）。 */
    setSystemBridge: (fn, meta = {}) => {
      systemBridge = typeof fn === "function" ? fn : null;
      systemFailures = 0;
      systemState = {
        status: systemBridge ? "ready" : "unavailable",
        detail: String(meta.detail || "").slice(0, 120),
        hapticEnabled: typeof meta.hapticEnabled === "boolean" ? meta.hapticEnabled : null,
      };
      window.dispatchEvent(new Event("nova:haptics"));
    },
    systemStatus,
    setEnabled,
    isEnabled,
    channel,
    isNativeAvailable,
    isVibrateAvailable,
  });
})();
