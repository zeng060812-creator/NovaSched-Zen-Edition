/* ZenJooo · NovaSched WebUI. */
(() => {
  "use strict";
  const $ = (s, root = document) => root.querySelector(s);
  const $$ = (s, root = document) => [...root.querySelectorAll(s)];
  const KEY = "!TcsEUQ#Be8bk4bk!dcf341Bo4NwXdQi8hik2_l3BXOJ2$hQOeHKDAUL1jrQUnkx";
  const AUTH_PREFIX = "novasched-auth.";
  const own = (object, key) => Object.prototype.hasOwnProperty.call(object, key);
  let MODULE = "/data/adb/modules/NovaSched_Zen_Edition";
  const MODULE_ID = "NovaSched_Zen_Edition";
  const MODULE_CANDIDATES = [MODULE, "/data/adb/ap/modules/" + MODULE_ID];
  try {
    const path = decodeURIComponent(new URL(location.href).pathname);
    const match = path.match(/^(\/data\/adb\/(?:modules|ap\/modules)\/NovaSched_Zen_Edition)(?:\/|$)/);
    if (match) { MODULE = match[1]; MODULE_CANDIDATES.unshift(MODULE); }
  } catch (_) {}
  const STORAGE = "novasched.zen.ui.v2";
  const UI_VERSION = window.NovaRelease?.version || "2.13.2";

  const MODES = {
    powersave:   { name: "省电", copy: "限制 CPU 频率上限，适合消息、阅读和轻量日用。" },
    balance:     { name: "均衡", copy: "兼顾应用响应与功耗，适合日常多任务。" },
    performance: { name: "性能", copy: "提高 CPU 可用频段，适合持续负载和常规游戏。" },
    fast:        { name: "极速", copy: "开放更高频率上限，NovaFAS 按可用帧信号动态调节。" },
  };
  const ENGINE_TITLES = {
    off: "NovaFAS 未启用", idle: "NovaFAS 待命", active: "NovaFAS 已接管", degraded: "NovaFAS 信号不可用", paused: "NovaFAS 已暂停",
  };
  const THEME_NAMES = { system: "跟随系统", light: "浅色", dark: "深色" };
  /* 仅 6 个系统表面的固定事实名称（AOSP/HyperOS 的真实 Label，非猜测）：
     元数据通道就绪前的前台展示兜底，避免桌面/通知栏过场显示裸包名。
     任何第三方应用的名称一律来自 PackageManager（AppInfoProvider），绝不
     写死或按包名猜测。 */
  const SYSTEM_NAMES = {
    "com.android.systemui": "系统界面", "com.android.settings": "系统设置",
    "com.omarea.vtools": "Scene", "com.android.launcher3": "系统桌面",
    "com.google.android.webview": "Android 系统 WebView", "com.miui.home": "系统桌面",
  };

  const s = {
    page: "overview", pendingControls: 0, online: false, connecting: true, phase: "", error: "", connectionError: "", version: "",
    mode: "", effective: "", pkg: "", controller: "", external: false, linked: false,
    /* Scene 接管存档（novasched.scene.mode 非空且已联动）：横幅 + 锁四档 */
    sceneDriving: false, cmdError: "",
    heartbeat: null, soc: "", socId: "", configProfile: "", fasAvailable: false, fasState: "unknown", fasMode: "", fasSync: "", fasLegacy: false, sceneInstalled: false, scenePresence: "unknown",
    fasEngineState: "", fasEngineSource: "", fasEngineReason: "", fasFps: 0, fasBoost: 0, fasOverride: false, fasGpu: 0, fasTick: 0, samsungSafe: false,
    extremePower: false, extremeStatus: "off", extremeError: "", extremePending: null,
    freqs: [],
    automatic: false, automaticActive: false, appKind: "", classificationSource: "",
    rules: [], rulesLoaded: false, overrides: {},
    /* 目标 FPS 规则（fps_rules.json）。null = 尚未探测过；探测成功后缓存，
       规则编辑器每次打开都强制重探，避免他处增删后页面显示陈旧值。 */
    fpsRules: null,
    /* 面板刷新率规则（refresh_rules.json，Scene 同款 SurfaceFlinger 模式
       切换）。refreshHzList = 设备真实模式里出现过的 Hz 值，构建选项用。 */
    refreshRules: null, refreshHzList: [], refreshModesError: "",
    /* 图形后端规则（独立存储 graphics_rules.json，与档位规则互不影响）：
       available=null 未探测 / true / false（false 时 angleAvailableReason 给出原因）。 */
    graphics: { probed: false, available: null, reason: "", rules: {}, status: {}, capability: null },
    pending: null, sockets: {}, suspended: false, retry: 0, epoch: 0, probes: new Set(),
    transport: "", rootTimer: 0, rootJob: null, rootOrigin: "", rootGeneration: 0, logCursor: "",
    discovered: false, logConnected: false, logConnecting: false, logs: [], logTail: "", level: "all", follow: true,
    logReceived: false,
  };
  const ACCENTS = { blue: "蓝色", indigo: "靛色", purple: "紫色", rose: "粉色", amber: "橙色", green: "绿色" };
  const ui = {
    theme: "system", accent: "blue", glassLevel: 50, wallBlur: 6, wallBright: 93, wallData: "",
    cardBlur: 24, cardAlpha: 73, cardRadius: 22, chromeBlur: 100, dispersion: true, material: true,
    colorMode: "colorful", cardColor: "#d6e9ff", palette: ["#64b5f6", "#f48fb1", "#81c784", "#ffd166", "#b39ddb", "#4dd0e1"],
    reduceMotion: false, portrait: { centerX: .5, centerY: .5, scale: 1 }, landscape: { centerX: .5, centerY: .5, scale: 1 },
  };
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE) || "{}");
    if (own(THEME_NAMES, saved.theme)) ui.theme = saved.theme;
    if (own(ACCENTS, saved.accent)) ui.accent = saved.accent;
    if (Number.isInteger(saved.glassLevel) && saved.glassLevel >= 0 && saved.glassLevel <= 100) ui.glassLevel = saved.glassLevel;
    if (Number.isInteger(saved.wallBlur) && saved.wallBlur >= 0 && saved.wallBlur <= 100) ui.wallBlur = saved.wallBlur;
    if (Number.isInteger(saved.wallBright) && saved.wallBright >= 35 && saved.wallBright <= 100) ui.wallBright = saved.wallBright;
    if (typeof saved.wallData === "string" && /^data:image\/(?:jpeg|png|webp);base64,/.test(saved.wallData)) ui.wallData = saved.wallData.slice(0, 3000000);
    for (const [key, min, max] of [["cardBlur",0,64],["cardAlpha",0,100],["cardRadius",10,34],["chromeBlur",0,200]])
      if (Number.isInteger(saved[key]) && saved[key] >= min && saved[key] <= max) ui[key] = saved[key];
    for (const key of ["dispersion", "material", "reduceMotion"]) if (typeof saved[key] === "boolean") ui[key] = saved[key];
    if (["solid", "gradient", "colorful"].includes(saved.colorMode)) ui.colorMode = saved.colorMode;
    if (/^#[\da-f]{6}$/i.test(saved.cardColor)) ui.cardColor = saved.cardColor;
    if (Array.isArray(saved.palette) && saved.palette.length && saved.palette.length <= 12 && saved.palette.every(color => /^#[\da-f]{6}$/i.test(color))) ui.palette = saved.palette;
    for (const key of ["portrait", "landscape"]) if (saved[key] && typeof saved[key] === "object") {
      const crop = saved[key];
      if ([crop.centerX, crop.centerY, crop.scale].every(Number.isFinite)) ui[key] = {
        centerX: Math.max(0,Math.min(1,crop.centerX)), centerY: Math.max(0,Math.min(1,crop.centerY)), scale: Math.max(1,Math.min(4,crop.scale))
      };
    }
  } catch (_) { /* 宿主 WebView 可能禁用存储 */ }
  const saveUI = () => {
    try { localStorage.setItem(STORAGE, JSON.stringify(ui)); return true; }
    catch (_) { toast("外观已应用，但当前宿主无法保存设置"); return false; }
  };

  const darkQuery = window.matchMedia?.("(prefers-color-scheme: dark)");

  /* ---------------- 小工具 ---------------- */
  let snackbarTimer = 0;
  let pendingTimer = 0;
  let bridgeCounter = 0;
  let sessionToken = "";
  let authRequest = null;
  let bridgeSignature = 0;
  let bridgeApi = null;
  const esc = (value) => String(value ?? "").replace(/[&<>"']/g, c => ({ "&":"&amp;", "<":"&lt;", ">":"&gt;", '"':"&quot;", "'":"&#39;" }[c]));
  const basePackage = (raw) => String(raw || "").split(":", 1)[0];
  const validPackage = (raw) => typeof raw === "string" && raw.length <= 255 && /^[A-Za-z][A-Za-z0-9_]*(?:\.[A-Za-z][A-Za-z0-9_]*)+(?::[A-Za-z0-9_.]+)?$/.test(raw);
  const editable = () => s.online && !s.external;
  const present = (id, text) => { const node = $(id); if (node && node.textContent !== String(text)) node.textContent = String(text); };

  function toast(message, timeout = 3800) {
    const bar = $("#snackbar");
    bar.textContent = message;
    bar.classList.add("show");
    clearTimeout(snackbarTimer);
    snackbarTimer = setTimeout(() => bar.classList.remove("show"), timeout);
  }
  function modeNote(message, error = false) {
    const node = $("#mode-note");
    node.textContent = message;
    node.dataset.error = error ? "1" : "0";
  }
  async function copyText(text, message = "已复制") {
    if (!text) return toast("没有可复制的内容");
    try {
      if (navigator.clipboard?.writeText) { await navigator.clipboard.writeText(text); return toast(message); }
    } catch (_) { /* 部分 WebView 拒绝剪贴板权限 */ }
    let field;
    try {
      field = document.createElement("textarea"); field.value = text;
      Object.assign(field.style, { position:"fixed", opacity:"0", pointerEvents:"none" });
      document.body.append(field); field.select();
      toast(document.execCommand("copy") ? message : "当前 WebView 未允许复制，请长按选择文本");
    } catch (_) { toast("当前 WebView 未允许复制"); }
    finally { field?.remove(); }
  }
  function appInfo(raw) {
    const pkg = basePackage(raw);
    const info = labels.get(pkg);
    const label = info?.appLabel || SYSTEM_NAMES[pkg] || "";
    const name = label || pkg || "无前台应用";
    return { package: String(raw || ""), base: pkg, name,
      /* 模块 meta 通道提供的 96px 真实图标（base64 PNG）；宿主桥不提供图标。 */
      icon: typeof info?.icon === "string" ? info.icon : "",
      /* 系统设置里被停用的应用（模块 meta 通道提供；null=未知）。 */
      enabled: typeof info?.enabled === "boolean" ? info.enabled : null };
  }
  /* 统一占位图标：AppInfo 未就绪/读取失败时的中性图形。按规格不使用
     首字母代替图标，也不按包名生成随机颜色——只有真实 PackageManager
     图标（模块 meta 通道）才渲染为应用图标。 */
  const APP_PLACEHOLDER = '<svg viewBox="0 0 24 24" aria-hidden="true"><rect x="4.5" y="4.5" width="15" height="15" rx="4.5" fill="none" stroke="currentColor" stroke-width="1.5" opacity=".38"/><circle cx="9.4" cy="9.4" r="1.5" fill="currentColor" opacity=".42"/><path d="M5.5 16l3.6-3.6 2.9 2.9 4.4-4.8 2.1 2.2" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" opacity=".42"/></svg>';

  /* ---------------- 应用名查询（宿主元数据桥） ----------------
     只有“宿主明确应答”的包才记入 queried（含应用已卸载的确认）；
     超时、异常或返回格式未知时保留有界重试机会，不把失败永久缓存。
     宿主无 getPackagesInfo 接口时如实显示包名——按包名查询应用名的
     根回退（app_process DEX 辅助）见 docs/v2.12.1-report.md 边界说明。 */
  const labels = new Map();
  const queried = new Set();
  const labelFails = new Map();
  const LABEL_MAX_FAILS = 3;
  const bridges = () => window.NovaHost?.hosts() || [window.ksu, window.magisk, window.apatch, window.KSU, window.mmrl].filter(Boolean);
  const bridge = () => bridges().find(api => typeof api.exec === "function") || bridges()[0];
  const metadataBridge = () => bridges().find(api => typeof api.getPackagesInfo === "function") || bridge();
  /* ================= AppInfoProvider（统一应用信息层 · 规格化） =================
     单一入口 ensureAppInfo(packages)，内部双通道互补：
     1) 宿主桥 getPackagesInfo（真实存在才调用）：只采信应用名；
     2) 模块 meta 通道（novasched meta，经 app_process 直读 PackageManager）：
        label + 96px 真实图标 + system 标记 + enabled 启用状态，按包名严格
        键取，设备侧 1h TTL 缓存，失败有界重试。
     应用信息一律来自系统 PackageManager，不写死、不按包名猜测；名称与
     图标仅用于显示，调度规则仍以 packageName 为唯一标识。 */
  function ensureAppInfo(packages) {
    loadLabels(packages);
    ensureMeta(packages);
  }

  function loadLabels(packages) {
    const api = metadataBridge();
    if (typeof api?.getPackagesInfo !== "function") return;
    const needed = [...new Set(packages.map(basePackage))]
      .filter(pkg => validPackage(pkg) && !queried.has(pkg)
        && (labelFails.get(pkg) || 0) < LABEL_MAX_FAILS);
    if (!needed.length) return;
    for (let index = 0; index < needed.length; index += 48) {
      const batch = needed.slice(index, index + 48);
      // 同步/异步返回都兼容；批量请求限时 3 秒，避免慢桥挂住渲染。
      Promise.race([
        Promise.resolve().then(() => api.getPackagesInfo(JSON.stringify(batch))),
        new Promise((_, reject) => setTimeout(() => reject(new Error("应用名查询超时")), 3000)),
      ])
        .then(response => {
          const result = typeof response === "string" ? JSON.parse(response) : response;
          const list = Array.isArray(result) ? result
            : Array.isArray(result?.packagesInfo) ? result.packagesInfo : null;
          if (!list) throw new Error("宿主返回格式未知");
          const answered = new Set();
          for (const info of list) {
            if (!info || !batch.includes(info.packageName)) continue;
            answered.add(info.packageName);
            if (typeof info.appLabel === "string" && info.appLabel.trim()) {
              // 只采信宿主桥的应用名；图标一律走模块 meta 通道——宿主桥的
              // 图标与包名的配对在各管理器实现间不一致（曾出现图标映射
              // 错位），本模块通道按包名严格键取，杜绝该类错位。
              labels.set(info.packageName, {
                packageName: info.packageName,
                appLabel: info.appLabel.trim().slice(0, 300),
                icon: "",
              });
            }
          }
          answered.forEach(pkg => { queried.add(pkg); labelFails.delete(pkg); });
          if (answered.size) render();
        })
        .catch(() => {
          batch.forEach(pkg => labelFails.set(pkg, (labelFails.get(pkg) || 0) + 1));
        });
    }
  }

  /* ---------------- 应用名与图标（模块 meta 通道，权威来源） ----------------
     `novasched meta <包名>…`：经 app_process 读取 PackageManager，返回按
     包名严格键取的 {label, icon(base64 PNG), system}，守护侧 1 小时 TTL
     缓存。宿主桥缺失、应用名查询失败、或需要图标时都走这里；批内请求
     失败按包计数有界重试。 */
  const META_MAX_FAILS = 3;
  const META_BATCH = 6;
  const metaFails = new Map();
  /* 故障诊断（规格五）：区分 Root 桥缺失 / CLI 失败 / JSON 解析失败 /
     DEX·PackageManager 失败（后端已分层透传原因），并记录批次与
     名称/图标命中数——不再把所有故障吞成静默占位。不含敏感信息。 */
  const metaDiag = { lastError: "", batches: 0, okBatches: 0, returned: 0, labels: 0, icons: 0 };
  let metaQueue = Promise.resolve();
  function ensureMeta(packages) {
    if (typeof bridge()?.exec !== "function") {
      metaDiag.lastError = "Root 命令接口不可用（宿主未注入 exec 桥）";
      return;
    }
    const needed = [...new Set(packages.map(basePackage))]
      .filter(pkg => validPackage(pkg)
        && (!(labels.get(pkg)?.appLabel) || !(labels.get(pkg)?.icon))
        && (metaFails.get(pkg) || 0) < META_MAX_FAILS);
    if (!needed.length) return;
    /* 真机修正：48 个/批的图标 base64 输出可达数百 KB，撞 app_process
       512KB 输出上限后整批失败、重试耗尽——规则页因此只见包名（个别
       名称来自宿主桥的零星应答）。改为每批 6 个、串行渐进：单批输出
       数十 KB，任何宿主桥都放得下；名称与图标随批次逐行点亮。 */
    for (let index = 0; index < needed.length; index += META_BATCH) {
      const batch = needed.slice(index, index + META_BATCH);
      const command = moduleCommand(`meta ${batch.map(shellQuote).join(" ")}`);
      metaQueue = metaQueue.then(() => rootExec(command, 20000, false)).then(result => {
        if (result.errno !== 0) throw new Error(result.stderr || "meta 查询失败");
        let parsed;
        try { parsed = JSON.parse(result.stdout || "{}"); } catch (_) { throw new Error("meta 输出无法解析"); }
        if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) throw new Error("meta 输出格式未知");
        metaDiag.batches += 1;
        metaDiag.okBatches += 1;
        metaDiag.returned += Object.keys(parsed).length;
        let changed = 0;
        let gotLabels = 0;
        let gotIcons = 0;
        for (const pkg of batch) {
          // 严格按键取条目：返回 JSON 的键就是包名，与顺序无关——图标
          // 不可能配到别的应用头上（图标错位的结构性根治）。
          const entry = parsed[pkg];
          if (!entry || typeof entry !== "object" || Array.isArray(entry)) continue;
          const label = typeof entry.label === "string" ? entry.label.trim().slice(0, 300) : "";
          const icon = typeof entry.icon === "string" && entry.icon ? entry.icon : "";
          const enabled = typeof entry.enabled === "boolean" ? entry.enabled : null;
          if (!label && !icon) continue;
          if (label) gotLabels += 1;
          if (icon) gotIcons += 1;
          const existing = labels.get(pkg) || {};
          labels.set(pkg, {
            packageName: pkg,
            appLabel: label || existing.appLabel || "",
            icon: icon || existing.icon || "",
            enabled: enabled !== null ? enabled : (typeof existing.enabled === "boolean" ? existing.enabled : null),
          });
          metaFails.delete(pkg);
          changed++;
        }
        metaDiag.labels += gotLabels;
        metaDiag.icons += gotIcons;
        metaDiag.lastError = "";
        if (changed) render();
      }).catch(error => {
        metaDiag.batches += 1;
        metaDiag.lastError = String(error?.message || error).slice(0, 140);
        batch.forEach(pkg => metaFails.set(pkg, (metaFails.get(pkg) || 0) + 1));
      });
    }
  }

  /* ---------------- 连接协议（与守护进程契约一致） ---------------- */
  function parseStatus(text) {
    const result = {};
    for (const part of String(text).split(",")) { const i = part.indexOf(":"); if (i > 0) result[part.slice(0,i)] = part.slice(i+1); }
    return result;
  }
  function goodStatus(data) {
    const age = Number(data.heartbeatMs);
    return own(MODES, data.mode) && ["ready","degraded","suspended"].includes(data.phase)
      && data.heartbeatMs !== "" && Number.isFinite(age) && age >= 0;
  }
  function stopSocket(kind) {
    const slot = s.sockets[kind];
    if (!slot) return;
    delete s.sockets[kind]; clearTimeout(slot.deadline); clearTimeout(slot.retry);
    try { slot.ws.onopen = slot.ws.onmessage = slot.ws.onclose = slot.ws.onerror = null; slot.ws.close(); } catch (_) {}
  }
  function cancelProbes() {
    for (const probe of s.probes) { clearTimeout(probe.timer); probe.ws.onmessage = probe.ws.onclose = probe.ws.onerror = null; try { probe.ws.close(); } catch (_) {} }
    s.probes.clear();
  }
  function cancelAuthentication() {
    const request = authRequest; authRequest = null;
    if (request) { request.completed = true; clearTimeout(request.timer); delete window[request.name]; }
    sessionToken = "";
  }
  function normalizeReply(args) {
    const code = value => typeof value === "number" && Number.isInteger(value)
      || typeof value === "string" && /^-?\d+$/.test(value);
    if (args.length === 1) {
      let value = args[0];
      if (typeof value === "string") {
        try { const object = JSON.parse(value); if (object && typeof object === "object" && (own(object,"stdout") || own(object,"exitCode") || own(object,"errno"))) value = object; } catch (_) {}
      }
      if (value && typeof value === "object") {
        const exit = value.errno ?? value.exitCode ?? value.exit_code ?? value.code ?? 0;
        return [code(exit) ? Number(exit) : 1, String(value.stdout ?? value.output ?? ""), String(value.stderr ?? value.error ?? "")];
      }
      return [0, String(value ?? ""), ""];
    }
    if (code(args[0])) return [Number(args[0]),String(args[1] ?? ""),String(args[2] ?? "")];
    if (code(args[2])) return [Number(args[2]),String(args[0] ?? ""),String(args[1] ?? "")];
    if (code(args[1])) return [Number(args[1]),String(args[0] ?? ""),String(args[2] ?? "")];
    return [0,String(args[0] ?? ""),String(args[1] ?? "")];
  }
  function rootRead(command, callback, finish, negotiate = true) {
    const api = bridge();
    if (typeof api?.exec !== "function") throw new Error("当前宿主没有提供 root 命令接口");
    if (api !== bridgeApi) { bridgeApi = api; bridgeSignature = 0; }
    const received = (...args) => finish(...normalizeReply(args));
    window[callback] = received;
    const first = bridgeSignature || (api.exec.length === 1 ? 1 : api.exec.length === 2 ? 2 : 3);
    for (let count = first; count >= 1; count--) {
      try {
        const result = count === 3 ? api.exec(command, "{}", callback)
          : count === 2 ? api.exec(command, callback) : api.exec(command + " 2>&1");
        bridgeSignature = count;
        const receive = value => {
          if (typeof value === "string" && (count === 1 || value.trim()) || value && typeof value === "object") received(value);
        };
        if (result && typeof result.then === "function") result.then(receive, () => finish(1, "", "宿主执行失败，请检查 root 授权"));
        else receive(result);
        return;
      } catch (error) {
        if (!negotiate || count === 1 || !/argument|overload|signature|parameter|method.*(?:not found|invoked)/i.test(String(error?.message || error))) throw error;
      }
    }
  }
  function shellQuote(value) { return "'" + String(value).replace(/'/g, "'\\''") + "'"; }
  function moduleCommand(command, suffix = "") {
    const candidates = [...new Set([MODULE, ...MODULE_CANDIDATES])].map(shellQuote).join(" ");
    return `for NOVA_PATH in ${candidates}; do if [ -x "$NOVA_PATH/bin/novasched" ] && grep -q '^id=${MODULE_ID}$' "$NOVA_PATH/module.prop"; then exec "$NOVA_PATH/bin/novasched" ${command} --module-dir "$NOVA_PATH" ${suffix}; fi; done; printf '%s\\n' 'novasched: 未找到本模块的可执行文件'; exit 1`;
  }
  function publicError(value) {
    return String(value || "").replace(/[a-f0-9]{64}/gi, "[凭据已隐藏]")
      .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f]/g, " ").trim().slice(0, 900);
  }
  function extractCredentials(text) {
    const raw = String(text || "").trim();
    let parsed;
    try { parsed = JSON.parse(raw); } catch (_) {
      const match = raw.match(/\{[^{}]*"port"\s*:\s*\d+[^{}]*"token"\s*:\s*"[a-f0-9]{64}"[^{}]*"origin"\s*:\s*"[^"]*"[^{}]*\}/);
      try { parsed = match ? JSON.parse(match[0]) : null; } catch (_) { parsed = null; }
    }
    return parsed && typeof parsed === "object" ? parsed : null;
  }
  function usableCredentials(value, origin) {
    return !!value && Number.isInteger(value.port) && value.port >= 1024 && value.port <= 65535
      && typeof value.token === "string" && /^[a-f0-9]{64}$/.test(value.token) && value.origin === origin;
  }
  function isAllowedOrigin(value) {
    try {
      const url = new URL(value);
      if (url.origin !== value || url.username || url.password || url.pathname !== "/" || url.search || url.hash) return false;
      if (url.protocol === "https:") return true;
      if (url.protocol !== "http:") return false;
      const host = url.hostname.toLowerCase().replace(/^\[|\]$/g, "");
      return host === "localhost" || host.endsWith(".localhost") || host === "::1" || /^127(?:\.\d{1,3}){3}$/.test(host);
    } catch (_) { return false; }
  }
  function loadAuthentication(epoch, ready) {
    const api = bridge();
    const failed = (message, retry = true) => {
      if (epoch !== s.epoch || s.suspended) return;
      sessionToken = ""; s.connectionError = message;
      s.online = false; s.connecting = false; render();
      clearTimeout(s.retry); if (retry) s.retry = setTimeout(discover, 5000);
    };
    let origin = "";
    try { origin = new URL(location.href).origin; } catch (_) {}
    s.rootOrigin = origin;
    if (origin === "null" && /^(file|content):/.test(location.href) && typeof api?.exec === "function") {
      startRootBridge(); return;
    }
    if (!isAllowedOrigin(origin)) {
      failed("当前页面来源不安全。请使用管理器本地 WebUI，或使用 HTTPS 页面。", false); return;
    }
    if (typeof api?.exec !== "function") {
      failed("等待宿主开放 root 命令接口。请允许 root／Shell 权限；Magisk 或 Alpha 可用 KsuWebUI、MMRL 打开本模块。"); return;
    }
    const name = `nova_auth_${Date.now()}_${++bridgeCounter}`;
    const request = { name, timer:0, completed:false }; authRequest = request;
    const finish = (errno, stdout, stderr) => {
      if (request.completed) return;
      request.completed = true; clearTimeout(request.timer); delete window[name];
      if (authRequest === request) authRequest = null;
      if (epoch !== s.epoch || s.suspended) return;
      const credentials = extractCredentials(stdout);
      if (credentials && !usableCredentials(credentials, origin)) {
        failed("连接凭据格式或页面来源不匹配，请完整安装同一版本的模块后重启。"); return;
      }
      if (!credentials && Number(errno) !== 0) {
        failed("无法读取守护连接状态：" + (publicError(stderr) || publicError(stdout) || `命令退出码 ${errno}，请检查宿主 root 授权并读取启动诊断。`)); return;
      }
      if (!credentials) {
        const detail = /^novasched:|Permission denied|not found/.test(String(stdout || "")) ? publicError(stdout) : "宿主返回格式无效，请读取启动诊断。";
        failed("未取得有效连接凭据：" + detail); return;
      }
      if (typeof credentials.moduleDir === "string" && /^\/data\/adb\/(?:modules|ap\/modules)\/NovaSched_Zen_Edition$/.test(credentials.moduleDir)) MODULE = credentials.moduleDir;
      sessionToken = credentials.token;
      s.connectionError = ""; ready(credentials.port);
    };
    request.timer = setTimeout(() => finish(1, "", "等待宿主响应超过 30 秒，请完成 root 授权后重新连接。"), 30000);
    window[name] = finish;
    try { rootRead(moduleCommand("webui-session", `--origin ${shellQuote(origin)}`), name, finish); }
    catch (_) { finish(1, "", "宿主命令接口执行失败，请检查 root／Shell 授权。"); }
  }
  function cancelRootBridge() {
    clearTimeout(s.rootTimer); s.rootTimer = 0; s.rootJob = null; s.rootGeneration++;
  }
  function rootExec(command, timeout = 30000, negotiate = true) {
    return new Promise((resolve, reject) => {
      const api = bridge();
      if (typeof api?.exec !== "function") return reject(new Error("当前管理器没有提供命令接口"));
      const name = `nova_exec_${Date.now()}_${++bridgeCounter}`;
      let completed = false;
      const finish = (err, result) => { if (completed) return; completed = true; clearTimeout(timer); delete window[name]; err ? reject(err) : resolve(result); };
      const timer = setTimeout(() => finish(new Error("宿主命令未在限定时间内完成")), timeout);
      window[name] = (errno, stdout, stderr) => finish(null, { errno:Number(errno), stdout:String(stdout || ""), stderr:String(stderr || "") });
      try { rootRead(command, name, window[name], negotiate); } catch (err) { finish(err); }
    });
  }
  function rootRequest(request) {
    const command = moduleCommand("webui-rpc", `--origin ${shellQuote(s.rootOrigin)} --request ${shellQuote(JSON.stringify(request))}`);
    return rootExec(command, 30000, !own(request, "message")).then(result => {
      let reply;
      try { reply = JSON.parse(result.stdout); } catch (_) {
        if (result.errno !== 0) throw new Error(publicError(result.stderr) || publicError(result.stdout) || `宿主返回退出码 ${result.errno}`);
        throw new Error("宿主备用通道未返回有效状态，请完整安装同一版本后重启。");
      }
      if (!reply || typeof reply !== "object" || typeof reply.modes !== "string" || typeof reply.apps !== "string"
          || typeof reply.logs !== "string" || typeof reply.cursor !== "string") throw new Error("备用通道状态格式无效");
      return reply;
    });
  }
  function receiveRootSnapshot(reply,withLogs=false){
    if (reply.modes) {
      const status = parseStatus(reply.modes);
      if (!goodStatus(status)) throw new Error("守护心跳或状态未就绪");
      if (Number(status.heartbeatMs) >= 30000) throw new Error("守护心跳已过期");
      s.discovered = true;
      receiveModes(status);
      modeNote("");
    }
    if (reply.apps) receiveApps(reply.apps);
    if(withLogs&&logsActive()){
      if(reply.logsReset) { s.logTail = ""; if (!s.logCursor) s.logs = []; }
      s.logCursor = reply.cursor;
      s.logConnected = true; s.logConnecting = false;
      if (reply.logs) receiveLogs(reply.logs); else renderLogs();
    }
  }
  function pollRootBridge(epoch) {
    if (epoch !== s.epoch || s.suspended || s.transport !== "root" || s.rootJob) return;
    const job={logs:logsActive()},generation=s.rootGeneration;s.rootJob=job;
    rootRequest({endpoint:"snapshot",logs:job.logs,cursor:s.logCursor}).then(reply => {
      if (epoch !== s.epoch || s.suspended || generation !== s.rootGeneration) return;
      receiveRootSnapshot(reply,job.logs);
      // root 备用通道就绪即探测系统能力（各一次；结果缓存，失败有界重试）：
      // 触感 → 外观设置「测试触感」；图形后端 → 规则页徽标与编辑入口需要。
      probeHaptics();
      probeGraphics();
      probeFps();
    }).catch(error => {
      if (epoch !== s.epoch || s.suspended || generation !== s.rootGeneration) return;
      offline("root"); s.connectionError = "宿主备用通道失败：" + publicError(error.message); render();
    }).finally(() => {
      if (s.rootJob === job) s.rootJob = null;
      if (epoch === s.epoch && !s.suspended && s.transport === "root")
        s.rootTimer=setTimeout(()=>pollRootBridge(epoch),logsActive()&&!job.logs?0:2000);
    });
  }
  function startRootBridge() {
    const epoch = ++s.epoch;
    clearTimeout(s.retry); cancelProbes(); cancelAuthentication(); cancelRootBridge();
    ["modes","apps","logs"].forEach(stopSocket);
    s.transport = "root"; s.online = false; s.connecting = true; s.discovered = false;
    s.connectionError = ""; s.logConnected = false; s.logConnecting = logsActive();
    render(); pollRootBridge(epoch);
  }
  function offline(reason, retry = true) {
    if (retry && s.transport !== "root" && !s.suspended && typeof bridge()?.exec === "function"
        && (reason === "handshake" || reason === "connection")) { startRootBridge(); return; }
    s.epoch++; cancelRootBridge(); s.transport = "";
    cancelAuthentication();
    s.online = false; s.connecting = false; s.discovered = false;
    s.connectionError = reason === "handshake"
      ? "已取得守护凭据，但 WebSocket 未连通。请读取启动诊断，并检查宿主是否允许本地连接。"
      : "本地连接已中断，请重新连接；若持续失败，请读取启动诊断。";
    stopSocket("modes"); stopSocket("apps"); stopSocket("logs"); s.logConnected = false; s.logConnecting = false;
    if (s.pending) finishPending(false, "连接中断，请重新连接后确认结果");
    render(); renderLogs();
    if (retry && !s.suspended) { clearTimeout(s.retry); s.retry = setTimeout(discover, 4000); }
  }
  function discover() {
    if (s.suspended) return;
    const epoch = ++s.epoch; clearTimeout(s.retry); cancelProbes(); cancelAuthentication();
    cancelRootBridge(); s.transport = "websocket";
    ["modes","apps","logs"].forEach(stopSocket);
    s.online = false; s.connecting = true; s.discovered = false; render();
    loadAuthentication(epoch, port => {
      const targets = [{ host:"127.0.0.1", port }];
      let index = 0, active = 0, chosen = false;
      const launch = () => {
        if (epoch !== s.epoch || s.suspended || chosen) return;
        while (active < 3 && index < targets.length) {
          const target = targets[index++]; let ws;
          try { ws = new WebSocket(`ws://${target.host}:${target.port}/modes`, [KEY, AUTH_PREFIX + sessionToken]); }
          catch (_) { continue; }
          active++;
          const probe = { ws, timer:0 }; s.probes.add(probe);
          let ended = false;
          const fail = () => {
            if (ended) return; ended = true; active--; clearTimeout(probe.timer); s.probes.delete(probe);
            ws.onmessage = ws.onclose = ws.onerror = null; try { ws.close(); } catch (_) {}
            if (epoch === s.epoch && !chosen) {
              if (index >= targets.length && active === 0) { offline("handshake"); }
              else launch();
            }
          };
          probe.timer = setTimeout(fail, 5000);
          ws.onerror = ws.onclose = fail;
          ws.onmessage = event => {
            if (ended || chosen || epoch !== s.epoch) return;
            const status = parseStatus(event.data);
            if (!goodStatus(status)) return fail();
            ended = true; chosen = true; active--; clearTimeout(probe.timer); s.probes.delete(probe); cancelProbes();
            s.discovered = true;
            bindSocket("modes", ws); receiveModes(status);
            openEndpoint("apps"); if (logsActive()) openEndpoint("logs");
          };
        }
        if (!active && index >= targets.length && !chosen) { offline("handshake"); }
      };
      launch();
    });
  }
  function bindSocket(kind, ws) {
    stopSocket(kind);
    const slot = {ws,deadline:0,retry:0}; s.sockets[kind] = slot;
    const current = () => s.sockets[kind] === slot && !s.suspended;
    const fail = () => {
      if (!current()) return;
      if (kind === "modes") return offline("connection");
      stopSocket(kind);
      if (kind === "logs") { s.logConnected = false; s.logConnecting = false; renderLogs(); }
      if (kind === "apps" && (["rule", "delete"].includes(s.pending?.kind))) finishPending(false, "应用规则连接中断，请重试");
      if (s.discovered && !s.suspended) {
        const epoch = s.epoch;
        const retrySlot = { ws, deadline:0, retry:setTimeout(() => { if (epoch === s.epoch && !s.suspended) openEndpoint(kind); }, 2200) };
        s.sockets[kind] = retrySlot;
      }
    };
    const arm = () => { clearTimeout(slot.deadline); if (kind !== "logs") slot.deadline = setTimeout(fail, 33000); };
    ws.onopen = () => { if (!current()) return; if (kind === "logs") { s.logConnected = true; s.logConnecting = false; renderLogs(); } arm(); };
    ws.onmessage = event => {
      if (!current()) return;
      if (kind === "modes") { const parsed = parseStatus(event.data); if (goodStatus(parsed)) receiveModes(parsed); }
      else if (kind === "apps") receiveApps(event.data);
      else receiveLogs(event.data);
      arm();
    };
    ws.onerror = ws.onclose = fail;
    if (ws.readyState === WebSocket.OPEN) arm();
    else slot.deadline = setTimeout(fail,8000);
  }
  function openEndpoint(kind) {
    if (s.transport === "root") {
      if (!s.rootJob) { clearTimeout(s.rootTimer); pollRootBridge(s.epoch); }
      return;
    }
    if (!s.discovered || !sessionToken || s.suspended || (kind === "logs" && !logsActive())) return;
    if (s.sockets[kind]?.ws?.readyState === WebSocket.OPEN || s.sockets[kind]?.ws?.readyState === WebSocket.CONNECTING) return;
    stopSocket(kind);
    const path = kind === "apps" ? "/app-modes" : "/logs";
    if (kind === "logs") { s.logConnecting = true; renderLogs(); }
    try { bindSocket(kind, new WebSocket(`ws://${s.host || "127.0.0.1"}:${s.port || 31415}${path}`, [KEY, AUTH_PREFIX + sessionToken])); }
    catch (_) { if (kind === "logs") { s.logConnecting = false; renderLogs(); } }
  }
  let legacyFasAt = 0, legacyFasJob = null;
  function probeLegacyFas() {
    if (s.suspended || document.hidden || legacyFasJob || performance.now() < legacyFasAt || typeof bridge()?.exec !== "function") return;
    legacyFasAt = performance.now() + 10000;
    legacyFasJob = rootExec("if [ -e /dev/fas_rs/mode ]; then cat /dev/fas_rs/mode; else printf '%s\\n' '__NOVA_FAS_ABSENT__'; fi", 5000)
      .then(reply => {
        if (!s.fasLegacy) return;
        const mode = reply.stdout.trim();
        s.fasState = reply.errno !== 0 ? "unreadable" : mode === "__NOVA_FAS_ABSENT__" ? "absent" : own(MODES, mode) ? "ready" : "invalid";
        s.fasMode = own(MODES, mode) ? mode : "";
        s.fasAvailable = s.fasState === "ready";
        s.fasSync = "unavailable";
        render();
      }).catch(() => { if (s.fasLegacy) { s.fasState = "unknown"; render(); } })
      .finally(() => { legacyFasJob = null; });
  }
  function receiveModes(data) {
    s.connectionError = "";
    s.mode = data.mode; s.effective = own(MODES, data.effective) ? data.effective : "";
    s.pkg = data.package || ""; s.controller = data.controller || "NovaSched";
    s.external = data.sceneActive === "true"; s.linked = data.sceneLinked === "true";
    s.sceneDriving = data.sceneDriving === "true";
    const incomingCmdError = data.cmdError || "";
    if (incomingCmdError && incomingCmdError !== s.cmdError) toast(incomingCmdError, 4300);
    s.cmdError = incomingCmdError;
    s.pendingControls = /^\d+$/.test(data.pendingControls || "") ? Number(data.pendingControls) : 0;
    s.samsungSafe = data.samsungSafe === "true";
    s.phase = data.phase; s.heartbeat = Number(data.heartbeatMs);
    s.online = s.heartbeat < 30000; s.connecting = false;
    s.soc = data.socName || ""; s.socId = data.socId || ""; s.configProfile = data.configProfile || "";
    if (typeof data.version === "string" && /^v?\d+\.\d+\.\d+(?:[-+][\w.-]+)?$/.test(data.version)) s.version = data.version.replace(/^v/, "");
    if (["ready", "running", "unreadable", "invalid", "absent", "unknown"].includes(data.fasState)) {
      s.fasLegacy = false;
      s.fasState = data.fasState;
      s.fasAvailable = data.fasAvailable === "true";
      s.fasMode = own(MODES, data.fasMode) ? data.fasMode : "";
      s.fasSync = ["synced", "pending", "failed", "paused", "unavailable"].includes(data.fasSync) ? data.fasSync : "unavailable";
    } else {
      s.fasLegacy = true;
      if (s.fasState === "unknown") s.fasAvailable = data.fasAvailable === "true";
      probeLegacyFas();
    }
    s.sceneInstalled = data.sceneInstalled === "true";
    s.scenePresence = ["available", "absent", "unknown"].includes(data.scenePresence) ? data.scenePresence : s.sceneInstalled ? "available" : "unknown";
    if (["off", "idle", "active", "degraded", "paused"].includes(data.fasEngine)) {
      s.fasEngineState = data.fasEngine;
      s.fasEngineSource = ["timestats", "heuristic", "none"].includes(data.fasEngineSrc) ? data.fasEngineSrc : "none";
      s.fasEngineReason = data.fasReason || "";
      s.fasFps = Number(data.fasFps) || 0;
      s.fasGpu = Number(data.fasGpu) || 0;
        s.fasTarget = Number(data.fasTarget) || 0;
      s.fasBoost = /^\d+$/.test(data.fasBoost || "") ? Number(data.fasBoost) : 0;
      s.fasOverride = data.fasOverride === "true";
      s.fasTick = /^\d+$/.test(data.fasTick || "") ? Number(data.fasTick) : 0;
    }
    s.freqs = String(data.freqs || "").split(";").map(Number).filter(value => Number.isFinite(value) && value > 0);
    s.extremePower = data.extremePower === "true";
    s.extremeStatus = ["off", "applying", "active", "partial", "unsupported", "failed"].includes(data.extremeStatus) ? data.extremeStatus : "off";
    s.extremeError = data.extremeError || "";
    if ((s.extremePending === s.extremePower && s.extremeStatus !== "applying") || s.extremeStatus === "failed") s.extremePending = null;
    s.automatic = data.automaticEnabled === "true";
    s.automaticActive = data.automaticActive === "true";
    s.appKind = ["game", "daily", "unknown"].includes(data.appKind) ? data.appKind : "";
    s.classificationSource = data.classificationSource || "";
    if (Number(data.port) >= 1024) { s.port = Number(data.port); s.host = "127.0.0.1"; }
    ensureAppInfo([s.pkg]); confirmPending(); render();
    if (!welcomeShown && s.online) { welcomeShown = true; setTimeout(maybeShowWelcome, 800); }
  }
  function receiveApps(text) {
    try {
      const data = JSON.parse(text);
      if (data.type !== "app-modes" || !Array.isArray(data.rules)) return;
      s.rules = data.rules.filter(r => r && validPackage(r.package) && own(MODES, r.mode));
      s.rulesLoaded = true;
      if (data.classificationOverrides && typeof data.classificationOverrides === "object" && !Array.isArray(data.classificationOverrides)) {
        s.overrides = Object.fromEntries(Object.entries(data.classificationOverrides).filter(([pkg, kind]) => validPackage(pkg) && !pkg.includes(":") && ["game", "daily"].includes(kind)));
      }
      s.error = typeof data.error === "string" ? data.error : "";
      if (typeof data.version === "string" && /^v?\d+\.\d+\.\d+(?:[-+][\w.-]+)?$/.test(data.version)) s.version = data.version.replace(/^v/, "");
      if (own(MODES, data.defaultMode)) s.mode = data.defaultMode;
      ensureAppInfo([s.pkg, ...s.rules.map(r => r.package), ...Object.keys(s.overrides)]);
      confirmPending(); render(); settleClear();
    } catch (_) { toast("应用规则数据无法解析，请重连后再试"); }
  }
  function parseLog(line) {
    const match = line.match(/^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2})\s+(信息|错误|警告|调试|INFO|ERROR|WARN|DEBUG)\s*(?:->|→|:)?\s*(.*)$/);
    const level = match ? /错误|ERROR/.test(match[2]) ? "error" : /警告|WARN/.test(match[2]) ? "warn" : "info" : /\bERROR\b|错误\s*->/.test(line) ? "error" : /\bWARN\b|警告\s*->/.test(line) ? "warn" : "info";
    return { time:match?.[1] || "", level, message:match?.[3] ?? line, raw:line };
  }
  function receiveLogs(data) {
    s.logReceived = true;
    const chunk = s.logTail + String(data);
    const lines = chunk.split(/\r?\n/); s.logTail = lines.pop() || "";
    for (const line of lines) if (line.trim()) s.logs.push(parseLog(line));
    if (s.logTail.length > 12000) { s.logs.push(parseLog(s.logTail.slice(0,12000))); s.logTail = ""; }
    if (s.logs.length > 1200) s.logs.splice(0, s.logs.length - 1000);
    renderLogs();
  }
  const visibleLogs = () => s.logs.filter(line => s.level === "all" || line.level === s.level);
  function send(kind, message) {
    if (!editable()) { toast(s.external ? "Scene 外部调度器接管中，请先在 Scene 停用它的调度" : "请先连接本地守护进程"); return false; }
    if (s.transport === "root") {
      const epoch = s.epoch, generation = ++s.rootGeneration;
      clearTimeout(s.rootTimer);
      rootRequest({endpoint:kind === "apps" ? "apps" : "modes", message}).then(reply => {
        if (epoch === s.epoch && generation === s.rootGeneration && !s.suspended) receiveRootSnapshot(reply);
      }).catch(error => {
        if (epoch === s.epoch && generation === s.rootGeneration) {
          if (message.startsWith("extreme\t")) {
            s.extremePending = null;
            s.extremeStatus = "failed";
            s.extremeError = "设置未保存：" + publicError(error.message);
            render();
          } else finishPending(false, "操作未确认：" + publicError(error.message));
        }
      }).finally(() => {
        if (epoch === s.epoch && !s.suspended) { clearTimeout(s.rootTimer); s.rootTimer = setTimeout(() => pollRootBridge(epoch), 100); }
      });
      return true;
    }
    const ws = s.sockets[kind]?.ws;
    if (!ws || ws.readyState !== WebSocket.OPEN) { toast("控制通道尚未连接，请稍后重试"); return false; }
    try { ws.send(message); return true; }
    catch (_) { toast("发送失败，请重连后确认状态"); return false; }
  }
  function beginPending(kind, value, pkg = "") {
    if (s.pending || !["mode", "rule", "delete", "auto"].includes(kind)) return false;
    if (kind === "auto" && s.linked) { toast("Scene 正在控制档位，请在 Scene 中调整"); return false; }
    const endpoint = ["rule", "delete"].includes(kind) ? "apps" : "modes";
    const message = kind === "mode" ? value : kind === "auto" ? `auto\t${value}` : kind === "rule" ? `set\t${pkg}\t${value}` : `delete\t${pkg}`;
    s.pending = { kind, value, pkg, dialogEpoch };
    if (!send(endpoint, message)) { s.pending = null; render(); return false; }
    if (kind === "mode") modeNote(`正在请求${MODES[value].name}档，等待守护进程反馈…`);
    if (kind === "auto") modeNote("正在切换自动调度，等待守护进程确认…");
    pendingTimer = setTimeout(() => {
      if (!s.pending) return;
      const p = s.pending;
      finishPending(false, p.kind === "mode" && s.mode === p.value ? "默认档位已保存，但尚未确认实际生效；请查看概览或诊断" : "未收到确认，状态未标记为成功，请重连后检查");
    }, s.transport === "root" ? 30000 : 10000);
    render(); return true;
  }
  function confirmPending() {
    const p = s.pending;
    if (!p) return;
    if (s.external) return finishPending(false, "Scene 外部调度器接管中，档位请以 Scene 为准");
    if (!s.online) return;
    if (p.kind === "auto") {
      if (s.automatic === (p.value === "on") && s.automaticActive === s.automatic && s.phase === "ready" && (p.value === "off" || s.effective === (s.appKind === "game" ? "fast" : "powersave"))) return finishPending(true, p.value === "on" ? "自动调度已开启" : "已返回手动调度");
    } else if (p.kind === "mode") {
      if (s.automatic || s.automaticActive) {
        // 自动调度开启时手动切档由守护进程临时保持（前台应用变化或
        // 约 10 分钟后恢复自动）；广播回显保持档位即可确认成功。
        if (s.phase === "ready" && s.mode === p.value && s.effective === p.value) return finishPending(true, `${MODES[p.value].name}档已生效；自动调度保持中，前台应用变化或约 10 分钟后恢复自动`);
        return;
      }
      if (s.phase === "degraded" && s.pendingControls === 0 && s.mode === p.value) return finishPending(false, s.error || "策略下发失败，已保留最后成功档位");
      if (s.phase === "ready" && s.mode === p.value && s.effective === p.value) return finishPending(true, `${MODES[p.value].name}档已生效`);
      const rule = s.rules.find(r => r.package === basePackage(s.pkg));
      if (s.mode === p.value && rule && rule.mode === s.effective && s.effective !== p.value && s.phase === "ready") return finishPending(true, `${MODES[p.value].name}已设为默认；当前应用规则优先，仍使用${MODES[s.effective].name}档`);
    } else if (p.kind === "rule" && s.rules.some(r => r.package === p.pkg && r.mode === p.value)) finishPending(true, "应用规则已保存");
    else if (p.kind === "delete" && !s.rules.some(r => r.package === p.pkg)) finishPending(true, "应用规则已删除，今后沿用默认档位");
  }
  function finishPending(success, message) {
    const p = s.pending; clearTimeout(pendingTimer); pendingTimer = 0; s.pending = null;
    if (success) NovaHaptics.success(); else NovaHaptics.failure();
    if (p?.kind === "mode" || p?.kind === "auto") modeNote(message, !success);
    if(success&&["rule","delete"].includes(p?.kind)&&p.dialogEpoch===dialogEpoch)closeDialog();
    if(!success&&["rule","delete"].includes(p?.kind)&&p.dialogEpoch===dialogEpoch){
      const err = $("#dialog-error");
      if (err && $("#dialog-layer").classList.contains("open")) { err.hidden = false; err.textContent = message; }
    }
    toast(message, 4300); render();
  }

  /* ---------------- 视图渲染 ---------------- */
  function stateLabel() {
    return !s.online ? s.connecting ? "连接中" : "未连接" : s.phase === "degraded" ? "需检查" : s.external ? "外部接管" : "调度正常";
  }
  function sourceLabel() {
    if (!s.online) return "等待本地服务";
    if (s.external) return "Scene 外部调度器";
    if (s.linked) return "Scene 联动";
    // 档位来源解释：回答“当前由谁决定档位”。
    if (s.automatic || s.automaticActive) {
      return `自动调度（当前分类：${s.appKind === "game" ? "游戏" : "日常"}）`;
    }
    const rule = s.rules.find(r => r.package === basePackage(s.pkg));
    if (rule) return `应用规则（${MODES[rule.mode]?.name || rule.mode}）`;
    return "手动档位";
  }
  function connectionSummary() {
    const message = s.connectionError.replace(/\s+/g, " ");
    return message.length > 220 ? message.slice(0, 220) + "… 点状态卡查看详情。" : message;
  }
  /* 渲染只写发生变化的文本/属性：心跳每两秒触发一次，也不产生无谓的 DOM 变更 */
  function render() {
    document.documentElement.dataset.online = s.online ? "true" : "false";

    /* 顶栏状态胶囊：点击=诊断（在线）或重连（离线） */
    const cta = $("#nav-cta");
    const ctaState = !s.online ? (s.connecting ? "connecting" : "offline") : s.phase === "degraded" ? "degraded" : s.external ? "external" : "ready";
    if (cta.dataset.state !== ctaState) cta.dataset.state = ctaState;
    present("#nav-cta-text", ctaState === "connecting" ? "连接中" : ctaState === "offline" ? "离线" : ctaState === "degraded" ? "需检查" : ctaState === "external" ? "外部接管" : "已连接");

    /* 状态卡：当前档位大字 + 来源 + 概述 + SoC/心跳 */
    present("#stat-mode", s.online ? (s.external ? "外部控制" : MODES[s.effective]?.name || "待确认") : "—");
    present("#stat-ctrl", s.online ? s.controller : "等待本地服务");
    present("#foreground-tag", !s.online ? "待连接" : s.external ? "外部接管" : s.phase === "degraded" ? "需检查" : "实时");
    present("#hero-sub", !s.online
      ? (connectionSummary() || (s.connecting ? "正在连接模块服务，首次解锁后需等待约 30 秒。" : "本地服务暂未连接，会自动重试；点状态卡可读取诊断或启动服务。"))
      : s.external ? "Scene 当前选用其他调度器，NovaSched 暂停节点写入并保持只读。"
      : s.phase === "degraded" ? "部分策略下发失败，守护进程正在重试；详情见日志。"
      : s.automatic ? "自动调度跟随前台应用：游戏用极速，其余用省电。"
      : MODES[s.effective]?.copy || "守护进程心跳正常，实际策略已生效。");
    const socChip = $("#stat-soc-chip");
    if (socChip) socChip.hidden = !(s.online && s.soc);
    present("#stat-soc-name", s.soc || "—");
    const heartbeat = s.online && Number.isFinite(s.heartbeat) ? `心跳 ${(Math.round(s.heartbeat / 100) / 10).toFixed(1)}s` : "心跳 —";
    present("#stat-heartbeat", heartbeat);
    /* 各簇实时频率：守护进程每 2 秒从 scaling_cur_freq 读取（kHz） */
    present("#live-freqs", !s.online ? "实时频率 —"
      : s.freqs.length ? "实时 " + s.freqs.map(f => (f / 1000000).toFixed(2)).join(" / ") + " GHz" : "实时频率 —");

    /* 档位网格 */
    $$("#mode-grid .mode-card").forEach(button => {
      const mode = button.dataset.mode;
      const actual = s.online && !s.external && mode === s.effective;
      const pending = s.pending?.kind === "mode" && s.pending.value === mode;
      const state = pending ? "pending" : actual ? "active" : "";
      if (button.dataset.state !== state) button.dataset.state = state;
      button.setAttribute("aria-pressed", String(actual));
      button.setAttribute("aria-busy", String(pending));
      $(".badge", button).hidden = !actual || !!(!s.linked && (s.automatic || s.automaticActive));
      button.disabled = !editable() || !!s.pending || s.sceneDriving;
    });
    /* ZenJooo · Scene 接管横幅：Scene 里切档后（novasched.scene.mode 非空）
       四档交给 Scene 主导，本页只读；「取回控制权」清掉接管存档恢复编辑。 */
    const takeover = $("#scene-takeover");
    if (takeover) {
      takeover.hidden = !(s.online && s.sceneDriving);
      const release = $("#scene-release");
      if (release) release.disabled = !!s.pending;
    }
    present("#modes-state", s.online ? (s.external ? "外部控制" : MODES[s.effective]?.name || "—") : "—");
    const autoSwitch = $("#auto-switch");
    autoSwitch.disabled = !editable() || !!s.pending || s.linked;
    autoSwitch.setAttribute("aria-checked", String(!s.linked && !!s.automatic));
    present("#auto-note", s.linked ? "Scene 决定档位；本地自动偏好保留，退出联动后恢复。" : "游戏用极速，日用和桌面用省电。");
    autoSwitch.setAttribute("aria-busy", String(s.pending?.kind === "auto"));
    const extremeSwitch = $("#extreme-switch");
    extremeSwitch.disabled = !editable() || s.extremePending !== null;
    extremeSwitch.setAttribute("aria-checked", String(s.extremePower));
    extremeSwitch.setAttribute("aria-busy", String(s.extremeStatus === "applying" || s.extremePending !== null));
    const extremeLabels = {
      off: "已关闭",
      applying: "正在应用",
      active: "已生效",
      partial: "部分策略簇支持，未支持的策略簇保持原策略",
      unsupported: s.external ? "外部调度器接管中，当前不可用" : "设备未提供可确认的 CPU 频率表，当前不支持",
      failed: s.extremeError || "策略下发失败，请查看日志",
    };
    present("#extreme-status", s.extremeError || extremeLabels[s.extremeStatus] || "等待本地服务");
    const extremeNote = $("#extreme-note");
    if (extremeNote) extremeNote.textContent = s.external
      ? "降低性能上限，延长续航。当前由 Scene 外部控制，极限省电暂不下发。"
      : "降低性能上限，延长续航。作为四档与应用规则之上的限制层；NovaFAS 不能突破此上限。";

    /* NovaFAS 引擎卡：引擎状态与 fas-rs 接口状态分开呈现 */
    $("#samsung-compat").hidden = !s.online || !s.samsungSafe;
    const engineRunning = s.online && ["active", "degraded"].includes(s.fasEngineState);
    present("#fas-engine-badge", !s.online ? "待连接" : engineRunning ? (s.fasEngineState === "degraded" ? "信号不可用，已回退" : s.fasOverride ? "已接管 fas-rs" : "运行中") : s.fasEngineState === "idle" ? "待命" : "未启用");
    let engineTitle, engineDesc, engineSource;
    if (!s.online) {
      engineTitle = "等待连接"; engineDesc = "连接后读取 NovaFAS 引擎状态。"; engineSource = "待连接";
    } else if (s.samsungSafe) {
      engineTitle = "三星兼容策略"; engineSource = "已暂停";
      engineDesc = "保留系统的帧调度与 uclamp，NovaFAS 不采样 SurfaceFlinger，也不写入动态底频。四档 CPU 频率策略仍可使用。";
    } else if (s.fasEngineState === "active") {
      engineTitle = ENGINE_TITLES.active;
      engineSource = s.fasEngineSource === "timestats"
        ? (s.fasTarget > 0 ? `帧信号 · 调度目标 ${s.fasTarget} FPS` : "帧信号 · SurfaceFlinger")
        : s.fasEngineSource === "heuristic"
          ? `负载启发式 · GPU ${s.fasGpu.toFixed(0)}%`
          : "无信号源 · 仅档位包络";
      engineDesc = s.fasOverride
        ? "读取前台应用渲染帧信号，在档位包络内动态调频；Scene 的 fas-rs 已被暂停，由本引擎接管。"
        : "读取前台应用渲染帧信号，在档位包络内动态调频：掉帧快速加压，稳定后缓慢回落省电。";
    } else if (s.fasEngineState === "degraded") {
      engineTitle = ENGINE_TITLES.degraded;
      engineSource = s.fasEngineSource === "heuristic" ? `帧信号缺失 · GPU ${s.fasGpu.toFixed(0)}% 启发式` : "帧与负载信号不可用";
      engineDesc = s.fasEngineReason || "采样信号缺失或已过期，NovaFAS 正在回退到当前档位策略。";
    } else if (s.fasEngineState === "idle") {
      engineTitle = ENGINE_TITLES.idle;
      engineSource = "待命";
      engineDesc = "引擎已加载，等待前台应用进入可调度状态。";
    } else if (s.fasEngineState === "paused") {
      engineTitle = ENGINE_TITLES.paused;
      engineSource = "暂停";
      engineDesc = "外部调度器接管中，NovaFAS 暂停写入。";
    } else {
      engineTitle = ENGINE_TITLES.off;
      engineSource = "未启用";
      engineDesc = "当前配置未启用 NovaFAS；安装完整模块并重启后生效。";
    }
    present("#fas-engine-title", engineTitle);
    present("#fas-engine-source", engineSource);
    present("#fas-engine-desc", engineDesc);
    const meter = $("#fas-engine-meter");
    if (meter) meter.hidden = !engineRunning;
    const frameSource = s.fasEngineSource === "timestats";
    // 采样有效但前台没有出帧（暂停/静态画面）时如实显示“待渲染”，
    // 不把无帧渲染成 0.0 fps 假数据。
    present("#fas-engine-fps", engineRunning && frameSource ? (s.fasFps >= 0.05 ? `${s.fasFps.toFixed(1)} fps` : "待渲染") : "—");
    present("#fas-engine-gpu", engineRunning ? (s.fasGpu > 0 || s.fasEngineSource === "heuristic" ? `${Math.min(100, s.fasGpu).toFixed(0)}%` : "—") : "—");
    present("#fas-engine-boost", engineRunning ? `${s.fasBoost}%` : "—");
    /* 引擎拍数每次心跳都在增长：数值稳定不动时，它是区分“稳定”与“卡死”的唯一信号 */
    present("#fas-engine-tick", engineRunning ? `第 ${s.fasTick} 拍` : "—");
    const bar = $("#fas-engine-bar");
    if (bar) bar.style.width = `${Math.min(100, s.fasBoost)}%`;
    /* 旧版服务没有引擎字段时，退回展示 fas-rs 模式接口状态 */
    const legacy = $("#fas-legacy-desc");
    if (legacy) {
      const showLegacy = s.online && !s.fasEngineState;
      legacy.hidden = !showLegacy;
      if (showLegacy) legacy.textContent = `fas-rs 接口：${{ ready: "已检测到", running: "进程运行中", absent: "未检测到", unreadable: "不可读", invalid: "待就绪" }[s.fasState] || "待确认"}${s.fasSync === "synced" ? " · 档位已同步" : ""}。这是旧版服务；完整更新模块后由 NovaFAS 引擎接管。`;
    }

    /* 前台应用分区 */
    present("#stat-appkind", s.online
      ? (s.appKind === "game" ? "识别为游戏" : s.appKind === "daily" ? "识别为日用" : "暂未识别")
      : "每两秒刷新");
    const pkgName = s.online && s.pkg ? appInfo(s.pkg).name : s.online ? "无前台应用" : "等待连接";
    present("#stat-pkg", pkgName);
    present("#stat-pkg-raw", s.online && s.pkg ? s.pkg : s.online ? "锁屏、待机或暂未读取到活动" : "连接后显示应用包名");
    /* 前台头像：模块 meta 通道的图标就绪时显示真实图标，否则统一占位图形
       （不使用首字母/随机色）。 */
    const avatar = $("#foreground-avatar");
    if (avatar) {
      const info = s.online && s.pkg ? appInfo(s.pkg) : null;
      const avatarHtml = !info ? "—"
        : info.icon ? `<img src="data:image/png;base64,${info.icon}" alt="" aria-hidden="true">`
        : APP_PLACEHOLDER;
      if (avatar.dataset.key !== avatarHtml) {
        avatar.dataset.key = avatarHtml;
        avatar.innerHTML = avatarHtml;
      }
    }
    present("#stat-pkg-note", s.online && s.pkg
      ? (s.appKind === "game" ? "识别为游戏 · 自动模式使用极速" : s.appKind === "daily" ? "识别为日用 · 自动模式使用省电" : "暂未识别，自动模式先用省电")
      : "前台活动每两秒刷新一次。");

    /* Scene 联动四态 */
    let sceneState, sceneNote;
    if (!s.online) {
      sceneState = "等待连接"; sceneNote = "连接守护进程后显示 Scene 的接管状态。";
    } else if (s.external) {
      sceneState = "Scene 接管未完成";
      sceneNote = "Scene 入口仍是其它调度脚本，模块会继续尝试备份并接管。点检测详情查看失败原因。";
    } else if (s.linked) {
      sceneState = "NovaSched 已接管 Scene";
      sceneNote = "Scene 的四档切换和应用规则由 NovaSched 执行，原调度与初始化脚本已备份，卸载时恢复。";
    } else if (s.sceneInstalled) {
      sceneState = "Scene 已安装 · 待接管";
      sceneNote = "安装已确认，调度回调尚未注册。可点检测详情重新注册。";
    } else if (s.scenePresence === "unknown") {
      sceneState = "Scene 检测暂不可用";
      sceneNote = "系统暂未返回安装状态。调度服务会继续检测，可在检测详情查看原因。";
    } else {
      sceneState = "WebUI 接管";
      sceneNote = "未安装 Scene 系应用，本页管理本地调度。";
    }
    present("#stat-scene", sceneState);
    present("#stat-scene-note", sceneNote);

    /* 规则区 */
    renderRules();
    renderGraphicsViews();
    present("#rules-count", s.rulesLoaded ? `${s.rules.length} 条规则` : "—");
    $("#add-rule").disabled = !editable() || !!s.pending;
    /* 关于 */
    present("#about-version", "v" + UI_VERSION);
    present("#about-engine-version", s.version ? "v" + s.version : "待连接");
    const versionMismatch = !!s.version && s.version !== UI_VERSION;
    $("#version-note").hidden = !versionMismatch;
    present("#version-note", versionMismatch ? `界面 v${UI_VERSION}，服务 v${s.version}。仅更新界面不会升级调度核心；完整安装同一版本并重启后，NovaFAS 与新策略才会全部生效。` : "");
    present("#about-soc", s.online && s.soc ? s.soc : "待识别");
    present("#about-profile", s.online && s.configProfile ? s.configProfile : "待识别");
    present("#about-fas", !s.online ? "待连接" : s.fasEngineState === "active" ? "NovaFAS 已接管" : s.fasEngineState === "idle" ? "NovaFAS 待命" : s.fasSync === "synced" ? "fas-rs 已同步" : fasTitleShort());
  }
  function fasTitleShort() {
    return { ready: "fas-rs 已检测到", running: "fas-rs 运行中", unreadable: "fas-rs 不可读", invalid: "fas-rs 待就绪", absent: "fas-rs 未检测到", unknown: "待确认" }[s.fasState] || "待确认";
  }
  let ruleQuery="";
  /* 历史轨迹（十九）：进入页面后按需加载，后台不轮询 */
  let historyRange = 24, historyLoaded = { range: 0, at: 0 };
  const historyPoints = [];
  async function loadHistory() {
    const status = $("#history-status");
    if (status) status.textContent = "正在读取守护进程轨迹…";
    try {
      const reply = await rootExec(moduleCommand(`history ${historyRange}`), 20000, false);
      // 规格四：区分各类失败，不统一吞成"history 命令失败"。
      if (reply.errno !== 0) {
        const stderr = (reply.stderr || "").trim();
        const stdout = (reply.stdout || "").trim();
        if (/未找到本模块的可执行文件/.test(stdout)) {
          throw new Error("novasched 二进制不存在或模块目录不可用（moduleCommand 未命中）");
        }
        if (/root|su[- ]c|权限/i.test(stderr)) {
          throw new Error(`Root 权限异常：${stderr.slice(0, 120)}`);
        }
        throw new Error(`命令执行失败（exit ${reply.errno}）：${(stderr || stdout || "无输出").slice(0, 120)}`);
      }
      let parsed;
      try {
        parsed = JSON.parse(reply.stdout || "{}");
      } catch (parseError) {
        // stdout 被日志污染或截断时在此显式暴露，而不是变成"命令失败"。
        throw new Error(`JSON 解析失败：${String(parseError?.message || parseError).slice(0, 80)}；原始输出前 60 字符：${(reply.stdout || "").slice(0, 60)}`);
      }
      historyPoints.length = 0;
      for (const sample of parsed.samples || []) historyPoints.push(sample);
      historyLoaded = { range: historyRange, at: Date.now() };
      if (status) status.textContent = historyPoints.length
        ? `${historyPoints.length} 个采样点（本机数据，仅保存在设备上）`
        : "所选时间范围内没有轨迹（历史目录尚未产生采样，或刚清空）；使用一段时间后再来看。";
      drawHistory();
    } catch (error) {
      if (status) status.textContent = `读取失败：${String(error?.message || error).slice(0, 140)}`;
    }
  }
  /* canvas 轨迹：电量（0–100%）与电池温度（双轴）；档位变化画事件刻度。
     缺口（相邻点间隔 > 3×中位间隔）断线，不补零不跨缺口连接。 */
  function drawHistory() {
    const canvas = $("#history-canvas");
    if (!canvas || !canvas.isConnected) return;
    const width = canvas.clientWidth || 360, height = 260;
    canvas.width = width * (window.devicePixelRatio || 1);
    canvas.height = height * (window.devicePixelRatio || 1);
    const ctx = canvas.getContext && canvas.getContext("2d");
    if (!ctx) return;
    ctx.scale(window.devicePixelRatio || 1, window.devicePixelRatio || 1);
    ctx.clearRect(0, 0, width, height);
    const points = historyPoints;
    if (points.length < 2) return;
    const t0 = points[0].t, t1 = points[points.length - 1].t;
    const gaps = [];
    for (let i = 1; i < points.length; i++) gaps.push(points[i].t - points[i - 1].t);
    gaps.sort((a, b) => a - b);
    const medianGap = gaps[gaps.length >> 1] || 30;
    const X = t => 8 + (t - t0) / Math.max(1, t1 - t0) * (width - 16);
    const tempMax = 55;
    const yBatt = v => height * 0.12 + (100 - v) / 100 * height * 0.52;
    const yTemp = v => height * 0.70 + Math.min(1, Math.max(0, (v - 25) / (tempMax - 25))) * height * 0.22;
    // 网格
    ctx.strokeStyle = "rgba(128,128,128,.25)"; ctx.lineWidth = 1;
    for (const pct of [0, 25, 50, 75, 100]) {
      ctx.beginPath(); ctx.moveTo(8, yBatt(pct)); ctx.lineTo(width - 8, yBatt(pct)); ctx.stroke();
    }
    const broken = (a, b) => (b.t - a.t) > medianGap * 3;
    // 电量线
    ctx.strokeStyle = "#4dd0e1"; ctx.lineWidth = 2; ctx.beginPath();
    let pen = false;
    for (let i = 0; i < points.length; i++) {
      const p = points[i];
      if (p.battery == null) { pen = false; continue; }
      const x = X(p.t), y = yBatt(p.battery);
      if (!pen || (i > 0 && broken(points[i - 1], p))) { ctx.moveTo(x, y); pen = true; }
      else ctx.lineTo(x, y);
    }
    ctx.stroke();
    // 温度线
    ctx.strokeStyle = "#ffb74d"; ctx.beginPath(); pen = false;
    for (let i = 0; i < points.length; i++) {
      const p = points[i];
      if (p.temp == null) { pen = false; continue; }
      const x = X(p.t), y = yTemp(p.temp);
      if (!pen || (i > 0 && broken(points[i - 1], p))) { ctx.moveTo(x, y); pen = true; }
      else ctx.lineTo(x, y);
    }
    ctx.stroke();
    // 档位变化事件刻度
    ctx.strokeStyle = "rgba(255,255,255,.5)";
    for (let i = 1; i < points.length; i++) {
      if (points[i].mode !== points[i - 1].mode) {
        const x = X(points[i].t);
        ctx.beginPath(); ctx.moveTo(x, height * 0.06); ctx.lineTo(x, height * 0.10); ctx.stroke();
      }
    }
    canvas._historyMeta = { t0, t1, yBatt, yTemp, X };
  }
  function renderRules() {
    const root = $("#rule-list");
    /* 心跳每 2 秒触发一次 render：内容没变时绝不重建 DOM，否则规则页会随心跳闪烁 */
    const key = JSON.stringify([ruleQuery, s.rulesLoaded, s.rules, [...labels.entries()].map(([k, v]) => [k, v.appLabel, v.icon]), s.graphics.rules, s.fpsRules]);
    if (root.dataset.key === key) return;
    root.dataset.key = key;
    /* FLIP：重建前记录每行位置，重建后按位移做补间——新增/删除规则时
       存活行平滑让位，而不是整表瞬移（动效标准：连续性优先）。 */
    const before = new Map();
    $$(".rule-row", root).forEach(row => { if (row.dataset.edit) before.set(row.dataset.edit, row.getBoundingClientRect().top); });
    if (!s.rulesLoaded) {
      root.innerHTML = `<div class="rule-empty">等待应用规则<br><small>成功连接后，这里会显示守护进程保存的应用策略。</small></div>`;
    } else if (!s.rules.length) {
      root.innerHTML = `<div class="rule-empty">还没有应用规则<br><small>点下方「添加应用规则」，为游戏或视频应用设定独立档位。</small></div>`;
    } else {
      const matches=s.rules.filter(rule=>!ruleQuery || (rule.package+" "+appInfo(rule.package).name).toLowerCase().includes(ruleQuery));
      root.innerHTML = matches.map((rule,index) => {
        const a = appInfo(rule.package);
        // 非默认图形后端在行内显示摘要徽标；默认（跟随系统）不显示。
        const gfx = s.graphics.rules[rule.package];
        const gfxBadge = gfx === "angle" ? '<span class="gfx-badge" title="OpenGL ES 经 ANGLE 转译，目标应用需完全退出重开生效">ANGLE</span>' : "";
        const fpsRule = (s.fpsRules || {})[rule.package];
        const fpsBadge = fpsRule ? `<span class="gfx-badge" title="调度目标（NovaFAS 帧基线，非强制限帧）">${fpsRule}FPS</span>` : "";
        return `<div class="rule-shell"><button class="swipe-delete" data-rule-delete="${esc(rule.package)}" type="button" tabindex="-1" aria-hidden="true">删除</button><button class="rule-row g" data-role="course" data-color-index="${index % 6}" data-radius="18" type="button" data-edit="${esc(rule.package)}" aria-label="${esc(a.name)}，${esc(MODES[rule.mode]?.name || "未知")}档${gfx === "angle" ? "，ANGLE 后端" : ""}">
          ${a.icon
            ? `<img class="app-avatar rule-avatar" src="data:image/png;base64,${a.icon}" alt="" aria-hidden="true">`
            : `<span class="app-avatar rule-avatar app-avatar-ph">${APP_PLACEHOLDER}</span>`}
          <span class="rule-copy"><b>${esc(a.name)}</b><small>${esc(rule.package)}</small></span>
          <span class="mode-badge">${esc(MODES[rule.mode]?.name || "未知")}</span>${gfxBadge}${fpsBadge}
          <svg class="chev" viewBox="0 0 24 24"><path d="m9 5 7 7-7 7"/></svg>
        </button></div>`;
      }).join("") || `<div class="rule-empty">没有找到匹配的应用</div>`;
    }
    /* 名称/图标通道不可用时的如实提示：模块 meta 通道重试耗尽且一条名称
       都没拿到时说明原因，不让用户对着裸包名猜链路状态。 */
    const rulesNote = $("#rules-note");
    if (rulesNote) {
      const anyNamed = s.rules.some(rule => labels.get(basePackage(rule.package))?.appLabel);
      rulesNote.textContent = (!s.rulesLoaded || !s.rules.length || anyNamed)
        ? "本地自动模式关闭后，应用规则生效。"
        : `应用名称与图标暂不可用（${metaDiag.lastError || "模块通道查询失败，已停止重试；重开本页可重试"}；本会话批次 ${metaDiag.batches}、名称命中 ${metaDiag.labels}、图标命中 ${metaDiag.icons}）。展示包名，调度不受影响。`;
    }
    /* 新增的玻璃行注册到渲染器 */
    window.NovaGlass?.scan(root);
    /* FLIP 收尾：只对仍然存在的行做位移补间（删除的行已不在 DOM）。 */
    if (!reducedMotion() && before.size) {
      $$(".rule-row", root).forEach(row => {
        const edit = row.dataset.edit;
        const previous = edit ? before.get(edit) : undefined;
        if (previous == null) return;
        const delta = previous - row.getBoundingClientRect().top;
        if (Math.abs(delta) < 1) return;
        row.animate(
          [{ transform: `translateY(${delta}px)` }, { transform: "none" }],
          { duration: 280, easing: "cubic-bezier(.22,.72,.2,1)" },
        );
      });
    }
    /* 一键清空按钮：仅在存在按应用规则且可编辑时可用。 */
    const clearButton = $("#rules-clear");
    if (clearButton) {
      const removable = s.rules.filter(rule => rule.package !== "*").length;
      clearButton.disabled = !editable() || !removable || !!s.pending;
      clearButton.textContent = removable > 1 ? `一键清空（${removable} 条）` : "一键清空";
    }
  }
  const GRAPHICS_STATUS_TEXT = {
    applied: "已写入（读回一致）", pending: "已配置 · 待同步", follow: "跟随系统",
    auto: "自动", unsupported: "不支持", disabled: "已停用", failed: "失败",
  };
  function graphicsStatusTitle(pkg) {
    const info = s.graphics.status?.[pkg];
    if (!info) return "";
    return `状态 ${GRAPHICS_STATUS_TEXT[info.status] || info.status}${info.detail ? "：" + info.detail : ""}`;
  }
  function renderGraphicsViews() {
    if (!s.graphics.probed) return;
    const cap = s.graphics.capability;
    const vulkanText = !cap ? "—"
      : !cap.vulkan.length ? "未检测到 Vulkan HAL"
      : cap.vulkan.map(device => `${device.name} (API ${device.api})`).join(" · ");
    const angleText = cap ? (cap.angle ? "可用" : "未安装") : "未知";
    const sdkText = cap && cap.sdk ? `Android SDK ${cap.sdk}` : "Android SDK —";
    present("#graphics-cap-angle", angleText);
    present("#graphics-cap-vulkan", vulkanText);
    present("#graphics-cap-time", s.graphics.probed ? "已探测" : "—");
    present("#gpage-angle", angleText);
    present("#gpage-vulkan", vulkanText);
    present("#gpage-sdk", sdkText);
    present("#graphics-page-cap-time", s.graphics.probed ? "已探测" : "—");
    const entries = Object.entries(s.graphics.rules || {});
    const renderInto = (rootId, withEntry) => {
      const root = $(rootId);
      if (!root) return;
      const key = JSON.stringify([entries, s.graphics.status, s.rules]);
      if (root.dataset.key === key) return;
      root.dataset.key = key;
      if (!entries.length) {
        root.innerHTML = `<div class="rule-empty">还没有图形后端规则<br><small>在规则页编辑应用时选择后端（跟随系统/自动/ANGLE/原生/Vulkan）。</small></div>`;
        window.NovaGlass?.scan(root);
        return;
      }
      root.innerHTML = entries.map(([pkg, mode]) => {
        const a = appInfo(pkg);
        const info = s.graphics.status?.[pkg] || {};
        const statusText = GRAPHICS_STATUS_TEXT[info.status] || info.status || "待同步";
        const detail = info.detail || "";
        return `<button class="rule-row g" data-role="course" data-radius="18" type="button" data-edit="${esc(pkg)}" aria-label="${esc(a.name)}，图形后端 ${esc(mode)}">
          ${a.icon
            ? `<img class="app-avatar rule-avatar" src="data:image/png;base64,${a.icon}" alt="" aria-hidden="true">`
            : `<span class="app-avatar rule-avatar app-avatar-ph">${APP_PLACEHOLDER}</span>`}
          <span class="rule-copy"><b>${esc(a.name)}</b><small>${esc(pkg)}</small></span>
          <span class="mode-badge" title="${esc(detail)}">${esc(mode)}${info.status && info.status !== "applied" ? " · " + esc(statusText) : ""}</span>
          <svg class="chev" viewBox="0 0 24 24"><path d="m9 5 7 7-7 7"/></svg>
        </button>`;
      }).join("");
      window.NovaGlass?.scan(root);
    };
    renderInto("#graphics-rule-list", true);
    renderInto("#graphics-page-list", true);
    present("#gpage-count", `${entries.length} 条`);
  }
  /* ================= DeepSeek 自适应引擎页 ================= */
  let aiProbed = false;
  function probeAi(force = false) {
    if (!force && aiProbed) return;
    if (typeof bridge()?.exec !== "function") return;
    rootExec(moduleCommand("ai-status"), 8000, false).then(result => {
      if (result.errno !== 0) return;
      aiProbed = true;
      renderAi(result.stdout || "");
    }).catch(() => {});
  }
  function renderAi(statusText) {
    const get = key => (statusText.match(new RegExp(`^${key}=(.*)$`, "m")) || [])[1] || "";
    const mode = get("ai_mode") || "off";
    $$("#page-ai [data-ai-mode]").forEach(button =>
      button.classList.toggle("active", button.dataset.aiMode === mode));
    present("#ai-mode-badge", get("ai_mode_label") || mode);
    present("#ai-mode-note", `当前：${get("ai_mode_label") || mode}。Key 指纹 ${get("key") || "未设置"}；接口 ${get("base_url") || "—"} · 模型 ${get("model") || "—"}`);
    present("#ai-key-badge", get("key") || "未设置");
    present("#ai-usage-calls", get("calls_today") || "0");
    present("#ai-usage-tokens", `今日 token ${get("tokens_today") || 0}（最近一次 ${get("last_tokens") || 0}）；用量与费用由用户在 DeepSeek 侧承担，本地已做配额+冷却+签名去重三重限频。`);
    const policyRaw = get("policy");
    const policyState = policyRaw.split(";")[0];
    const policyFields = Object.fromEntries(policyRaw.split(";").slice(1).map(part => part.split("=")).filter(pair => pair.length === 2));
    const policyText = policyState === "expired"
      ? "临时策略：已过期（不会再被调度消费；设备状态变化后会重新分析）"
      : policyState === "stored"
        ? `临时策略：有效，${policyFields.expires_in || "?"} 秒后过期 · 建议档位 ${policyFields.mode || "—"} · ceiling 上限 ${policyFields.fas_ceiling || 0}%${get("policy_summary") ? " · " + get("policy_summary") : ""}`
        : "临时策略：无（关闭/观察模式只记录建议，不参与调度）";
    present("#ai-policy-note", policyText);
    const cooldown = Number(get("cooldown_remaining") || 0);
    present("#ai-cooldown-note", `冷却与配额：今日 ${get("calls_today") || "0"} 次调用 · ${get("tokens_today") || 0} token${cooldown > 0 ? ` · 冷却剩余 ${Math.ceil(cooldown / 60)} 分钟` : " · 可立即分析"}`);
    const decision = get("last_decision");
    present("#ai-last-decision", `最近决策：${decision || "—"}`);
    const input = $("#ai-model-input");
    if (input && document.activeElement !== input) input.value = get("model") || "deepseek-chat";
    const url = $("#ai-url-input");
    if (url && document.activeElement !== url) url.value = get("base_url") || "";
  }
  function probeAiLog() {
    if (typeof bridge()?.exec !== "function") return;
    rootExec(moduleCommand("ai-log"), 8000, false).then(result => {
      const lines = (result.stdout || "").split("\n").filter(line => line.trim()).reverse().slice(0, 30);
      const root = $("#ai-log-list");
      if (!root) return;
      const key = (result.stdout || "").length + ":" + lines[0];
      if (root.dataset.key === key) return;
      root.dataset.key = key;
      root.innerHTML = lines.length ? lines.map(line => {
        let at = "", text = line;
        try { const obj = JSON.parse(line); at = new Date((obj.at || 0) * 1000).toLocaleTimeString(); text = obj.summary || obj.reason || obj.error || obj.text || line; } catch (_) {}
        return `<button class="rule-row g" type="button"><span class="rule-copy"><b>${esc(text)}</b><small>${esc(at)}</small></span></button>`;
      }).join("") : `<div class="rule-empty">暂无 AI 调度日志<br><small>设置 Key 并触发事件（或点「立即分析」）后这里会出现决策与原因。</small></div>`;
      present("#ai-log-count", `${lines.length} 条`);
    }).catch(() => {});
  }
  function aiApply(fn, okMessage) {
    return fn.then(result => {
      const text = `${result.stdout || ""}\n${result.stderr || ""}`.trim();
      if (result.errno !== 0) { toast(text.slice(0, 200) || "命令失败"); return false; }
      toast(okMessage || text.slice(0, 200));
      probeAi(true); probeAiLog();
      return true;
    }).catch(error => { toast(`失败：${String(error?.message || error).slice(0, 140)}`); return false; });
  }

  /* ---------------- 日志渲染（增量追加，避免整表重建卡顿） ---------------- */
  const LV_TAG = { error: "错误", warn: "警告", info: "信息", debug: "调试" };
  const logRender = { level: null, rendered: 0 };
  const logList = $("#log-list");
  function logRow(line) {
    return `<article data-raw="${esc(line.raw)}"><time>${esc(line.time || "本地记录")}</time><span class="lv" data-lv="${line.level}">${LV_TAG[line.level]}</span><span class="msg">${esc(line.message)}</span></article>`;
  }
  function rebuildLogs(lines) {
    logList.innerHTML = lines.length ? lines.map(logRow).join("")
      : `<div class="log-empty">${s.logReceived ? "暂时没有匹配的记录<br><small>可清除级别筛选，或等待新的日志。</small>" : "进入日志页后按需连接，不在后台持续订阅。<br><small>连接成功后新日志会实时出现在这里。</small>"}</div>`;
    logRender.level = s.level;
    logRender.rendered = s.logs.length;
  }
  function renderLogs() {
    const lines = visibleLogs();
    const errors = s.logs.filter(l => l.level === "error").length;
    present("#log-count", `${lines.length} / ${s.logs.length} 条${errors ? ` · ${errors} 错误` : ""}${s.logConnected ? " · 实时" : s.logConnecting ? " · 连接中" : ""}`);
    const output = logList;
    const oldScroll = output.scrollTop;
    const nearBottom = output.scrollHeight - output.scrollTop - output.clientHeight < 60;
    const trimmed = s.logs.length < logRender.rendered;
    if (logRender.level !== s.level || trimmed) {
      rebuildLogs(lines);
    } else if (s.logs.length > logRender.rendered && output.querySelector(".log-empty")) {
      rebuildLogs(lines);  /* 原来是空态占位，来了新行改用整体重建清掉占位 */
    } else {
      /* 日志只会追加或整段裁剪；裁剪已在上面的分支处理，这里只追加新行 */
      for (let index = logRender.rendered; index < s.logs.length; index++) {
        const line = s.logs[index];
        if (s.level === "all" || line.level === s.level) output.insertAdjacentHTML("beforeend", logRow(line));
      }
      logRender.rendered = s.logs.length;
      if (s.level !== "all" && output.getElementsByTagName("article").length > 1100) rebuildLogs(lines);
    }
    if (s.follow || nearBottom) output.scrollTop = output.scrollHeight;
    else output.scrollTop = oldScroll;
  }
  const logsActive = () => s.page === "logs";

  /* ---------------- 页面切换（底部胶囊 dock，双向转场） ---------------- */
  const PAGE_IDS = ["overview", "rules", "logs", "about", "history", "graphics", "ai"];
  /* 子页面没有 dock 按键：高亮跟随父页（图形→规则，AI→设置）。 */
  const DOCK_PARENT = { graphics: "rules", ai: "about" };
  let currentPage = "overview", routeDepth = 0;
  const pageScroll = new Map();
  const motionQuery=window.matchMedia?.("(prefers-reduced-motion: reduce)");
  const reducedMotion=()=>ui.reduceMotion||!!motionQuery?.matches;
  const motions=new Map();
  let stopPageMotion=null,dialogEpoch=0,personalEpoch=0;
  function ownedMotion(element,frames,options,complete){
    motions.get(element)?.cancel();
    let animation=null,timer=0,settled=false;
    const controller={cancel:()=>settle(false),finish:()=>settle(true)};
    function settle(finished){if(settled)return;settled=true;clearTimeout(timer);if(motions.get(element)===controller)motions.delete(element);if(animation){animation.onfinish=null;animation.oncancel=null;animation.cancel();}complete?.(finished);}
    if(reducedMotion()||typeof element?.animate!=="function"){settle(true);return controller;}
    motions.set(element,controller);
    try{animation=element.animate(frames,options);animation.onfinish=controller.finish;animation.oncancel=controller.cancel;timer=setTimeout(controller.finish,(options.duration||0)+80);}catch(_){settle(true);}
    return controller;
  }
  function finishMotion(){stopPageMotion?.();for(const motion of [...motions.values()])motion.finish();}
  motionQuery?.addEventListener?.("change",()=>{if(reducedMotion())finishMotion();});
  /* 物理弹簧：欠阻尼二阶系统（ζ≈0.84，轻微过冲）按 16ms 采样成 WAAPI
     关键帧，每帧 easing:"linear"——运动由采样值承载，而不是把一条
     cubic-bezier 宣称成弹簧。时长由收敛阈值推导（约 430ms，与旧转场
     时长同量级），不写固定毫秒。 */
  const SPRING={stiffness:170,damping:22,dt:1/60,maxMs:900,threshold:.01,velocity:.04};
  function springFrames(p0){
    const {stiffness,damping,dt,maxMs,threshold,velocity}=SPRING;
    const frames=[p0];let p=p0,v=0,t=0;
    while(t<maxMs){
      const a=-stiffness*(p-1)-damping*v;
      v+=a*dt;p+=v*dt;t+=dt*1000;
      frames.push(p);
      if(Math.abs(p-1)<threshold&&Math.abs(v)<velocity)break;
    }
    frames.push(1);
    return frames;
  }
  /* 把 0..1（可过冲）的弹簧进度映射为关键帧属性；build 每帧调用一次。 */
  function springKeyframes(p0,build){
    const frames=springFrames(p0),last=frames.length-1;
    return {frames:frames.map((p,i)=>({...build(p),offset:i===0?0:i/last,easing:"linear"})),duration:Math.round(frames.length*1000/60)};
  }
  /* 中断续接：元素在动画或拖拽中时，读取当前计算矩阵作为起点，
     新动画从当前位置继续，不跳回端点重放。宿主不提供
     getComputedStyle 时返回 null，动画退回端点起播。 */
  function currentMotionState(element){
    if(typeof getComputedStyle!=="function")return null;
    const computed=element&&getComputedStyle(element);
    if(computed&&computed.transform&&computed.transform.startsWith("matrix(")){
      const parts=computed.transform.slice(7,-1).split(",").map(Number);
      if(parts.length===6&&parts.every(Number.isFinite)){
        return {dx:parts[4],dy:parts[5],sx:parts[0]||1,sy:parts[3]||1,op:Number(computed.opacity)||0,raw:true};
      }
    }
    return null;
  }
  /* 兼容别名：所有触感统一走 NovaHaptics 语义层 */
  function haptic() { NovaHaptics.select(); }
  function rememberRoute(overlay = null, replace = false) {
    try {
      const state = { nova: true, page: currentPage, overlay, depth: replace ? routeDepth : ++routeDepth };
      history[replace ? "replaceState" : "pushState"](state, "", location.href);
    } catch (_) {}
  }
  rememberRoute(null, true);
  const mainScroll = $("#main");
  function syncInert() {
    const modal = !!$(".appearance-layer.open,.dialog-layer.open");
    for (const selector of ["main", "#dock-wrap", ".top-bar"]) { const node=$(selector); if(node)node.inert=modal; }
    $("#personal-panel").inert = $("#dialog-layer").classList.contains("open");
    document.body.classList.toggle("overlay-open", modal);
    document.body.style.overflow=modal?"hidden":"";
  }
  /* 锚点 morph（FLIP + 缩放补偿）：打开与关闭共用同一几何描述，
     弹簧驱动、可中断——从当前实际位置/圆角继续，不跳回端点。
     文字保护：内容包进 .morph-inner 并做逐帧反向缩放，容器按真实边界
     缩放而文字/图标始终以目标尺寸渲染（经典 FLIP 补偿），不存在
     “文字随容器拉伸”。圆角：每帧写入 rv/sx、rv/sy 的水平/垂直半径，
     渲染出的圆角恒等于起点→终点的线性插值，中途与结尾都不跳变。
     玻璃装饰层 .glass-surface 留在容器外（弹层的预览态选择器依赖
     直接子代关系）。 */
  const morphRadius = new WeakMap();
  function ensureMorphInner(element) {
    if (element.dataset.morphInner === "1") return element.querySelector(":scope > .morph-inner");
    const inner = document.createElement("div");
    inner.className = "morph-inner";
    // 静态快照遍历：appendChild 会移动节点，while(firstChild) 写法会在
    // “最后一个子节点是玻璃装饰层”时原地自旋。
    for (const node of [...element.childNodes]) {
      if (node.nodeType === 1 && node.classList.contains("glass-surface")) continue;
      inner.appendChild(node);
    }
    element.appendChild(inner);
    element.dataset.morphInner = "1";
    return inner;
  }
  function readRadius(element) {
    if (morphRadius.has(element)) return morphRadius.get(element);
    if (typeof getComputedStyle !== "function") return 22;
    const value = parseFloat(getComputedStyle(element).borderTopLeftRadius);
    return Number.isFinite(value) ? value : 22;
  }
  function morph(element,anchor,opening=true,complete,startState=null){
    const target=element?.getBoundingClientRect(),anchorRect=anchor?.getBoundingClientRect();
    let small={dx:0,dy:12,sx:.96,sy:.96,op:0};
    if(target?.width&&target.height&&anchorRect?.width&&anchorRect.height){
      small={dx:anchorRect.left+anchorRect.width/2-target.left-target.width/2,dy:anchorRect.top+anchorRect.height/2-target.top-target.height/2,
        sx:Math.max(.08,anchorRect.width/target.width),sy:Math.max(.08,anchorRect.height/target.height),op:0};
    }
    const identity={dx:0,dy:0,sx:1,sy:1,op:1};
    const midFlight=motions.has(element)||!!element?.style?.transform;
    let from=startState||(opening&&!midFlight?small:currentMotionState(element)||(opening?small:identity));
    const to=opening?identity:small;
    const lerp=(a,b,m)=>a+(b-a)*m;
    // 圆角端点：打开=锚点圆角→容器圆角；关闭=容器圆角→锚点圆角。
    // 中断续接时从上一动画最后帧记录的圆角出发。
    const rFrom=opening?readRadius(anchor):readRadius(element);
    const rTo=opening?readRadius(element):(readRadius(anchor)||rFrom);
    const build=m=>{
      const sx=lerp(from.sx,to.sx,m),sy=lerp(from.sy,to.sy,m);
      const rv=Math.max(0,lerp(rFrom,rTo,m));
      morphRadius.set(element,rv);
      return {
        transform:`translate(${lerp(from.dx,to.dx,m)}px, ${lerp(from.dy,to.dy,m)}px) scale(${lerp(from.sx,to.sx,m)}, ${lerp(from.sy,to.sy,m)})`,
        opacity:Math.max(0,Math.min(1,lerp(from.op,to.op,m))),
        borderRadius:`${(rv/Math.max(.05,sx)).toFixed(1)}px / ${(rv/Math.max(.05,sy)).toFixed(1)}px`
      };
    };
    const {frames,duration}=springKeyframes(0,build);
    // 内容反缩放：内层每帧 1/sx、1/sy，抵消容器缩放——内容始终以
    // 目标尺寸渲染，展开时从容器的真实边界逐渐显露（FLIP 裁切接入）。
    const inner=element?.dataset ? ensureMorphInner(element) : null;
    const prevOverflow=inner?.parentElement?element.style.overflow:"",prevPE=inner?inner.style.pointerEvents:"";
    let innerMotion=null;
    if(inner){
      const sx0=from.sx,sy0=from.sy,tx=to.sx,ty=to.sy;
      const innerFrames=frames.map(frame=>{
        const match=/scale\(([-\d.]+), ([-\d.]+)\)/.exec(frame.transform);
        const sx=match?parseFloat(match[1]):lerp(sx0,tx,frame.offset),sy=match?parseFloat(match[2]):lerp(sy0,ty,frame.offset);
        return {...frame,transform:`scale(${(1/Math.max(.05,sx)).toFixed(4)}, ${(1/Math.max(.05,sy)).toFixed(4)})`};
      });
      innerMotion=ownedMotion(inner,innerFrames,{duration,fill:"both"},null);
      // 飞行期间冻结滚动与指针：约 0.4 秒，落定后恢复原状态。
      element.style.overflow="hidden";
      inner.style.pointerEvents="none";
    }
    const motion=ownedMotion(element,frames,{duration,fill:"both"},finished=>{
      innerMotion?.cancel();
      if(element){element.style.overflow=prevOverflow;}
      if(inner){inner.style.pointerEvents=prevPE;}
      complete?.(finished);
    });
    window.NovaGlass?.invalidate(Math.min(600,duration+120));return motion;
  }
  function animatePageSwitch(previous,next,forward){
    stopPageMotion?.();
    const incoming=$("#page-"+next),outgoing=$("#page-"+previous);
    if(!incoming||!outgoing||reducedMotion()||typeof incoming.animate!=="function")return;
    if(mainScroll.scrollTop>0||(pageScroll.get(previous)||0)>0){
      let motion;const finish=()=>{motion?.cancel();if(stopPageMotion===finish)stopPageMotion=null;};stopPageMotion=finish;
      motion=ownedMotion(incoming,[{opacity:.45},{opacity:1}],{duration:220,easing:"ease-out",fill:"both"},()=>{if(stopPageMotion===finish)stopPageMotion=null;});window.NovaGlass?.invalidate(320);return;
    }
    let out,inn,timer=0,done=0,settled=false;
    const finish=()=>{if(settled)return;settled=true;clearTimeout(timer);for(const a of [out,inn])if(a){a.onfinish=null;a.oncancel=null;a.cancel();}outgoing.classList.remove("leaving");if(stopPageMotion===finish)stopPageMotion=null;window.NovaGlass?.invalidate(240);};
    stopPageMotion=finish;outgoing.classList.add("leaving");
    // 弹簧驱动 + 中断续接：快速连点切页时从各页当前计算位移继续，
    // 不从 ±12%/±18% 端点重放。
    const restPct=()=>Math.round((mainScroll.clientWidth||1)*.18);
    const shift=(element,rest)=>currentMotionState(element)||{dx:rest,dy:0,sx:1,sy:1,op:rest?0:1};
    const outFrom=shift(outgoing,0),inFrom=shift(incoming,1);
    const outDx=(forward?-1:1)*restPct();
    const outBuild=m=>({transform:`translateX(${outFrom.dx+(outDx-outFrom.dx)*m}px)`,opacity:Math.max(0,Math.min(1,outFrom.op*(1-m)))});
    const inBuild=m=>({transform:`translateX(${inFrom.dx*(1-m)}px)`,opacity:Math.max(0,Math.min(1,inFrom.op+(1-inFrom.op)*m))});
    try{
      const outSpec=springKeyframes(0,outBuild),inSpec=springKeyframes(0,inBuild);
      out=outgoing.animate(outSpec.frames,{duration:outSpec.duration,fill:"both"});
      inn=incoming.animate(inSpec.frames,{duration:inSpec.duration,fill:"both"});
      const ended=()=>{if(++done===2)finish();};out.onfinish=ended;inn.onfinish=ended;out.oncancel=finish;inn.oncancel=finish;timer=setTimeout(finish,Math.max(outSpec.duration,inSpec.duration)+120);
    }catch(_){finish();}
    window.NovaGlass?.invalidate(460);
  }
  function setPage(page, fromHistory = false) {
    if (!PAGE_IDS.includes(page)) page = "overview";
    const previous = currentPage, changed = previous !== page;
    if (changed) {
      stopPageMotion?.();
      pageScroll.set(previous, mainScroll.scrollTop || 0);
      if ($("#appearance-layer").classList.contains("open")) closePersonalize(true,true);
      if ($("#dialog-layer").classList.contains("open")) closeDialog(true,true);
    }
    currentPage = page; s.page = page;
    $$(".page").forEach(node => {
      node.classList.toggle("active", node.id === "page-" + page);
      node.setAttribute("aria-hidden", String(node.id !== "page-" + page));
      node.inert=node.id!=="page-"+page;
    });
    const index = PAGE_IDS.indexOf(DOCK_PARENT[page] ?? page);
    $$(".dock button").forEach((button, buttonIndex) => {
      if (buttonIndex === index) button.setAttribute("aria-current", "page");
      else button.removeAttribute("aria-current");
    });
    $(".dock").style.setProperty("--tab-index", String(index));
    document.documentElement.dataset.page = page;
  if (page === "history") {
    const stale = historyLoaded.range !== historyRange || Date.now() - historyLoaded.at > 60000;
    if (stale) loadHistory(); else drawHistory();
  }
  if (page === "graphics") probeGraphics(true);
  if (page === "ai") { probeAi(true); probeAiLog(); }
    if (changed) {
      /* Tab 式导航：每个页面保留自己的滚动位置，切回时恢复。 */
      mainScroll.scrollTop = pageScroll.get(page) || 0;
      animatePageSwitch(previous, page, PAGE_IDS.indexOf(page) > PAGE_IDS.indexOf(previous));
      if (!fromHistory) rememberRoute();
    }
    window.dispatchEvent(new CustomEvent("nova:page", { detail: { animate: changed && !reducedMotion() } }));
    if (page === "logs") openEndpoint("logs");
    else { stopSocket("logs"); s.logConnected = false; s.logConnecting = false; renderLogs(); }
  }
  document.addEventListener("click", event => {
    const button = event.target instanceof Element ? event.target.closest("[data-page]") : null;
    if (!button) return; switchPageWithHaptic(button.dataset.page);
  });
  window.addEventListener("popstate", event => {
    if (!event.state?.nova) return;
    routeDepth = event.state.depth || 0;
    if (event.state.overlay !== "dialog") closeDialog(true);
    if (event.state.overlay !== "appearance") closePersonalize(true);
    setPage(event.state.page || "overview", true);
    if (event.state.overlay === "appearance" && !$("#appearance-layer").classList.contains("open")) openPersonalize(null, false);
  });
  const dock = $(".dock"); let dockDrag = null, ignoreDockClick = 0;
  dock.addEventListener("pointerdown", event => {
    if(event.button>0||event.isPrimary===false||dockDrag)return;
    const rect = dock.getBoundingClientRect(), cell = (rect.width - 10) / 5;
    dockDrag = { pointer: event.pointerId, start: event.clientX, startY: event.clientY, rect, cell, moved: false, index: PAGE_IDS.indexOf(currentPage) };
    /* 按下后下一帧给轻微压缩反馈；不缩放单个按钮，点击区域保持稳定 */
    requestAnimationFrame(() => { if (dockDrag && !dockDrag.moved) dock.classList.add("dock-pressing"); });
  });
  dock.addEventListener("pointermove", event => {
    if (!dockDrag || event.pointerId !== dockDrag.pointer) return;
    if (!dockDrag.moved) {
      const dx = Math.abs(event.clientX - dockDrag.start), dy = Math.abs(event.clientY - dockDrag.startY);
      if(dy>8&&dy>dx){endDock(event,true);return;}
      if (dx <= 8) return;
      dockDrag.moved = true;
      dock.setPointerCapture?.(event.pointerId);
    }
    if (!dockDrag.moved) return;
    event.preventDefault(); dock.classList.add("dock-dragging");
    const x = Math.max(-6, Math.min(dockDrag.cell * 3 + 6, event.clientX - dockDrag.rect.left - 5 - dockDrag.cell / 2));
    dock.style.setProperty("--dock-drag-x", x + "px"); window.NovaGlass?.invalidate();
  });
  function endDock(event, cancelled = false) {
    if (!dockDrag || event.pointerId !== dockDrag.pointer) return;
    const drag = dockDrag;
    dock.classList.remove("dock-pressing");
    /* 只有拖拽换页在 pointerup 提交；原地点按交给按钮自身的 click 事件，
     * 两者职责分离，绝不用坐标推断覆盖真实命中目标。 */
    if (drag.moved && !cancelled) {
      const index = Math.max(0, Math.min(3, Math.round((event.clientX - drag.rect.left - 5 - drag.cell / 2) / drag.cell)));
      switchPageWithHaptic(PAGE_IDS[index]);
      ignoreDockClick = performance.now() + 180;
    }
    dockDrag=null;
    if(dock.hasPointerCapture?.(event.pointerId))dock.releasePointerCapture?.(event.pointerId);
    dock.classList.remove("dock-dragging");dock.style.removeProperty("--dock-drag-x"); window.NovaGlass?.invalidate(360);
  }
  /* 统一的切页入口：有效切换一次触感；点击当前页只给轻反馈并保留状态 */
  function switchPageWithHaptic(page) {
    const same = page === currentPage;
    setPage(page);
    if (same) NovaHaptics.tick();
    else NovaHaptics.select();
  }
  dock.addEventListener("pointerup", event => endDock(event));
  dock.addEventListener("pointercancel", event => endDock(event, true));
  dock.addEventListener("lostpointercapture",event=>endDock(event,true));
  window.addEventListener("blur",()=>{if(dockDrag)endDock({pointerId:dockDrag.pointer},true);});
  /* 原地点按已在 pointerup 处理；吞掉随后的 click，避免一次操作触发两次 */
  dock.addEventListener("click", event => { if (performance.now() < ignoreDockClick) { event.preventDefault(); event.stopImmediatePropagation(); } }, true);
  dock.addEventListener("keydown", event => {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    event.preventDefault(); const current = PAGE_IDS.indexOf(currentPage);
    const index = event.key === "Home" ? 0 : event.key === "End" ? 3 : Math.max(0, Math.min(3, current + (event.key === "ArrowRight" ? 1 : -1)));
    switchPageWithHaptic(PAGE_IDS[index]); $$("button", dock)[index]?.focus();
  });
  /* ---------------- 触控高光（借鉴 SleepDown 的 InteractiveHighlight） ---------------- */
  function markSheen(event) {
    const target = event.target instanceof Element ? event.target.closest("[data-sheen]") : null;
    if (!target) return;
    const rect = target.getBoundingClientRect();
    target.style.setProperty("--mx", `${event.clientX - rect.left}px`);
    target.style.setProperty("--my", `${event.clientY - rect.top}px`);
  }
  document.addEventListener("pointerdown", markSheen, { passive: true });
  document.addEventListener("pointermove", markSheen, { passive: true });

  const wallpaperCache=new Map();let wallpaperEpoch=0,wallpaperKey="";
  function currentCrop(){return window.innerWidth>window.innerHeight?ui.landscape:ui.portrait;}
  function loadWallpaperFromStorage(force=false) {
    const dark=document.documentElement.dataset.theme==="dark";
    const key=ui.wallData || (dark?"./assets/wallpapers/dark.jpg":"./assets/wallpapers/light.jpg");
    if(key===wallpaperKey&&!force)return;wallpaperKey=key;
    const epoch=++wallpaperEpoch;
    if(wallpaperCache.has(key)){window.NovaGlass?.setWallpaper(wallpaperCache.get(key));return;}
    const img=new Image();
    img.onload=()=>{if(epoch!==wallpaperEpoch)return;wallpaperCache.clear();wallpaperCache.set(key,img);window.NovaGlass?.setWallpaper(img);};
    img.onerror=()=>{if(epoch===wallpaperEpoch){wallpaperKey="";toast("壁纸无法读取，请重新选择");}};img.src=key;
  }
  function applyTheme() {
    const dark=ui.theme==="dark" || ui.theme==="system"&&!!darkQuery?.matches;
    const root=document.documentElement;root.dataset.theme=dark?"dark":"light";root.dataset.accent=ui.accent;root.dataset.motion=ui.reduceMotion?"reduce":"full";
    $("meta[name=theme-color]").content=dark?"#0d1015":"#e9eef5";
    $$(".theme-picker button").forEach(button=>button.classList.toggle("active",button.dataset.themeChoice===ui.theme));
    $$(".swatch-row button").forEach(button=>button.classList.toggle("active",button.dataset.accentChoice===ui.accent));
    $$("[data-color-mode]").forEach(button=>{button.classList.toggle("active",button.dataset.colorMode===ui.colorMode);button.setAttribute("aria-pressed",String(button.dataset.colorMode===ui.colorMode));});
    $$("[data-card-color]").forEach(button=>button.setAttribute("aria-pressed",String(button.dataset.cardColor.toLowerCase()===ui.cardColor.toLowerCase())));
    root.style.setProperty("--glass-blur",ui.cardBlur+"px");
    for(const [input,key,output,suffix] of [["glass-level","glassLevel","glass-value",""],["wall-blur","wallBlur","wall-blur-val","%"],["wall-bright","wallBright","wall-bright-val","%"],["card-alpha","cardAlpha","card-alpha-val","%"],["card-blur","cardBlur","card-blur-val",""],["card-radius","cardRadius","card-radius-val",""],["chrome-blur","chromeBlur","chrome-blur-val","%"]]){
      const slider=$("#"+input);if(!slider)continue;
      if(slider.value!==String(ui[key]))slider.value=String(ui[key]);
      slider.style.setProperty("--val",((ui[key]-Number(slider.min))/(Number(slider.max)-Number(slider.min))*100)+"%");present("#"+output,ui[key]+suffix);
    }
    for(const [id,key] of [["dispersion-switch","dispersion"],["material-switch","material"],["motion-switch","reduceMotion"]])$("#"+id).setAttribute("aria-checked",String(ui[key]));
    $("#wall-clear").hidden=!ui.wallData;
    window.NovaGlass?.setTheme(dark?"dark":"light",ui.accent);
    window.NovaGlass?.setLevel(ui.glassLevel);
    window.NovaGlass?.setWallOptions({blur:ui.wallBlur,brightness:ui.wallBright/100,...currentCrop()});
    window.NovaGlass?.configure({level:ui.glassLevel,cardAlpha:ui.cardAlpha,cardBlur:ui.cardBlur,cardRadius:ui.cardRadius,chromeBlur:ui.chromeBlur,dispersion:ui.dispersion,material:ui.material,colorMode:ui.colorMode,cardColor:ui.cardColor,palette:ui.palette});
    loadWallpaperFromStorage();
    if(reducedMotion())finishMotion();
  }
  $$(".theme-picker button").forEach(button=>button.onclick=()=>{ui.theme=button.dataset.themeChoice;saveUI();applyTheme();});
  $$(".swatch-row button").forEach(button=>button.onclick=()=>{ui.accent=button.dataset.accentChoice;saveUI();applyTheme();});
  $$("[data-color-mode]").forEach(button=>button.onclick=()=>{ui.colorMode=button.dataset.colorMode;saveUI();applyTheme();});
  $$("[data-card-color]").forEach(button=>button.onclick=()=>{ui.cardColor=button.dataset.cardColor;ui.colorMode="solid";saveUI();applyTheme();});
  $("#card-custom-color").oninput=event=>{ui.cardColor=event.target.value;ui.colorMode="solid";applyTheme();};
  $("#card-custom-color").onchange=()=>saveUI();
  $("#wall-colors").onclick=()=>{ui.palette=window.NovaGlass?.paletteFromWallpaper()||ui.palette;ui.colorMode="colorful";saveUI();applyTheme();toast("已从当前壁纸提取卡片配色");};
  for(const [id,key] of [["dispersion-switch","dispersion"],["material-switch","material"],["motion-switch","reduceMotion"]])$("#"+id).onclick=()=>{ui[key]=!ui[key];saveUI();applyTheme();haptic();};
  let sliderTimer=0;
  function stopSliderPreview(){clearTimeout(sliderTimer);$("#appearance-layer").classList.remove("previewing");$$(".slider-owner").forEach(node=>node.classList.remove("slider-owner"));window.NovaGlass?.invalidate(220);}
  function bindSlider(id,key) {
    const slider=$(id);if(!slider)return;
    slider.oninput=()=>{const value=Number(slider.value);if(!Number.isFinite(value))return;
      /* 只在跨越离散刻度时给轻触感，并按语义层内部节流 */
      const step=Number(slider.step||1);
      if(Number.isFinite(step)&&step>0){const k=Math.round(value/step);if(slider._tickK!==undefined&&k!==slider._tickK)NovaHaptics.tick();slider._tickK=k;}
      ui[key]=value;applyTheme();};
    slider.onchange=()=>{saveUI();stopSliderPreview();};
    slider.addEventListener("pointerdown",()=>{
      clearTimeout(sliderTimer);sliderTimer=setTimeout(()=>{
        slider.closest(".personal-section")?.classList.add("slider-owner");$("#appearance-layer").classList.add("previewing");window.NovaGlass?.invalidate(220);
      },160);
    });
    slider.addEventListener("pointerup",stopSliderPreview);slider.addEventListener("pointercancel",stopSliderPreview);
    slider.addEventListener("keydown",()=>{clearTimeout(sliderTimer);});
  }
  for(const [id,key] of [["glass-level","glassLevel"],["wall-blur","wallBlur"],["wall-bright","wallBright"],["card-alpha","cardAlpha"],["card-blur","cardBlur"],["card-radius","cardRadius"],["chrome-blur","chromeBlur"]])bindSlider("#"+id,key);
  darkQuery?.addEventListener?.("change",applyTheme);window.addEventListener("orientationchange",()=>{setTimeout(applyTheme,80);});window.addEventListener("resize",()=>{window.NovaGlass?.setWallOptions(currentCrop());});
  let personalAnchor=null,personalFocus=null,personalCloseTimer=0;
  function openPersonalize(anchor=$("#personalize-open"),push=true,glassOnly=false) {

    const layer=$("#appearance-layer");if(layer.classList.contains("open")&&!layer.classList.contains("closing"))return;
    if(anchor&&personalAnchor&&anchor!==personalAnchor)personalAnchor.style.visibility="";
    personalAnchor=anchor||personalAnchor;
    const epoch=++personalEpoch;if(!layer.classList.contains("open"))personalFocus=document.activeElement;
    motions.get($("#personal-panel"))?.cancel();clearTimeout(personalCloseTimer);layer.classList.remove("closing");
    layer.classList.add("open");layer.setAttribute("aria-hidden","false");syncInert();
    if(push&&history.state?.overlay!=="appearance")rememberRoute("appearance");
    const panel=$("#personal-panel");window.NovaGlass?.scan(panel);morph(panel,personalAnchor);
    if(personalAnchor=== $("#personalize-open"))personalAnchor.style.visibility="hidden";
    requestAnimationFrame(()=>{if(epoch!==personalEpoch||!layer.classList.contains("open"))return;if(glassOnly)$("#glass-controls").scrollIntoView?.({block:"start"});else $(".personal-scroll").scrollTop=0;$("#personalize-close").focus();});
    window.dispatchEvent(new Event("nova:overlay"));
  }
  function closePersonalize(fromHistory=false,immediate=false,startState=null){
    const layer=$("#appearance-layer");if(!layer.classList.contains("open")||layer.classList.contains("closing")&&!immediate)return;
    stopSliderPreview();const epoch=++personalEpoch;motions.get($("#personal-panel"))?.cancel();clearTimeout(personalCloseTimer);
    const finish=finished=>{if(!finished||epoch!==personalEpoch)return;layer.classList.remove("open","closing");layer.setAttribute("aria-hidden","true");if(personalAnchor)personalAnchor.style.visibility="";syncInert();if(!$("#dialog-layer").classList.contains("open")&&personalFocus?.isConnected)personalFocus.focus();window.dispatchEvent(new Event("nova:overlay"));if(!fromHistory&&history.state?.overlay==="appearance")history.back();};
    layer.classList.add("closing");if(immediate||reducedMotion())finish(true);else morph($("#personal-panel"),personalAnchor,false,finish,startState);
  }
  $("#personalize-open").onclick=event=>openPersonalize(event.currentTarget);
  $("#appearance-settings").onclick=event=>openPersonalize(event.currentTarget);
  $("#glass-settings").onclick=event=>openPersonalize(event.currentTarget,true,true);
  $("#personalize-close").onclick=()=>closePersonalize();
  $("#appearance-layer").addEventListener("click",event=>{if(event.target===$("#appearance-layer"))closePersonalize();});
  let panelDrag=null;const panelHandle=$(".personal-title");
  panelHandle.addEventListener("pointerdown",event=>{if(event.button>0||event.isPrimary===false||event.target.closest("button")||panelDrag)return;panelDrag={id:event.pointerId,y:event.clientY,dy:0,lastY:event.clientY,lastT:performance.now(),v:0};panelHandle.setPointerCapture?.(event.pointerId);});
  panelHandle.addEventListener("pointermove",event=>{if(!panelDrag||event.pointerId!==panelDrag.id)return;const now=performance.now(),dt=now-panelDrag.lastT;panelDrag.dy=Math.max(0,event.clientY-panelDrag.y);
    // 速度用最近一次移动采样（px/ms），60ms 内的样本才有效：停住再松手
    // 不应被视为甩动。
    if(dt>0&&dt<120){panelDrag.v=(event.clientY-panelDrag.lastY)/dt;panelDrag.lastY=event.clientY;panelDrag.lastT=now;}
    else if(dt>=120){panelDrag.v=0;panelDrag.lastY=event.clientY;panelDrag.lastT=now;}
    if(panelDrag.dy>8){event.preventDefault();$("#personal-panel").style.transform=`translateY(${Math.min(140,panelDrag.dy)}px)`;window.NovaGlass?.invalidate();}});
  function endPanelDrag(event,cancelled=false){
    if(!panelDrag||event.pointerId!==panelDrag.id)return;
    const drag=panelDrag;panelDrag=null;
    if(panelHandle.hasPointerCapture?.(event.pointerId))panelHandle.releasePointerCapture?.(event.pointerId);
    const panel=$("#personal-panel");
    // 甩动判定：位移过阈值或释放速度足够大（且方向向下）即完成关闭；
    // 否则弹簧回弹到原位。取消一律回弹。
    const fling=!cancelled&&drag.dy>0&&drag.v>0.45;
    if(!cancelled&&(drag.dy>65||fling)){
      // 从当前拖拽位置继续关闭动画（morph 读取传入起点，不跳变）。
      panel.style.transform="";
      closePersonalize(false,false,{dx:0,dy:drag.dy,sx:1,sy:1,op:1});
    }else if(drag.dy>8){
      panel.style.transform="";
      const {frames,duration}=springKeyframes(0,m=>({transform:`translateY(${drag.dy*(1-m)}px)`,opacity:1}));
      ownedMotion(panel,frames,{duration,fill:"both"});
    }
    window.NovaGlass?.invalidate(360);
  }
  panelHandle.addEventListener("pointerup",e=>endPanelDrag(e));panelHandle.addEventListener("pointercancel",e=>endPanelDrag(e,true));panelHandle.addEventListener("lostpointercapture",e=>endPanelDrag(e,true));
  $("#wall-pick").onclick=()=>$("#wall-input").click();
  $("#wall-clear").onclick=()=>{ui.wallData="";ui.portrait={centerX:.5,centerY:.5,scale:1};ui.landscape={centerX:.5,centerY:.5,scale:1};saveUI();applyTheme();toast("已恢复默认壁纸");};
  $("#wall-frame").onclick=()=>{const img=window.NovaGlass?.getWallpaper();if(img)showWallpaperEditor(img,false);else toast("请等待壁纸加载完成");};
  $("#wall-input").addEventListener("change",event=>{
    const file=event.target.files?.[0];event.target.value="";if(!file)return;
    if(!file.type.startsWith("image/")){toast("请选择图片文件");return;}
    const reader=new FileReader();reader.onerror=()=>toast("图片读取失败");reader.onload=()=>{
      const img=new Image();img.onload=()=>showWallpaperEditor(img,true);img.onerror=()=>toast("图片无法解析，请换一张试试");img.src=String(reader.result);
    };reader.readAsDataURL(file);
  });
  function showWallpaperEditor(img,isNew) {
    let orientation=window.innerWidth>window.innerHeight?"landscape":"portrait";
    const crops={portrait:isNew?{centerX:.5,centerY:.5,scale:1}:{...ui.portrait},landscape:isNew?{centerX:.5,centerY:.5,scale:1}:{...ui.landscape}};
    openDialog(`<h3 id="dialog-title">调整壁纸</h3><div class="segmented" role="group" aria-label="壁纸方向"><button data-crop="portrait" type="button">竖屏</button><button data-crop="landscape" type="button">横屏</button></div><div class="crop-preview" id="crop-preview"><canvas id="crop-canvas"></canvas><span class="crop-guide" aria-hidden="true"></span></div><p class="crop-help">拖动调整位置，双指或滑条缩放。横竖屏分别保存。</p><input class="liquid-range" type="range" min="100" max="400" value="100" id="crop-scale" aria-label="壁纸缩放"><div class="dialog-actions"><button class="btn-capsule ghost" id="crop-cancel" type="button">取消</button><button class="btn-capsule accent" id="crop-save" type="button">完成</button></div>`);
    $(".dialog").classList.add("crop-dialog");
    const canvas=$("#crop-canvas"),context=canvas.getContext("2d"),preview=$("#crop-preview"),zoom=$("#crop-scale");const pointers=new Map();let lastPinch=0;
    if(!context||!img.naturalWidth||!img.naturalHeight){closeDialog(true,true);toast("当前宿主无法编辑这张图片");return;}
    function draw(){
      const w=Math.max(1,Math.round(preview.clientWidth||258)),h=orientation==="portrait"?Math.round(w*1.2):Math.round(w*.65);preview.style.height=h+"px";canvas.width=w*2;canvas.height=h*2;
      const crop=crops[orientation],cover=Math.max(canvas.width/img.naturalWidth,canvas.height/img.naturalHeight)*crop.scale,dw=img.naturalWidth*cover,dh=img.naturalHeight*cover;
      const x=Math.max(canvas.width-dw,Math.min(0,canvas.width/2-crop.centerX*dw)),y=Math.max(canvas.height-dh,Math.min(0,canvas.height/2-crop.centerY*dh));
      context.clearRect(0,0,canvas.width,canvas.height);context.drawImage(img,x,y,dw,dh);
      $$("[data-crop]").forEach(button=>button.classList.toggle("active",button.dataset.crop===orientation));zoom.value=String(Math.round(crop.scale*100));zoom.style.setProperty("--val",((crop.scale-1)/3*100)+"%");
    }
    $$("[data-crop]").forEach(button=>button.onclick=()=>{orientation=button.dataset.crop;draw();});
    zoom.oninput=()=>{crops[orientation].scale=Number(zoom.value)/100;draw();};
    preview.addEventListener("pointerdown",event=>{pointers.set(event.pointerId,{x:event.clientX,y:event.clientY});preview.setPointerCapture?.(event.pointerId);lastPinch=0;});
    preview.addEventListener("pointermove",event=>{
      const old=pointers.get(event.pointerId);if(!old)return;pointers.set(event.pointerId,{x:event.clientX,y:event.clientY});event.preventDefault();const crop=crops[orientation];
      if(pointers.size===2){const [a,b]=[...pointers.values()],dist=Math.hypot(a.x-b.x,a.y-b.y);if(lastPinch>0)crop.scale=Math.max(1,Math.min(4,crop.scale*dist/lastPinch));lastPinch=dist;}
      else {const cover=Math.max(preview.clientWidth/img.naturalWidth,preview.clientHeight/img.naturalHeight)*crop.scale;crop.centerX=Math.max(0,Math.min(1,crop.centerX-(event.clientX-old.x)/(img.naturalWidth*cover)));crop.centerY=Math.max(0,Math.min(1,crop.centerY-(event.clientY-old.y)/(img.naturalHeight*cover)));}
      draw();
    });
    for(const name of ["pointerup","pointercancel"])preview.addEventListener(name,event=>{pointers.delete(event.pointerId);lastPinch=0;});
    $("#crop-cancel").onclick=()=>closeDialog();
    $("#crop-save").onclick=()=>{
      if(isNew){const output=document.createElement("canvas"),scale=Math.min(1,2048/Math.max(img.naturalWidth,img.naturalHeight));output.width=Math.round(img.naturalWidth*scale);output.height=Math.round(img.naturalHeight*scale);output.getContext("2d").drawImage(img,0,0,output.width,output.height);
        try{const data=output.toDataURL("image/jpeg",.88);if(data.length>3000000){toast("图片过大，请换一张较小的图片");return;}ui.wallData=data;}catch(_){toast("当前宿主无法处理这张图片");return;}
      }
      ui.portrait=crops.portrait;ui.landscape=crops.landscape;saveUI();applyTheme();closeDialog();toast("壁纸取景已保存");
    };
    draw();
  }

  /* ---------------- 弹窗 ---------------- */
  let dialogFocus = null, dialogCloseTimer = 0;
  /* 来源重解析：列表渲染会重建行元素，关闭时按选择器找回最新的来源行，
     morph 回到它的当前位置；行已被删除/筛除时自然降级（收拢到原位小态）。 */
  let dialogAnchorQuery = null;
  function dialogAnchor() {
    if (dialogFocus?.isConnected) return dialogFocus;
    if (dialogAnchorQuery) {
      const row = document.querySelector(dialogAnchorQuery);
      if (row) return row;
    }
    return dialogFocus;
  }
  function openDialog(bodyHtml) {
    const epoch=++dialogEpoch,reopening=$("#dialog-layer").classList.contains("closing");
    motions.get($(".dialog"))?.cancel();clearTimeout(dialogCloseTimer);$("#dialog-layer").classList.remove("closing");
    const firstOpen=!$("#dialog-layer").classList.contains("open");
    if(firstOpen&&history.state?.overlay!=="dialog")rememberRoute("dialog");
    $(".dialog").classList.remove("crop-dialog");
    const layer = $("#dialog-layer");
    if (!layer.classList.contains("open")) dialogFocus = document.activeElement;
    $("#dialog-body").innerHTML = `<button class="dialog-close" id="dialog-close" type="button" aria-label="关闭弹窗">×</button>` + bodyHtml;
    layer.classList.add("open"); layer.setAttribute("aria-hidden", "false");
    syncInert();
    $("#dialog-close").onclick = () => closeDialog();
    window.NovaGlass?.scan(layer);
    if(firstOpen||reopening)morph($(".dialog"),dialogAnchor());else ownedMotion($("#dialog-body"),[{opacity:.45,transform:"translateY(5px)"},{opacity:1,transform:"none"}],{duration:180,easing:"ease-out",fill:"both"});
    window.dispatchEvent(new Event("nova:dialog"));
    requestAnimationFrame(() => { if(epoch===dialogEpoch&&layer.classList.contains("open")) ($("#dialog-input") || $("#dialog-close")).focus(); });
    return epoch;
  }
  function closeDialog(fromHistory=false,immediate=false,startState=null){
    const layer=$("#dialog-layer");if(!layer.classList.contains("open")||layer.classList.contains("closing")&&!immediate)return;
    const epoch=++dialogEpoch;motions.get($(".dialog"))?.cancel();clearTimeout(dialogCloseTimer);
    const finish=finished=>{if(!finished||epoch!==dialogEpoch)return;layer.classList.remove("open","closing");layer.setAttribute("aria-hidden","true");syncInert();if(dialogFocus?.isConnected&&!dialogFocus.closest?.("[inert],[aria-hidden=true]"))dialogFocus.focus();window.dispatchEvent(new Event("nova:dialog"));if(!fromHistory&&history.state?.overlay==="dialog")history.back();};
    // 关闭从当前实际状态开始，回到来源的最新有效位置（行元素被列表
    // 渲染重建后按选择器重解析）；来源消失时自然收拢，不飞向错误位置。
    layer.classList.add("closing");if(immediate||reducedMotion())finish(true);else morph($(".dialog"),dialogAnchor(),false,finish,startState);
  }
  /* 标题区拖拽关闭：拖多少动多少；按位移与释放速度判定完成或回弹；
     回弹从当前拖拽位置弹簧续接（保留动量方向），取消一律回弹。
     仅接管标题区，正文滚动不受影响；系统返回手势与历史路由共用
     closeDialog 这一个退出状态机。 */
  let dialogDrag=null;
  $("#dialog-body").addEventListener("pointerdown",event=>{
    if(event.button>0||dialogDrag||!$("#dialog-layer").classList.contains("open"))return;
    if(!event.target.closest("h3,.eyebrow,.dialog-close"))return;
    dialogDrag={id:event.pointerId,y:event.clientY,dy:0,lastY:event.clientY,lastT:performance.now(),v:0};
    event.target.setPointerCapture?.(event.pointerId);
  });
  $("#dialog-body").addEventListener("pointermove",event=>{
    if(!dialogDrag||event.pointerId!==dialogDrag.id)return;
    const now=performance.now(),dt=now-dialogDrag.lastT;
    dialogDrag.dy=Math.max(0,event.clientY-dialogDrag.y);
    if(dt>0&&dt<120){dialogDrag.v=(event.clientY-dialogDrag.lastY)/dt;dialogDrag.lastY=event.clientY;dialogDrag.lastT=now;}
    else if(dt>=120){dialogDrag.v=0;dialogDrag.lastY=event.clientY;dialogDrag.lastT=now;}
    if(dialogDrag.dy>6){event.preventDefault();$(".dialog").style.transform=`translateY(${Math.min(140,dialogDrag.dy)}px)`;window.NovaGlass?.invalidate();}
  });
  function endDialogDrag(event,cancelled=false){
    if(!dialogDrag||event.pointerId!==dialogDrag.id)return;
    const drag=dialogDrag;dialogDrag=null;
    const dialog=$(".dialog");
    const fling=!cancelled&&drag.dy>0&&drag.v>0.45;
    if(!cancelled&&(drag.dy>65||fling)){
      dialog.style.transform="";
      closeDialog(false,false,{dx:0,dy:drag.dy,sx:1,sy:1,op:1});
    }else if(drag.dy>6){
      dialog.style.transform="";
      const {frames,duration}=springKeyframes(0,m=>({transform:`translateY(${drag.dy*(1-m)}px)`,opacity:1}));
      ownedMotion(dialog,frames,{duration,fill:"both"});
    }
    window.NovaGlass?.invalidate(360);
  }
  $("#dialog-body").addEventListener("pointerup",e=>endDialogDrag(e));
  $("#dialog-body").addEventListener("pointercancel",e=>endDialogDrag(e,true));
  $("#dialog-layer").addEventListener("click", event => {
    if (event.target === $("#dialog-layer")) closeDialog();
  });
  document.addEventListener("keydown", event => {
    if (!$("#dialog-layer").classList.contains("open")) {
      if(event.key==="Escape") { if($("#appearance-layer").classList.contains("open"))closePersonalize(); }
      if(event.key==="Tab"&&$("#appearance-layer").classList.contains("open"))trapFocus(event,$("#personal-panel"));
      return;
    }
    if (event.key === "Escape") { closeDialog(); return; }
    if (event.key !== "Tab") return;
    trapFocus(event,$("#dialog-body"));
  });

  function showRuleEditor(pkg = "") {
    if (!editable() || s.pending) { toast(s.pending ? "上一次操作正在确认，请稍候" : s.external ? "当前由其它调度器接管，请先处理 Scene 调度来源" : "请先连接到模块服务"); return; }
    const existing = s.rules.find(rule => rule.package === pkg);
    const a = pkg ? appInfo(pkg) : null;
    let selected = existing?.mode || s.mode || "balance";
    if (!own(MODES, selected)) selected = "balance";
    // 图形后端当前值：独立存储，缺省“跟随系统”。
    probeGraphics();
    // 每次打开都强制重探：他处（CLI/另一会话）增删的规则不能显示陈旧值。
    probeFps(true);
    probeRefresh(true);
    const graphicsValue = s.graphics.rules[pkg] || "follow";
    const angleUsable = s.graphics.available === true;
    const cap = s.graphics.capability;
    const vulkanBlockedReason = !cap ? "图形能力未知（root 通道未就绪）"
      : !cap.vulkan.length ? "未检测到 Vulkan HAL 描述文件（/system|/vendor/etc/vulkan）"
      : "Android 公开的逐应用驱动入口仅接受 ANGLE/原生；强制 Vulkan 需应用 manifest 自带声明，外部无安全开关——选择后将显示真实 unsupported 原因";
    const vulkanUsable = !vulkanBlockedReason;
    const hasRoot = typeof bridge()?.exec === "function";
    const fpsValue = (s.fpsRules || {})[pkg] || 0;
    const refreshValue = (s.refreshRules || {})[pkg] || 0;
    const FPS_OPTIONS = [[0, "跟随应用"], [30, "30"], [45, "45"], [60, "60"], [90, "90"], [120, "120"]];
    // 面板刷新率选项：只来自设备真实模式列表；解析不到时整行禁用并说明。
    const REFRESH_OPTIONS = [[0, "跟随系统"], ...s.refreshHzList.map(hz => [hz, String(hz)])];
    const refreshDisabled = !hasRoot || (s.refreshRules !== null && !s.refreshHzList.length);
    openDialog(`
      <p class="eyebrow">${existing ? "编辑规则" : "新建规则"}</p>
      <h3 id="dialog-title">${existing ? esc(a.name) : "添加应用规则"}</h3>
      <p class="dialog-sub">${s.linked ? "规则会同步到 Scene，并在应用进入前台时执行。" : "本地自动调度关闭时，应用规则优先于默认档位。"}</p>
      <label for="dialog-input">应用包名</label>
      <input type="text" id="dialog-input" value="${esc(pkg)}" placeholder="com.example.app" autocapitalize="off" autocorrect="off" spellcheck="false" maxlength="255" ${existing ? "readonly" : ""}>
      <p class="input-error" id="dialog-error" hidden></p>
      <label>目标档位</label>
      <div class="mode-chips" role="group" aria-label="目标档位">
        ${Object.entries(MODES).map(([mode, info]) => `<button type="button" data-chip="${mode}" class="${mode === selected ? "active" : ""}">${info.name}</button>`).join("")}
      </div>
      <label>图形后端（逐应用 Vulkan 管理）</label>
      <div class="mode-chips" role="group" aria-label="图形后端">
        <button type="button" data-gfx="follow" class="${graphicsValue === "follow" ? "active" : ""}">跟随系统</button>
        <button type="button" data-gfx="auto" class="${graphicsValue === "auto" ? "active" : ""}">自动</button>
        <button type="button" data-gfx="angle" class="${graphicsValue === "angle" ? "active" : ""}" ${angleUsable && hasRoot ? "" : "disabled"}>ANGLE</button>
        <button type="button" data-gfx="native" class="${graphicsValue === "native" ? "active" : ""}">原生</button>
        <button type="button" data-gfx="vulkan" class="${graphicsValue === "vulkan" ? "active" : ""}" ${vulkanUsable && hasRoot ? "" : "disabled"} title="${esc(vulkanBlockedReason)}">Vulkan</button>
      </div>
      <p class="dialog-sub" id="graphics-note" style="font-size:11px;">${
        !hasRoot ? "图形后端需要 root 备用通道；当前不可用，档位规则仍可保存。"
        : angleUsable ? "ANGLE=官方转译层（逐应用可用）；原生=驱动原生 GL；自动=按能力解析；Vulkan 档显示真实不支持原因（Android 无公开逐应用强制入口）。保存后应用需完全退出重开。"
        : (s.graphics.reason || "图形后端能力未知") + "；ANGLE 不可用时仍可选自动/原生/Vulkan（按能力降级）。"
      }</p>
      <label>目标 FPS（调度目标）</label>
      <div class="mode-chips" role="group" aria-label="目标 FPS">
        ${FPS_OPTIONS.map(([value, label]) => `<button type="button" data-fps="${value}" class="${fpsValue === value ? "active" : ""}" ${hasRoot ? "" : "disabled"}>${label}</button>`).join("")}
      </div>
      <p class="dialog-sub" id="fps-note" style="font-size:11px;">${
        hasRoot
          ? "调度目标：NovaFAS 以此判断帧率是否达标、是否需要加压（应用达不到目标时才提升 CPU）。不是强制限帧，不改变游戏渲染上限。"
          : "目标 FPS 需要 root 备用通道；当前不可用。"
      }</p>
      <label>面板刷新率（Scene 同款）</label>
      <div class="mode-chips" role="group" aria-label="面板刷新率">
        ${REFRESH_OPTIONS.map(([value, label]) => `<button type="button" data-refresh="${value}" class="${refreshValue === value ? "active" : ""}" ${refreshDisabled ? "disabled" : ""}>${label}</button>`).join("")}
      </div>
      <p class="dialog-sub" id="refresh-note" style="font-size:11px;">${
        !hasRoot ? "面板刷新率需要 root 备用通道；当前不可用。"
        : refreshDisabled ? "未能从设备解析出可用的显示模式列表，入口保持可见但暂不可用。"
        : "前台进入该应用时把面板切到目标刷新率（SurfaceFlinger 激活显示模式，与 Scene 同一机制）；离开后回到启动时的原模式。这不是强制帧率上限，不改变游戏渲染目标。"
      }</p>
      <div class="dialog-actions">
        <button class="btn-capsule ghost" type="button" id="dialog-cancel">取消</button>
        <button class="btn-capsule accent" type="button" id="dialog-save">保存规则</button>
      </div>
      ${existing ? '<button class="danger-link" type="button" id="dialog-delete">删除这条规则</button>' : ""}
    `);
    // 关闭时按选择器重解析来源行：列表刷新重建 DOM 后仍回到最新位置。
    dialogAnchorQuery = pkg ? `[data-edit="${pkg}"]` : "#add-rule";
    // 探测可能在对话框打开后才返回：返回后就地刷新 ANGLE 可用态与说明，
    // 不要求用户重开对话框。
    const editorEpoch = dialogEpoch;
    // 连接期探测可能尚未返回：强制重探一次（一条只读命令），返回后就地
    // 刷新 ANGLE 可用态与说明，不要求用户重开对话框。
    probeGraphics(true).then(g => {
      if (editorEpoch !== dialogEpoch) return;
      const angleButton = $('[data-gfx="angle"]');
      const vulkanChip = $('[data-gfx="vulkan"]');
      const note = $("#graphics-note");
      if (!angleButton || !note) return;
      angleButton.disabled = !(g.available === true && hasRoot);
      const vulkanBlocked = !g.capability || !g.capability.vulkan.length;
      if (vulkanChip) {
        vulkanChip.disabled = vulkanBlocked && hasRoot ? true : !hasRoot;
        vulkanChip.title = vulkanBlocked
          ? "Vulkan 档：真实 unsupported 原因链（Android 无公开逐应用强制 Vulkan 入口）"
          : "Vulkan 档：设备支持 Vulkan，但 Android 公开入口仍仅接受 ANGLE/原生——将显示如实原因";
      }
      note.textContent =
        !hasRoot ? "图形后端需要 root 备用通道；当前不可用，档位规则仍可保存。"
        : g.available === true ? "ANGLE=官方转译层（逐应用）；自动=按能力解析；Vulkan 档显示真实 unsupported 原因链。写入后应用需完全退出重开生效。"
        : (g.reason || "图形后端能力未知") + "；ANGLE 不可用时仍可选自动/原生/Vulkan（按能力降级）。";
    });
    let mode = selected;
    let graphics = graphicsValue;
    let fpsTarget = fpsValue;
    let refreshTarget = refreshValue;
    $$("#dialog-body [data-chip]").forEach(button => button.onclick = () => {
      mode = button.dataset.chip;
      $$("#dialog-body [data-chip]").forEach(node => node.classList.toggle("active", node === button));
    });
    $$("#dialog-body [data-gfx]").forEach(button => button.onclick = () => {
      graphics = button.dataset.gfx;
      $$("#dialog-body [data-gfx]").forEach(node => node.classList.toggle("active", node === button));
    });
    $$("#dialog-body [data-fps]").forEach(button => button.onclick = () => {
      fpsTarget = Number(button.dataset.fps);
      $$("#dialog-body [data-fps]").forEach(node => node.classList.toggle("active", node === button));
    });
    $$("#dialog-body [data-refresh]").forEach(button => button.onclick = () => {
      refreshTarget = Number(button.dataset.refresh);
      $$("#dialog-body [data-refresh]").forEach(node => node.classList.toggle("active", node === button));
    });
    $("#dialog-cancel").onclick = () => closeDialog();
    $("#dialog-save").onclick = () => {
      const value = $("#dialog-input").value.trim();
      const error = $("#dialog-error");
      if (!validPackage(value)) {
        error.hidden = false;
        error.textContent = "请输入有效的主应用包名，例如 com.example.app。";
        return;
      }
      // 图形后端独立保存：与档位规则分属两个存储，互不覆盖。
      // 仅在用户显式修改时提交；跟随系统 → graphics-remove（只撤销本模块条目）。
      if (graphics !== (s.graphics.rules[value] || "follow") && hasRoot) {
        applyGraphics(value, graphics).then(outcome => toast(outcome.message, outcome.ok ? 3200 : 4600));
      }
      // 目标 FPS 独立保存（fps_rules.json）；0 = 跟随应用/系统 → 移除。
      if (fpsTarget !== ((s.fpsRules || {})[value] || 0) && hasRoot) {
        applyFps(value, fpsTarget).then(outcome => toast(outcome.message, outcome.ok ? 3200 : 4600));
      }
      // 面板刷新率独立保存（refresh_rules.json）；0 = 跟随系统 → 移除。
      if (refreshTarget !== ((s.refreshRules || {})[value] || 0) && hasRoot && !refreshDisabled) {
        applyRefresh(value, refreshTarget).then(outcome => toast(outcome.message, outcome.ok ? 3200 : 4600));
      }
      if (beginPending("rule", mode, value)) { error.hidden = true; }
    };
    if (existing) $("#dialog-delete").onclick = () => showDeleteConfirm(pkg);
  }
  function showDeleteConfirm(pkg) {
    const a = appInfo(pkg);
    openDialog(`
      <p class="eyebrow">删除规则</p>
      <h3 id="dialog-title">删除「${esc(a.name)}」的规则？</h3>
      <p class="dialog-sub">删除后，这个应用沿用默认档位。其他应用规则保留。</p>
      <p class="dialog-sub mono" style="font-size:11px;">${esc(pkg)}</p>
      <div class="dialog-actions">
        <button class="btn-capsule ghost" type="button" id="dialog-cancel">返回编辑</button>
        <button class="btn-capsule" type="button" id="dialog-confirm" style="background:rgba(255,69,58,.18);color:var(--err);">删除规则</button>
      </div>
    `);
    $("#dialog-cancel").onclick = () => showRuleEditor(pkg);
    $("#dialog-confirm").onclick = () => {
      if (!beginPending("delete", null, pkg)) return;
      $("#dialog-confirm").disabled = true;
      // 删除规则同时撤销本模块写入的图形后端覆盖与刷新率规则
      // （独立存储、只动自己的条目）。
      if (s.graphics.rules[pkg] && typeof bridge()?.exec === "function") applyGraphics(pkg, "follow");
      if ((s.refreshRules || {})[pkg] && typeof bridge()?.exec === "function") applyRefresh(pkg, 0);
    };
  }
  $("#rule-list").addEventListener("click", event => {
    const row = event.target.closest("[data-edit]");
    if (row) showRuleEditor(row.dataset.edit);
  });
  $$("#page-history .history-range button").forEach(button => button.onclick = () => {
    historyRange = Number(button.dataset.range) || 24;
    $$("#page-history .history-range button").forEach(node => node.classList.toggle("active", node === button));
    loadHistory();
  });
  {
    const canvas = $("#history-canvas");
    canvas.addEventListener("pointermove", event => {
      if (!canvas._historyMeta || !historyPoints.length) return;
      const rect = canvas.getBoundingClientRect();
      const meta = canvas._historyMeta;
      const t = meta.t0 + (event.clientX - rect.left - 8) / (rect.width - 16) * (meta.t1 - meta.t0);
      let best = null, bestDist = Infinity;
      for (const p of historyPoints) {
        const dist = Math.abs(p.t - t);
        if (dist < bestDist) { bestDist = dist; best = p; }
      }
      if (!best) return;
      const tip = $("#history-tip");
      // 第二阶段·闭环反馈：同一时间线对照"加压幅度 ↔ 实际帧率"——
      // 有加压没帧收益的时间段在气泡里直接可见。
      const boostText = Number(best.boost) > 0 ? ` · 加压 ${best.boost}%` : "";
      if (tip) tip.textContent = `${new Date(best.t * 1000).toLocaleTimeString()} · ${best.pkg || "无前台"} · ${best.mode}档 · 电量 ${best.battery ?? "—"}% · ${best.charging ? "充电中" : "未充电"} · 温度 ${best.temp ?? "—"}℃${best.fps != null ? ` · ${best.fps.toFixed(1)} FPS` : ""}${boostText}`;
    });
  }
  /* 清空确认后的回执：收到规则为空即关弹窗并提示条数（超时兜底）。 */
  let pendingClear = null;
  function settleClear() {
    if (!pendingClear) return;
    const removable = s.rules.filter(rule => rule.package !== "*").length;
    if (removable === 0) {
      const removed = pendingClear.count;
      pendingClear = null;
      if ($("#dialog-layer").classList.contains("open")) closeDialog();
      toast(`已删除 ${removed} 条按应用规则（默认档位保留）`);
      return;
    }
    if (Date.now() - pendingClear.at > 6000) {
      pendingClear = null;
      if ($("#dialog-layer").classList.contains("open")) closeDialog();
      toast("未收到清空确认，请在规则页确认结果");
    }
  }
  $("#add-rule").onclick = () => showRuleEditor("");
  /* 一键清空按应用规则：二次确认（不可撤销），保留默认档位 "*"。 */
  $("#rules-clear")?.addEventListener("click", () => {
    const removable = s.rules.filter(rule => rule.package !== "*").length;
    if (!removable) { toast("没有可删除的按应用规则"); return; }
    if (!editable()) { toast(s.external ? "Scene 外部调度器接管中，暂不能修改规则" : "请先连接本地服务"); return; }
    NovaHaptics.select();
    openDialog(`
      <p class="eyebrow">清空规则</p>
      <h3 id="dialog-title">删除全部 ${removable} 条应用规则？</h3>
      <p class="dialog-sub">只删除按应用规则，默认档位与其它设置保留；删除后无法恢复，需要重新添加。</p>
      <div class="dialog-actions">
        <button class="btn-capsule ghost" type="button" id="dialog-cancel">取消</button>
        <button class="btn-capsule" type="button" id="dialog-confirm" style="background:rgba(255,69,58,.18);color:var(--err);">全部删除</button>
      </div>
    `);
    $("#dialog-cancel").onclick = () => closeDialog();
    $("#dialog-confirm").onclick = () => {
      const button = $("#dialog-confirm");
      if (button) button.disabled = true;
      if (!send("apps", "clear")) { closeDialog(); return; }
      pendingClear = { at: Date.now(), count: removable };
      if (typeof NovaHaptics !== "undefined") setTimeout(() => NovaHaptics.success(), 220);
    };
  });

  /* ---------------- 捐赠与欢迎 ---------------- */
  const WELCOME_KEY = "novasched.welcome.v280";
  function showDonate() {
    openDialog(`
      <p class="eyebrow">SUPPORT</p>
      <h3 id="dialog-title">请作者喝杯奶茶</h3>
      <p class="dialog-sub">NovaSched 与 NovaFAS 完全免费开源，没有任何解锁或付费项。如果调度够稳、游戏够顺、续航有改善，欢迎请作者喝一杯奶茶——每一份心意都是逐颗处理器继续打磨的动力。</p>
      <p class="dialog-sub">捐赠完全自愿，不捐也丝毫不影响使用；提交反馈和日志同样是最好的支持。</p>
      <div class="donate-grid">
        <figure class="donate-item"><img src="./assets/donate/wechat.jpg" alt="微信赞赏码" loading="lazy"><figcaption>微信 · 赞赏码</figcaption></figure>
        <figure class="donate-item"><img src="./assets/donate/alipay.jpg" alt="支付宝收款码" loading="lazy"><figcaption>支付宝 · 收款码</figcaption></figure>
      </div>
      <div class="dialog-actions"><button class="btn-capsule ghost" type="button" id="dialog-cancel">关闭</button></div>
    `);
    $("#dialog-cancel").onclick = () => closeDialog();
  }
  function maybeShowWelcome() {
    let seen = false;
    try { seen = localStorage.getItem(WELCOME_KEY) === "1"; } catch (_) { return; }
    if(seen||!s.online||document.hidden||currentPage!=="overview"||$(".appearance-layer.open,.dialog-layer.open"))return;
    try { localStorage.setItem(WELCOME_KEY, "1"); } catch (_) { return; }
    openDialog(`
      <p class="eyebrow">WELCOME</p>
      <h3 id="dialog-title">NovaSched v${UI_VERSION}</h3>
      <p class="dialog-sub">四档策略已按你的处理器逐频点适配；Scene 联动时，省电 / 均衡 / 性能 / 极速四个档位都由本模块执行下发。</p>
      <p class="dialog-sub">NovaFAS 接管 Scene FAS / FEAS / FAS Lite 的职责：掉帧快速加压、稳定缓慢回落。请关闭其它独立的帧调度器，避免两个帧调度器争抢节点。</p>
      <p class="dialog-sub">概览页可查看当前档位、前台应用与各簇频率；连接异常时，可从设置读取诊断。</p>
      <div class="dialog-actions">
        <button class="btn-capsule ghost" type="button" id="welcome-donate">支持开发</button>
        <button class="btn-capsule accent" type="button" id="welcome-close">开始使用</button>
      </div>
    `);
    $("#welcome-close").onclick = () => closeDialog();
    $("#welcome-donate").onclick = () => showDonate();
  }
  let welcomeShown = false;

  /* ---------------- 云端更新 ---------------- */
  function updateScriptCommand(args) {
    const candidates = [...new Set([MODULE, ...MODULE_CANDIDATES])].map(shellQuote).join(" ");
    return `for NOVA_PATH in ${candidates}; do if [ -f "$NOVA_PATH/bin/cloud-update.sh" ]; then exec sh "$NOVA_PATH/bin/cloud-update.sh" ${args}; fi; done; printf '%s\\n' 'UPDATE_FAIL=未找到更新脚本，请完整安装模块'; exit 1`;
  }
  function parseUpdateKeys(output) {
    const keys = {};
    for (const line of String(output || "").split(/\r?\n/)) {
      const i = line.indexOf("=");
      if (i > 0) keys[line.slice(0, i)] = line.slice(i + 1);
    }
    return keys;
  }
  function showUpdate() {
    const epoch=openDialog(`
      <p class="eyebrow">UPDATE</p>
      <h3 id="dialog-title">检查更新</h3>
      <p class="dialog-sub" id="update-status">当前版本 v${UI_VERSION}。检查需要联网，下载走直连与镜像自动回退，SHA256 校验通过后才安装。</p>
      <div class="dialog-actions" id="update-actions">
        <button class="btn-capsule ghost" type="button" id="dialog-cancel">关闭</button>
        <button class="btn-capsule accent" type="button" id="update-check" ${typeof bridge()?.exec === "function" ? "" : "disabled"}>检查最新版本</button>
      </div>
      <pre class="report" id="update-report" hidden></pre>
    `);
    $("#dialog-cancel").onclick = () => closeDialog();
    $("#update-check").onclick = () => {
      NovaHaptics.select();
      const status = $("#update-status"), actions = $("#update-actions");
      $("#update-check").disabled = true;
      status.textContent = "正在连接 GitHub 获取最新版本…";
      const restore = (message, error = true) => {
        if(epoch!==dialogEpoch||!status.isConnected)return;
        status.textContent = message;
        const check = $("#update-check");
        if (check) { check.disabled = false; check.textContent = "重新检查"; }
        if (error) NovaHaptics.failure();
      };
      rootExec(updateScriptCommand("check"), 90000).then(result => {
        if(epoch!==dialogEpoch||!status.isConnected)return;
        const keys = parseUpdateKeys(result.stdout);
        if (keys.UPDATE_FAIL) return restore("检查失败：" + keys.UPDATE_FAIL);
        if(!/^v[0-9]+\.[0-9]+\.[0-9]+$/.test(keys.LATEST||""))return restore("返回数据不完整，请稍后再试");
        if (keys.UPDATE_OK === "latest" || !keys.UPDATE_AVAILABLE) {
          NovaHaptics.success();
          status.textContent = `已是最新版本（当前 v${UI_VERSION}，最新 ${keys.LATEST}）。`;
          return;
        }
        NovaHaptics.success();
        status.textContent = `发现新版本 ${keys.LATEST}（当前 v${UI_VERSION}）。校验通过后自动安装，完成后需要重启。`;
        actions.innerHTML = `
          <button class="btn-capsule ghost" type="button" id="update-open">打开 Release 页</button>
          <button class="btn-capsule accent" type="button" id="update-install">下载并安装 ${keys.LATEST}</button>`;
        $("#update-open").onclick = () => { try { window.open(`https://github.com/zeng060812-creator/NovaSched-Zen-Edition/releases/tag/${keys.LATEST}`, "_blank"); } catch (_) { toast("当前宿主无法打开浏览器"); } };
        $("#update-install").onclick = () => {
          NovaHaptics.toggle();
          actions.innerHTML = `<button class="btn-capsule ghost" type="button" id="dialog-cancel2">关闭</button>`;
          $("#dialog-cancel2").onclick = () => closeDialog();
          status.textContent = "正在下载、校验并安装，请保持网络畅通与屏幕亮起…";
          rootExec(updateScriptCommand(`install ${shellQuote(keys.LATEST)}`), 420000).then(installResult => {
            if(epoch!==dialogEpoch||!status.isConnected)return;
            const done = parseUpdateKeys(installResult.stdout);
            const report = $("#update-report");
            if(report?.isConnected){report.hidden=false;report.textContent=(installResult.stdout || "").trim(); }
            if (done.UPDATE_OK && done.UPDATE_OK !== "latest") {
              NovaHaptics.success();
              status.textContent = `安装完成（${done.MANAGER || "已写入 modules_update"}）。请重启手机生效。`;
            } else {
              restore("更新失败：" + (done.UPDATE_FAIL || "未知原因，详情见下方输出"));
            }
          }).catch(error => restore("更新失败：" + publicError(error.message)));
        };
      }).catch(error => restore("检查失败：" + publicError(error.message)));
    };
  }

  function showDiagnostics(sceneOnly = false) {    const command = sceneOnly ? "scene-diagnose" : s.online ? "game-diagnose" : "diagnose";
    openDialog(`
      <p class="eyebrow">${sceneOnly ? "SCENE" : "DIAGNOSTICS"}</p>
      <h3 id="dialog-title">${sceneOnly ? "Scene 检测详情" : "运行诊断"}</h3>
      <p class="dialog-sub">${sceneOnly ? "检测到 Scene 后自动备份并接管调度入口。重新注册会替换当前调度脚本，卸载时恢复首次备份。" : s.online ? "读取服务与节点状态，采样约 10 秒。" : "读取启动记录。服务未启动时可点「启动服务」，错误会显示在下方。"}</p>
      <div class="dialog-actions"><button class="btn-capsule ghost" id="dialog-copy" type="button">复制命令</button><button class="btn-capsule accent" id="dialog-run" type="button" ${typeof bridge()?.exec === "function" ? "" : "disabled"}>读取诊断</button></div>
      ${!sceneOnly && !s.online ? `<button class="btn-capsule ghost" id="service-start" type="button" ${typeof bridge()?.exec === "function" ? "" : "disabled"}>启动服务</button>` : ""}
      ${sceneOnly ? `<button class="btn-capsule ghost" id="scene-register" type="button" ${typeof bridge()?.exec === "function" ? "" : "disabled"}>重新检测并注册</button>` : ""}
      <pre class="report" id="dialog-report" hidden></pre>
    `);
    const report = $("#dialog-report");
    if (!s.online && s.connectionError) { report.hidden = false; report.textContent = s.connectionError; }
    const run = async (button, register) => {
      button.disabled = true; button.textContent = "读取中…";
      try {
        let output = "";
        for (const action of register ? ["scene-install", "reload", command] : [command]) {
          const result = await rootExec(moduleCommand(action), 40000, !["scene-install", "reload"].includes(action));
          output += (result.stdout || "") + (result.stderr ? "\n" + result.stderr : "") + "\n";
          if (result.errno !== 0) output += `退出码：${result.errno}\n`;
        }
        if (report.isConnected) { report.hidden = false; report.textContent = output.trim() || "诊断未返回内容"; }
      } catch (error) {
        if (report.isConnected) { report.hidden = false; report.textContent = error.message; }
      } finally {
        if (button.isConnected) { button.disabled=false; button.textContent=register ? "重新检测并注册" : "重新读取诊断"; }
      }
    };
    $("#dialog-copy").onclick = () => copyText(`su -c ${MODULE}/bin/novasched ${command}`);
    $("#dialog-run").onclick = event => run(event.currentTarget, false);
    if (sceneOnly) $("#scene-register").onclick = event => run(event.currentTarget, true);
    const start = $("#service-start");
    if (start) start.onclick = async () => {
      start.disabled = true; start.textContent = "正在启动…";
      try {
        const result = await rootExec(moduleCommand("start"), 140000, false);
        if (report.isConnected) {
          report.hidden = false;
          report.textContent = publicError((result.stdout || "") + "\n" + (result.stderr || "")) || `启动退出码：${result.errno}`;
        }
        if (result.errno === 0) discover();
      } catch (error) {
        if (report.isConnected) { report.hidden = false; report.textContent = publicError(error.message); }
      } finally {
        if (start.isConnected) { start.disabled = false; start.textContent = "启动服务"; }
      }
    };
  }

  /* ---------------- 控件事件 ---------------- */
  $$("#mode-grid .mode-card").forEach(button => button.onclick = () => {
    NovaHaptics.select();
    if (s.automatic || s.automaticActive) { beginPending("mode", button.dataset.mode); return; }
    if (!s.automatic && s.phase === "ready" && button.dataset.mode === s.mode && button.dataset.mode === s.effective) { toast("当前档位已生效"); return; }
    beginPending("mode", button.dataset.mode);
  });
  /* switch 是 button[role=switch]，没有原生 change 事件；统一用 click 翻转 */
  $("#auto-switch").onclick = event => {
    const on = event.currentTarget.getAttribute("aria-checked") !== "true";
    NovaHaptics.toggle();
    beginPending("auto", on ? "on" : "off");
    render();
  };
  $("#extreme-switch").onclick = event => {
    if (s.extremePending !== null) return;
    const enabled = event.currentTarget.getAttribute("aria-checked") !== "true";
    NovaHaptics.toggle();
    const previousStatus = s.extremeStatus;
    s.extremePending = enabled;
    s.extremeStatus = "applying";
    s.extremeError = "";
    if (!send("modes", `extreme\t${enabled ? "1" : "0"}`)) {
      s.extremePending = null;
      s.extremeStatus = previousStatus;
      render();
      return;
    }
    render();
  };
  $("#scene-diagnose").onclick = () => showDiagnostics(true);
  $("#open-graphics")?.addEventListener("click", () => { NovaHaptics.select(); setPage("graphics"); });
  $("#ai-settings-entry")?.addEventListener("click", () => { NovaHaptics.select(); setPage("ai"); });
  /* DeepSeek 页：三态切换 / Key / 模型 / 接口 / 测试 / 恢复本地调度。 */
  $$("#page-ai [data-ai-mode]").forEach(button => button.onclick = () => {
    NovaHaptics.select();
    aiApply(rootExec(moduleCommand(`ai-mode ${shellQuote(button.dataset.aiMode)}`), 8000, false),
      { off: "已关闭：完全本地调度", observe: "观察模式：只分析建议，不修改参数", auto: "自动模式：建议经本地验证后参与调度" }[button.dataset.aiMode]);
  });
  $("#ai-key-eye")?.addEventListener("click", () => {
    const input = $("#ai-key-input");
    if (input) input.type = input.type === "password" ? "text" : "password";
  });
  $("#ai-key-save")?.addEventListener("click", () => {
    const value = ($("#ai-key-input")?.value || "").trim();
    if (!value) { toast("请输入 API Key"); return; }
    NovaHaptics.select();
    aiApply(rootExec(moduleCommand(`ai-key set ${shellQuote(value)}`), 8000, false), "Key 已保存（仅设备本地 0600）")
      .then(ok => { if (ok) $("#ai-key-input").value = ""; });
  });
  $("#ai-key-clear")?.addEventListener("click", () => {
    NovaHaptics.select();
    aiApply(rootExec(moduleCommand("ai-key clear"), 8000, false), "Key 已清除")
      .then(ok => { if (ok) $("#ai-key-input").value = ""; });
  });
  $("#ai-test")?.addEventListener("click", async event => {
    const button = event.currentTarget;
    NovaHaptics.select();
    button.disabled = true;
    const result = $("#ai-test-result");
    if (result) result.textContent = "连接测试中…（会消耗一次极小请求）";
    try {
      const outcome = await rootExec(moduleCommand("ai-test"), 40000, false);
      const text = `${outcome.stdout || ""}\n${outcome.stderr || ""}`.trim();
      const ok = outcome.errno === 0;
      if (result) { result.textContent = text.slice(0, 200) || (ok ? "成功" : "失败"); result.dataset.ok = ok ? "1" : "0"; }
      ok ? NovaHaptics.success() : NovaHaptics.failure();
      probeAi(true);
    } catch (error) {
      if (result) result.textContent = `失败：${String(error?.message || error).slice(0, 140)}`;
      NovaHaptics.failure();
    } finally { button.disabled = false; }
  });
  $("#ai-model-save")?.addEventListener("click", () => {
    const value = ($("#ai-model-input")?.value || "").trim();
    if (!value) { toast("请输入模型名"); return; }
    aiApply(rootExec(moduleCommand(`ai-model ${shellQuote(value)}`), 8000, false), "模型已保存");
  });
  $("#ai-url-save")?.addEventListener("click", () => {
    const value = ($("#ai-url-input")?.value || "").trim();
    if (!value) { toast("请输入接口地址"); return; }
    aiApply(rootExec(moduleCommand(`ai-url ${shellQuote(value)}`), 8000, false), "接口地址已保存（localhost/私有地址会被拒绝）");
  });
  $("#ai-restore")?.addEventListener("click", () => {
    NovaHaptics.select();
    aiApply(rootExec(moduleCommand("ai-restore"), 8000, false), "已恢复本地调度（AI 临时策略已清除）");
  });
  /* 立即分析：手动诊断（只记录建议，不写入临时策略；未设置 Key 时给出明确原因）。 */
  $("#ai-analyze")?.addEventListener("click", async event => {
    const button = event.currentTarget;
    NovaHaptics.select();
    button.disabled = true;
    const original = button.textContent;
    button.textContent = "分析中…";
    try {
      const outcome = await rootExec(moduleCommand("ai-decide ui_manual"), 45000, false);
      const text = `${outcome.stdout || ""}\n${outcome.stderr || ""}`.trim();
      if (outcome.errno !== 0) throw new Error(text.slice(0, 160) || "分析命令失败");
      toast(text.slice(0, 180) || "已提交分析");
      NovaHaptics.success();
      probeAi(true);
      probeAiLog();
    } catch (error) {
      toast(`分析未完成：${String(error?.message || error).slice(0, 150)}`);
      NovaHaptics.failure();
    } finally {
      button.disabled = false;
      button.textContent = original;
    }
  });
  $("#scene-release")?.addEventListener("click", () => {
    if (s.pending) return;
    NovaHaptics.select();
    // ZenJooo · 取回控制权：服务端清除 novasched.scene.mode 存档属性；
    // 守护下一拍回读后 sceneDriving 翻转，横幅消失、四档恢复可点。
    if (!send("modes", "scene\trelease")) return;
    s.sceneDriving = false;
    render();
    toast("已取回档位控制权");
  });
  $("#scene-diagnose-settings").onclick = () => showDiagnostics(true);
  $("#about-diagnose").onclick = event => { event.preventDefault(); showDiagnostics(); };
  $("#about-update").onclick = () => showUpdate();
  $("#about-donate").onclick = () => showDonate();
  $("#about-license").onclick = event => { event.preventDefault(); openDialog(`<h3 id="dialog-title">来源与许可</h3><p class="dialog-sub">模块作者：ZenJooo。调度核心沿用 GPL-3.0-only。</p><p class="dialog-sub">NovaFAS 为本模块原创的帧感知调度引擎，读取 SurfaceFlinger 帧统计并在档位包络内动态调频。</p><p class="dialog-sub">界面布局沿用其早期版本的玻璃设计语言；玻璃透镜与色散公式来自 Kyant Backdrop，Apache-2.0。</p><p class="dialog-sub"><a href="https://github.com/Kyant0/AndroidLiquidGlass" target="_blank" rel="noopener noreferrer">Kyant0/AndroidLiquidGlass</a></p><div class="dialog-actions"><button class="btn-capsule ghost" id="license-close" type="button">关闭</button></div>`);$("#license-close").onclick=()=>closeDialog(); };
  $("#nav-cta").onclick = () => { if (s.online) showDiagnostics(); else if (!s.connecting) { clearTimeout(s.retry); discover(); toast("正在连接模块服务"); } else toast("正在连接，请稍候"); };
  $(".hero-card").onclick = () => showDiagnostics();
  $(".hero-card").addEventListener("keydown", event => {
    if (event.key === "Enter" || event.key === " ") { event.preventDefault(); showDiagnostics(); }
  });
  $$(".log-tools [data-level]").forEach(button => button.onclick = () => {
    s.level = button.dataset.level;
    $$(".log-tools [data-level]").forEach(node => node.classList.toggle("active", node === button));
    renderLogs();
  });
  $("#log-follow").onclick = event => {
    const on = event.currentTarget.getAttribute("aria-checked") !== "true";
    event.currentTarget.setAttribute("aria-checked", String(on));
    s.follow = on;
    if (on) { logList.scrollTop = logList.scrollHeight; }
  };
  $("#log-copy").onclick = () => copyText(visibleLogs().map(l => l.raw).join("\n"), "筛选后的日志已复制");
  logList.addEventListener("click", event => {
    const row = event.target.closest("article[data-raw]");
    if (!row) return;
    const line = parseLog(row.dataset.raw);
    openDialog(`
      <p class="eyebrow">日志详情</p>
      <h3 id="dialog-title">${esc(line.time || "本地记录")} · ${LV_TAG[line.level]}</h3>
      <pre class="report">${esc(line.raw)}</pre>
      <div class="dialog-actions">
        <button class="btn-capsule accent" type="button" id="dialog-cancel">关闭</button>
      </div>
    `);
    $("#dialog-cancel").onclick = () => closeDialog();
  });

  function trapFocus(event,root){
    if(event.key!=="Tab")return;
    const fields=$$("button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled),a[href],[tabindex]",root).filter(node=>node.getClientRects().length&&!node.hidden&&node.getAttribute("tabindex")!=="-1"&&!node.closest("[inert]"));
    const first=fields[0],last=fields.at(-1);if(!first)return;
    if(!fields.includes(document.activeElement)){event.preventDefault();first.focus();}else if(event.shiftKey&&document.activeElement===first){event.preventDefault();last.focus();}else if(!event.shiftKey&&document.activeElement===last){event.preventDefault();first.focus();}
  }
  $("#foreground-time").onclick=()=>{if(!editable()||!s.pkg){toast("连接后可以为前台应用设置规则");return;}showRuleEditor(basePackage(s.pkg));};
  $("#foreground-card").onclick=event=>{if (!event.target.closest("button")) $("#foreground-time").click();};
  $("#foreground-card").addEventListener("keydown",event=>{if(event.target.closest("button"))return;if(event.key==="Enter"||event.key===" "){event.preventDefault();$("#foreground-time").click();}});
  $("#rule-search").oninput=event=>{ruleQuery=event.target.value.trim().toLowerCase();renderRules();};
  let ruleDrag=null,suppressRuleClick=0;
  $("#rule-list").addEventListener("pointerdown",event=>{
    const row=event.target.closest(".rule-row");if(!row||event.button>0||event.isPrimary===false||ruleDrag)return;ruleDrag={row,id:event.pointerId,x:event.clientX,y:event.clientY,delta:0,active:false};
  },{passive:true});
  $("#rule-list").addEventListener("pointermove",event=>{
    if(!ruleDrag||event.pointerId!==ruleDrag.id)return;
    const dx=event.clientX-ruleDrag.x,dy=event.clientY-ruleDrag.y;
    if(!ruleDrag.active&&Math.abs(dy)>10&&Math.abs(dy)>Math.abs(dx)){ruleDrag=null;return;}
    if(!ruleDrag.active&&Math.abs(dx)>12&&Math.abs(dx)>Math.abs(dy)*1.5){ruleDrag.active=true;ruleDrag.row.setPointerCapture?.(event.pointerId);}
    if(!ruleDrag.active)return;event.preventDefault();ruleDrag.delta=dx;ruleDrag.row.style.transform=`translateX(${Math.max(-72,Math.min(0,dx))}px)`;window.NovaGlass?.invalidate();
  });
  function stopRuleSwipe(event,cancelled=false){
    if(!ruleDrag||event.pointerId!==ruleDrag.id)return;const {row,delta,active}=ruleDrag;
    ruleDrag=null;if(row.hasPointerCapture?.(event.pointerId))row.releasePointerCapture?.(event.pointerId);
    if(active){const open=!cancelled&&delta<-36;row.style.transform=open?"translateX(-72px)":"";row.parentElement.classList.toggle("swiped",open);const action=$(".swipe-delete",row.parentElement);action.tabIndex=open?0:-1;action.setAttribute("aria-hidden",String(!open));suppressRuleClick=performance.now()+180;window.NovaGlass?.invalidate(300);}
  }
  $("#rule-list").addEventListener("lostpointercapture",event=>stopRuleSwipe(event,true));
  $("#rule-list").addEventListener("pointerup",event=>stopRuleSwipe(event));$("#rule-list").addEventListener("pointercancel",event=>stopRuleSwipe(event,true));
  $("#rule-list").addEventListener("click",event=>{if(performance.now()<suppressRuleClick){event.preventDefault();event.stopImmediatePropagation();}},true);
  $("#rule-list").addEventListener("click",event=>{const button=event.target.closest("[data-rule-delete]");if(button)showDeleteConfirm(button.dataset.ruleDelete);});

  /* ---------------- 页面可见性 ---------------- */
  function suspend() {
    finishMotion();stopSliderPreview();if(dockDrag)endDock({pointerId:dockDrag.pointer},true);if(panelDrag)endPanelDrag({pointerId:panelDrag.id},true);if(ruleDrag)stopRuleSwipe({pointerId:ruleDrag.id},true);
    cancelRootBridge();
    if (s.suspended) return;
    s.suspended = true; s.epoch++; clearTimeout(s.retry); cancelProbes(); cancelAuthentication();
    ["modes","apps","logs"].forEach(stopSocket);
    s.online = false; s.connecting = false; s.discovered = false;
    if (s.pending) finishPending(false, "页面进入后台，返回后请确认操作结果");
  }
  function resume() { if (s.suspended) { s.suspended = false; discover(); } }
  document.addEventListener("visibilitychange", () => document.hidden ? suspend() : resume());
  window.addEventListener("pagehide", suspend);
  window.addEventListener("pageshow", resume);

  /* ---------------- 目标 FPS（调度目标，独立存储 fps_rules.json） ----------------
     Android 无公开的按应用 FPS 强制入口：这里的目标传给 NovaFAS 作为帧
     基线（达标不加压、低于目标才加压），不改变游戏渲染上限，不称"已限帧"。 */
  function probeFps(force = false) {
    if (!force && s.fpsRules) return Promise.resolve(s.fpsRules);
    if (typeof bridge()?.exec !== "function") return Promise.resolve({});
    return rootExec(moduleCommand("fps-status"), 8000, false).then(result => {
      const rules = {};
      if (result.errno === 0) {
        for (const match of (result.stdout || "").matchAll(/^rule_(\S+)=(\d+)$/gm)) {
          if (Number(match[2]) > 0) rules[match[1]] = Number(match[2]);
        }
        s.fpsRules = rules;
        renderRules();
      }
      return s.fpsRules || {};
    }).catch(() => ({}));
  }
  function applyFps(pkg, fps) {
    const command = fps === 0
      ? moduleCommand(`fps-remove ${shellQuote(pkg)}`)
      : moduleCommand(`fps-set ${shellQuote(pkg)} ${shellQuote(String(fps))}`);
    return rootExec(command, 15000, false).then(result => {
      const text = `${result.stdout || ""}\n${result.stderr || ""}`.trim();
      // fps-set/fps-remove 成功时回显 status()（必含 rules_count=）；失败
      // 走 stderr 且退出码非 0。旧判定依赖中文文案易误判（审计 F8）。
      if (result.errno !== 0 || !/rules_count=/.test(result.stdout || "")) {
        return { ok: false, message: text.slice(0, 160) || "目标 FPS 命令失败" };
      }
      s.fpsRules = { ...s.fpsRules };
      if (fps === 0) delete s.fpsRules[pkg];
      else s.fpsRules[pkg] = fps;
      renderRules();
      return { ok: true, message: `调度目标 ${fps} FPS 已保存：NovaFAS 以此判断是否加压；不改变游戏渲染上限` };
    }).catch(error => ({ ok: false, message: `目标 FPS 保存失败：${String(error?.message || error).slice(0, 120)}` }));
  }

  /* ---------------- 面板刷新率（Scene 同款，refresh_rules.json） ----------------
     机制与 Scene N1 相同：dumpsys display 解析设备真实模式列表，前台切换时
     `service call SurfaceFlinger 1035` 设置激活显示模式。选项只来自设备真实
     模式，解析不到就禁用并说明原因，不虚构 Hz。 */
  function probeRefresh(force = false) {
    if (!force && s.refreshRules) return Promise.resolve(s.refreshRules);
    if (typeof bridge()?.exec !== "function") return Promise.resolve({});
    return rootExec(moduleCommand("refresh-status"), 8000, false).then(result => {
      const rules = {};
      const hzList = new Set();
      let modesError = "";
      if (result.errno === 0) {
        for (const match of (result.stdout || "").matchAll(/^rule_(\S+)=(\d+)$/gm)) {
          if (Number(match[2]) > 0) rules[match[1]] = Number(match[2]);
        }
        for (const match of (result.stdout || "").matchAll(/^mode_\d+=(\d+)Hz/gm)) {
          const hz = Number(match[1]);
          if (hz >= 30 && hz <= 240) hzList.add(hz);
        }
        const unavailable = (result.stdout || "").match(/^modes_unavailable=(.+)$/m);
        if (unavailable) modesError = unavailable[1].trim().slice(0, 120);
      } else {
        modesError = "refresh-status 查询失败";
      }
      s.refreshRules = rules;
      s.refreshHzList = [...hzList].sort((a, b) => b - a);
      s.refreshModesError = modesError;
      return rules;
    }).catch(() => ({}));
  }
  function applyRefresh(pkg, hz) {
    const command = hz === 0
      ? moduleCommand(`refresh-remove ${shellQuote(pkg)}`)
      : moduleCommand(`refresh-set ${shellQuote(pkg)} ${shellQuote(String(hz))}`);
    return rootExec(command, 15000, false).then(result => {
      const text = `${result.stdout || ""}\n${result.stderr || ""}`.trim();
      // 与 applyFps 同一判定：成功必回显 rules_count=（审计 F8 同类修正）。
      if (result.errno !== 0 || !/rules_count=/.test(result.stdout || "")) {
        return { ok: false, message: text.slice(0, 160) || "刷新率规则保存失败" };
      }
      s.refreshRules = { ...s.refreshRules };
      if (hz === 0) delete s.refreshRules[pkg];
      else s.refreshRules[pkg] = hz;
      renderRules();
      return { ok: true, message: `刷新率 ${hz}Hz 已保存：前台进入该应用时切换面板模式（Scene 同款）；不是强制帧率上限` };
    }).catch(error => ({ ok: false, message: `刷新率保存失败：${String(error?.message || error).slice(0, 120)}` }));
  }

  /* ---------------- 触感开关（外观组） ----------------
     通道链路：NovaHostHaptics（若宿主注入）→ `haptic` 系统命令
     （cmd vibrator_manager，能力探测含 list 振动器检查并缓存）→
     Web Vibration。各层只声明“请求已提交”，不声称用户感受到了振动。 */
  const hapticsSwitch = $("#haptics-switch");
  const hapticsNote = $("#haptics-note");
  const hapticsTest = $("#haptics-test");
  function renderHapticsSetting() {
    const on = NovaHaptics.isEnabled();
    hapticsSwitch.setAttribute("aria-checked", String(on));
    const channel = NovaHaptics.channel();
    const system = NovaHaptics.systemStatus();
    hapticsNote.textContent = !on ? "已关闭"
      : channel === "native" ? "宿主原生触感"
      : channel === "system"
        ? system.hapticEnabled === false
          ? "系统触感命令已接入；系统设置中的触感反馈当前为关闭"
          : "Android 系统触感（遵循系统开关）"
      : channel === "vibrate" ? "设备振动回退（系统触感未接入或不可用）"
      : system.wired && system.detail ? `系统触感不可用：${system.detail}`
      : "当前宿主未提供触感能力";
  }
  hapticsSwitch.onclick = () => {
    const next = !NovaHaptics.isEnabled();
    NovaHaptics.setEnabled(next);
    renderHapticsSetting();
    if (next) NovaHaptics.toggle();
  };
  if (hapticsTest) hapticsTest.onclick = async () => {
    probeHaptics(true);
    hapticsTest.disabled = true;
    try {
      const outcome = await NovaHaptics.test();
      renderHapticsSetting();
      toast(outcome.message, outcome.ok ? 2600 : 4200);
    } finally {
      hapticsTest.disabled = false;
    }
  };
  renderHapticsSetting();window.addEventListener("nova:haptics",renderHapticsSetting);
  /* 系统触感探测：桥接就绪后自动执行一次；瞬时失败有界重试（3 次，
     退避 1.5s），确定性失败保留原因并允许“测试触感”强制重探。
     探测结果解析 stdout：channel/haptic_enabled 分别决定通道与静音提示。 */
  let hapticsProbed = false, hapticsProbeAttempts = 0;
  function wireHapticsProbe(stdout, stderr, errno) {
    if (errno === 0) {
      hapticsProbed = true;
      hapticsProbeAttempts = 0;
      const channel = /channel=feedback/.test(stdout) ? "feedback" : /channel=synced/.test(stdout) ? "synced" : "";
      const hapticEnabled = /haptic_enabled=1/.test(stdout) ? true : /haptic_enabled=0/.test(stdout) ? false : null;
      if (channel) {
        NovaHaptics.setSystemBridge(async semantic => {
          const r = await rootExec(moduleCommand(`haptic ${shellQuote(semantic)}`), 5000, false);
          if (r.errno !== 0) throw new Error((r.stderr || "系统触感命令失败").trim().slice(0, 120));
          if (/haptic=submitted/.test(r.stdout || "")) return "submitted";
          if (/haptic=skipped/.test(r.stdout || "")) return "skipped";
          throw new Error("系统触感命令未确认提交");
        }, { hapticEnabled });
        return;
      }
      NovaHaptics.setSystemBridge(null, { detail: (stderr || stdout || "系统没有兼容的触感命令").trim().slice(0, 120) });
      return;
    }
    // 探测本身失败（无兼容命令 / 无振动器）：确定性失败保留原因。
    hapticsProbed = true;
    NovaHaptics.setSystemBridge(null, { detail: (stderr || "系统触感探测失败").trim().slice(0, 120) });
  }
  function probeHaptics(force = false) {
    if (!force && (hapticsProbed || typeof bridge()?.exec !== "function")) return;
    if (hapticsProbeAttempts >= 3) return; // 有界：瞬时故障重试 3 次后停止
    hapticsProbeAttempts += 1;
    rootExec(moduleCommand("haptic probe"), 5000)
      .then(result => wireHapticsProbe(result.stdout || "", result.stderr || "", result.errno))
      .catch(() => {
        // 瞬时失败（桥未就绪/root 瞬断）：不清空已接入通道，退避后重试。
        if (hapticsProbeAttempts < 3) setTimeout(() => { if (!hapticsProbed) probeHaptics(true); }, 1500 * hapticsProbeAttempts);
        else hapticsProbeAttempts = 0; // 下一轮打开设置时重新允许探测
      });
  }
  for(const id of ["personalize-open","appearance-settings","glass-settings"])$("#"+id).addEventListener("click",() => probeHaptics());

  /* ---------------- 图形后端规则（ANGLE / 跟随系统） ----------------
     规则存储在 graphics_rules.json，与档位规则（app_modes.txt）相互独立；
     经 root 备用通道调用 novasched graphics-status / graphics-set /
     graphics-remove，后端写系统 angle 列表并读回确认。
     如实声明：ANGLE 是 OpenGL ES 经 ANGLE 转译，不是“原生 Vulkan”；
     Android 没有公开的按应用强制 Vulkan 入口，本模块不提供虚假选项。
     生效条件是目标应用完全退出重开；保存成功≠当前进程已切换。 */
  function probeGraphics(force = false) {
    if (!force && s.graphics.probed) return Promise.resolve(s.graphics);
    if (typeof bridge()?.exec !== "function") {
      s.graphics = { ...s.graphics, available: null, reason: "需要 root 备用通道才能读取图形后端能力" };
      return Promise.resolve(s.graphics);
    }
    return rootExec(moduleCommand("graphics-status"), 8000, false).then(result => {
      s.graphics = { ...s.graphics, probed: true };
      if (result.errno !== 0) {
        s.graphics.available = null;
        s.graphics.reason = (result.stderr || "graphics-status 查询失败").trim().slice(0, 120);
        renderGraphicsViews();
        return s.graphics;
      }
      const text = result.stdout || "";
      const available = /angle_driver_available=yes/.test(text) ? true
        : /angle_driver_available=no/.test(text) ? false : null;
      const rules = {};
      // schema v2：rule_<pkg>=<mode>;enabled=…;status=…;detail=…
      for (const match of text.matchAll(/^rule_(\S+)=([a-z]+);enabled=(true|false);status=([^;]*);detail=(.*)$/gm)) {
        rules[match[1]] = match[2];
      }
      const status = {};
      for (const match of text.matchAll(/^rule_(\S+)=[a-z]+;enabled=(?:true|false);status=([^;]*);detail=(.*)$/gm)) {
        status[match[1]] = { status: match[2], detail: match[3] };
      }
      const vulkanDevices = [...text.matchAll(/^vulkan_device=(.+)\|API ([\d.]+)$/gm)]
        .map(match => ({ name: match[1], api: match[2] }));
      const sdkMatch = text.match(/^android_sdk=(\d+)$/m);
      s.graphics.available = available;
      s.graphics.reason = available === false ? "本机未安装 ANGLE 驱动包（com.google.android.angle）" : "";
      s.graphics.rules = rules;
      s.graphics.status = status;
      s.graphics.capability = {
        sdk: sdkMatch ? Number(sdkMatch[1]) : null,
        vulkan: vulkanDevices,
        angle: available,
      };
      renderGraphicsViews();
      renderRules();
      return s.graphics;
    }).catch(error => {
      s.graphics = { ...s.graphics, available: null, reason: String(error?.message || error).slice(0, 120) };
      renderGraphicsViews();
      return s.graphics;
    });
  }
  /* 提交单条图形后端规则并读回；resolve 简短结果文案。跟随系统=移除；
     vulkan 档返回真实的 unsupported 原因链（写盘成功但状态=unsupported）。 */
  function applyGraphics(pkg, backend) {
    const command = backend === "follow"
      ? moduleCommand(`graphics-remove ${shellQuote(pkg)}`)
      : moduleCommand(`graphics-set ${shellQuote(pkg)} ${shellQuote(backend)}`);
    return rootExec(command, 20000, false).then(result => {
      const text = `${result.stdout || ""}\n${result.stderr || ""}`.trim();
      if (result.errno !== 0) return { ok: false, message: text.slice(0, 160) || "图形后端命令失败" };
      s.graphics.rules = { ...s.graphics.rules };
      if (backend === "follow") delete s.graphics.rules[pkg];
      else s.graphics.rules[pkg] = backend;
      const unsupported = text.match(/(com\.\S+: 不支持 —— .*)/);
      renderGraphicsViews();
      renderRules();
      if (unsupported) return { ok: true, message: unsupported[1].slice(0, 200) };
      return { ok: true, message: "图形后端已保存并写回系统设置；目标应用需完全退出重开才能生效" };
    }).catch(error => ({ ok: false, message: `图形后端保存失败：${String(error?.message || error).slice(0, 120)}` }));
  }


  applyTheme();
  loadWallpaperFromStorage();
  setPage("overview");
  render();
  renderLogs();
  discover();

  /* 滚动期临时关闭 CSS 模糊：长规则列表滚动时避免大面积 backdrop-filter
     每帧重算（低端机掉帧的主因之一）。结束 160ms 后恢复，外观不变。 */
  let scrollTimer = 0;
  const onScroll = () => {
    if (!document.body.classList.contains("is-scrolling")) document.body.classList.add("is-scrolling");
    clearTimeout(scrollTimer);
    scrollTimer = setTimeout(() => document.body.classList.remove("is-scrolling"), 160);
  };
  mainScroll.addEventListener("scroll", onScroll, { passive: true });
  document.addEventListener("scroll", event => {
    if (event.target && event.target !== document && event.target.scrollHeight > event.target.clientHeight) onScroll();
  }, { passive: true, capture: true });
})();
