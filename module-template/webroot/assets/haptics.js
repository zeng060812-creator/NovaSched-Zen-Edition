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
  let systemBridge=null,systemBusy=false;
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
  /* 当前实际生效的通道：native / vibrate / off（用于设置页如实展示）。 */
  function channel() {
    if (!enabled) return "off";
    if (isNativeAvailable()) return "native";
    if(systemBridge)return "system";
    if (vibrateSupported) return "vibrate";
    return "off";
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
    if(systemBridge){if(systemBusy)return false;systemBusy=true;Promise.resolve().then(()=>enabled&&!document.hidden?systemBridge(semantic):undefined).catch(()=>{systemBridge=null;window.dispatchEvent(new Event("nova:haptics"));}).finally(()=>{systemBusy=false;});return true;}
    if (vibrateSupported) {
      try { return navigator.vibrate(pattern); } catch (_) { return false; }
    }
    return false;
  }

  window.NovaHaptics = Object.freeze({
    fire,
    select: () => fire("select"),
    toggle: () => fire("toggle"),
    tick: () => fire("tick"),
    success: () => fire("success"),
    failure: () => fire("failure"),
    warning: () => fire("warning"),
    setSystemBridge:fn=>{systemBridge=typeof fn==="function"?fn:null;window.dispatchEvent(new Event("nova:haptics"));},
    setEnabled,
    isEnabled,
    channel,
    isNativeAvailable,
    isVibrateAvailable,
  });
})();
