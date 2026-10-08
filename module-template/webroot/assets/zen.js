/* NovaSched Zen UI v1.5.1.
 * Original Web implementation; the Rust command protocol is unchanged.
 * Rendering never equates sending a command with successful application.
 */
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
  const STORAGE = "novasched.zen.ui.v1";
  const MODES = {
    powersave: { name: "省电", icon: "eco", copy: "降低频率上限，适合轻负载" },
    balance: { name: "均衡", icon: "balance", copy: "适合日常使用，保留完整频率范围" },
    performance: { name: "性能", icon: "speed", copy: "适度提高频率下限" },
    fast: { name: "极速", icon: "flash_on", copy: "提高频率下限，适合持续高负载" },
  };
  const THEME_NAMES = { system: "跟随系统", light: "浅色", dark: "深色" };
  // These are honest fallback aliases only. Other labels come from Android's PM.
  const SYSTEM_NAMES = {
    "com.android.systemui": "系统界面", "com.android.settings": "系统设置",
    "com.google.android.webview": "Android 系统 WebView", "com.miui.home": "系统桌面",
  };
  const s = {
    page: "home", online: false, connecting: true, phase: "", error: "", connectionError: "", version: "",
    mode: "", effective: "", pkg: "", controller: "", external: false, linked: false,
    heartbeat: null, lastFrame: 0, host: "127.0.0.1", port: 31415,
    soc: "", socId: "", configProfile: "", smoothSupported: null, extremeSupported: null,
    smooth: false, extreme: false, powerProfile: "", rules: [], rulesLoaded: false,
    pending: null, sockets: {}, suspended: false, retry: 0, epoch: 0, probes: new Set(),
    transport: "", rootTimer: 0, rootJob: null, rootOrigin: "", rootGeneration: 0, logCursor: "",
    discovered: false, logConnected: false, logConnecting: false, logs: [], logTail: "", level: "all", follow: true,
    logReceived: false, sheet: "", sheetEpoch: 0, edit: null, installed: null, loadingApps: false,
    labels: new Map(), queried: new Set(), iconFailures: new Set(), rulesRenderKey: "", logsRenderKey: "",
  };
  let ui = { theme: "system", glass: true, motion: false, port: 31415, glassLevel: 45 };
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE) || "{}");
    if (own(THEME_NAMES, saved.theme)) ui.theme = saved.theme;
    if (typeof saved.glass === "boolean") ui.glass = saved.glass;
    if (typeof saved.motion === "boolean") ui.motion = saved.motion;
    if (Number.isInteger(saved.glassLevel) && saved.glassLevel >= 0 && saved.glassLevel <= 100) ui.glassLevel = saved.glassLevel;
    if (Number.isInteger(saved.port) && saved.port >= 1024 && saved.port <= 65535) ui.port = saved.port;
  } catch (_) { /* Storage may be disabled by the host WebView. */ }
  s.port = ui.port;
  const darkQuery = window.matchMedia?.("(prefers-color-scheme: dark)");
  const motionQuery = window.matchMedia?.("(prefers-reduced-motion: reduce)");
  let snackbarTimer = 0;
  let pendingTimer = 0;
  let bridgeCounter = 0;
  let sessionToken = "";
  let authRequest = null;
  let bridgeSignature = 0;
  let previousFocus = null;
  const scrollPositions = {};
  const bridges = () => [window.ksu, window.magisk, window.apatch, window.KSU, window.mmrl].filter(Boolean);
  const bridge = () => bridges().find(api => typeof api.exec === "function") || bridges()[0];
  const metadataBridge = () => bridges().find(api => typeof api.getPackagesInfo === "function") || bridge();
  const esc = (value) => String(value ?? "").replace(/[&<>"']/g, c => ({ "&":"&amp;", "<":"&lt;", ">":"&gt;", '"':"&quot;", "'":"&#39;" }[c]));
  const icon = (name) => `<svg class="icon" aria-hidden="true"><use href="#i-${esc(name)}"/></svg>`;
  const basePackage = (raw) => String(raw || "").split(":", 1)[0];
  const validPackage = (raw) => typeof raw === "string" && raw.length <= 255 && /^[A-Za-z][A-Za-z0-9_]*(?:\.[A-Za-z][A-Za-z0-9_]*)+(?::[A-Za-z0-9_.]+)?$/.test(raw);
  const editable = () => s.online && !s.external;
  const present = (id, text) => { const node = $(id); if (node && node.textContent !== String(text)) node.textContent = String(text); };
  function saveUI() { try { localStorage.setItem(STORAGE, JSON.stringify(ui)); } catch (_) {} }

  function glassNote(level) {
    if (level >= 80) return "极通透 · 最纯粹的液态质感";
    if (level >= 55) return "通透 · 明显的折射光感";
    if (level >= 30) return "适中 · 平衡通透与可读性";
    return "磨砂 · 雾面玻璃，最易阅读";
  }
  function applyAppearance() {
    const dark = ui.theme === "dark" || (ui.theme === "system" && !!darkQuery?.matches);
    document.documentElement.dataset.theme = dark ? "dark" : "light";
    document.documentElement.dataset.effects = ui.glass ? "glass" : "simple";
    // One CSS variable drives every liquid glass surface (blur, tint, specular).
    document.documentElement.style.setProperty("--glass-level", String(ui.glassLevel));
    document.documentElement.dataset.motion = ui.motion || motionQuery?.matches ? "reduced" : "full";
    $("meta[name=theme-color]").content = dark ? "#111c19" : "#f2f6f3";
    present("#theme-label", THEME_NAMES[ui.theme]);
    $("#glass-effects").checked = ui.glass;
    $("#reduce-motion").checked = ui.motion;
    const slider = $("#glass-intensity");
    if (slider) {
      slider.disabled = !ui.glass;
      if (slider.value !== String(ui.glassLevel)) slider.value = String(ui.glassLevel);
      slider.style.setProperty("--fill", ui.glassLevel + "%");
      present("#glass-value", ui.glassLevel + "%");
      present("#glass-note", glassNote(ui.glassLevel));
    }
    present("#motion-note", motionQuery?.matches ? "系统已启用减少动态效果" : "也会尊重系统的减少动态效果设置");
  }
  function toast(message, timeout = 3500) {
    present("#snackbar-message", message);
    $("#snackbar").classList.add("show");
    clearTimeout(snackbarTimer);
    snackbarTimer = setTimeout(() => $("#snackbar").classList.remove("show"), timeout);
  }
  function feedback(message, error = false) {
    const node = $("#mode-feedback");
    node.hidden = !message;
    node.textContent = message;
    node.dataset.error = String(error);
  }
  async function copyText(text, message = "已复制") {
    if (!text) return toast("没有可复制的内容");
    try {
      if (navigator.clipboard?.writeText) { await navigator.clipboard.writeText(text); return toast(message); }
    } catch (_) { /* Some KSU WebViews reject the Clipboard permission. */ }
    let field;
    try {
      field = document.createElement("textarea"); field.value = text;
      Object.assign(field.style, { position:"fixed", opacity:"0", pointerEvents:"none" });
      (s.sheet ? $("#sheet-body") : document.body).append(field); field.select();
      toast(document.execCommand("copy") ? message : "当前 WebView 未允许复制，请长按文本选择");
    } catch (_) { toast("当前 WebView 未允许复制，请长按文本选择"); }
    finally { field?.remove(); }
  }

  function appInfo(raw) {
    const pkg = basePackage(raw);
    const info = s.labels.get(pkg);
    const name = info?.appLabel || SYSTEM_NAMES[pkg] || "未知应用";
    return { package: String(raw || ""), base: pkg, name, known: !!info?.appLabel || !!SYSTEM_NAMES[pkg],
      initial: name === "未知应用" ? "?" : Array.from(name)[0], version: info?.versionName || "",
      system: info?.isSystem === true, uid: info?.uid, process: String(raw || "").includes(":") };
  }
  function avatar(raw, extra = "") {
    const a = appInfo(raw);
    const available = a.base && typeof metadataBridge()?.listPackages === "function" && !s.iconFailures.has(a.base);
    return `<span class="app-avatar ${esc(extra)}"><span>${esc(a.initial)}</span>${available ? `<img alt="" loading="lazy" decoding="async" data-app-icon="${esc(a.base)}" src="ksu://icon/${encodeURIComponent(a.base)}">` : ""}</span>`;
  }
  function setAvatar(node, raw) {
    const a = appInfo(raw);
    const key = a.base + "/" + a.name + "/" + s.iconFailures.has(a.base);
    if (node.dataset.key === key) return;
    node.dataset.key = key;
    const temporary = document.createElement("div"); temporary.innerHTML = avatar(raw);
    node.replaceChildren(...temporary.firstElementChild.childNodes);
  }
  function loadLabels(packages) {
    const api = metadataBridge();
    if (typeof api?.getPackagesInfo !== "function") return;
    const needed = [...new Set(packages.map(basePackage))].filter(pkg => validPackage(pkg) && !s.queried.has(pkg));
    if (!needed.length) return;
    for (let index = 0; index < needed.length; index += 48) {
      const batch = needed.slice(index, index + 48);
      try {
        const result = JSON.parse(api.getPackagesInfo(JSON.stringify(batch)));
        if (!Array.isArray(result)) continue;
        for (const info of result) {
          if (batch.includes(info.packageName) && typeof info.appLabel === "string" && info.appLabel.trim()) {
            s.labels.set(info.packageName, { ...info, appLabel: info.appLabel.trim().slice(0, 300) });
            s.queried.add(info.packageName);
          }
        }
        // Missing entries get a session-level negative cache to avoid heartbeats
        // repeatedly crossing the Android bridge for isolated/sandbox processes.
        batch.forEach(pkg => s.queried.add(pkg));
      } catch (_) { batch.forEach(pkg => s.queried.add(pkg)); }
    }
  }
  document.addEventListener("error", event => {
    const img = event.target;
    if (img instanceof HTMLImageElement && img.dataset.appIcon) {
      s.iconFailures.add(img.dataset.appIcon); img.parentElement.classList.add("icon-failed");
    }
  }, true);

  function stateLabel() {
    return !s.online ? s.connecting ? "连接中" : "未连接" : s.phase === "degraded" ? "需检查" : s.external ? "外部接管" : "调度正常";
  }
  function sourceLabel() {
    if (!s.online) return "等待本地服务";
    if (s.external) return "Scene 外部调度器";
    if (s.linked) return "Scene 联动";
    return "WebUI 接管";
  }
  const connectionAddress = () => s.discovered ? (s.transport === "root" ? "宿主 root 通道" : `${s.host}:${s.port}`) : "从管理器读取本机服务";
  function render() {
    const connection = !s.online ? s.connecting ? "connecting" : "offline" : s.phase === "degraded" ? "degraded" : "online";
    $("#connection").dataset.state = connection;
    present("#connection-label", s.online ? s.phase === "degraded" ? "需检查" : "已连接" : s.connecting ? "连接中" : "未连接");
    $("#hero").dataset.state = connection;
    $("#hero").dataset.mode = s.online && !s.external ? s.effective : "";
    present("#phase-name", stateLabel());
    present("#active-mode", !s.online ? "—" : s.external ? "外部控制" : MODES[s.effective]?.name || "待确认");
    present("#mode-description", !s.online ? "暂未读到守护进程的有效心跳" : s.external ? "Scene 决定档位，本地策略已暂停" : MODES[s.effective]?.copy || "等待实际策略反馈");
    $("#hero-icon").setAttribute("href", "#i-" + (MODES[s.effective]?.icon || "balance"));
    present("#runtime-title", !s.online ? s.connecting ? "正在寻找本地服务" : "暂未连接到守护进程" : s.phase === "degraded" ? "部分下发失败，请查看诊断" : s.external ? "外部控制期间，本地保持只读" : "心跳有效，调度核心正在运行");
    present("#heartbeat", s.online && Number.isFinite(s.heartbeat) ? `心跳 ${Math.round(s.heartbeat / 100) / 10}s` : "—");
    const powerNames = { smooth:"流畅省电", extreme:"极限节能", standard:"标准省电" };
    const actualPower = s.online && !s.external && s.phase === "ready" && s.effective === "powersave" && powerNames[s.powerProfile];
    $("#power-profile").hidden = !actualPower;
    present("#power-profile", actualPower || "");
    const active = s.online && !!s.pkg;
    const a = appInfo(active ? s.pkg : "");
    present("#foreground-name", active ? a.name : "等待识别");
    present("#foreground-caption", active ? a.process ? "应用子进程 · 点按查看详情" : a.known ? "正在使用 · 点按查看详情" : "名称暂不可用 · 点按查看标识" : "连接后自动更新");
    setAvatar($("#foreground-icon"), active ? s.pkg : "");
    setAvatar($("#current-icon"), active ? s.pkg : "");
    $("#foreground-card").disabled = !active;
    present("#current-name", active ? a.name : "等待识别");
    $("#current-rule").disabled = !editable() || !active || !!s.pending;
    const currentRule = s.rules.find(rule => rule.package === a.base);
    present("#current-rule-note", currentRule ? `${MODES[currentRule.mode]?.name || "未知"}档 · 点按编辑` : "使用单独的运行档位");
    present("#profile-hint", !s.online ? "连接后可切换" : s.external ? "Scene 外部调度器接管 · 请在 Scene 切档" : s.pending?.kind === "mode" ? "等待实际反馈" : s.linked ? "与 Scene 双向同步" : "高亮为实际生效档位");
    $$(".profile").forEach(button => {
      const actual = s.online && !s.external && button.dataset.mode === s.effective;
      const pending = s.pending?.kind === "mode" && s.pending.value === button.dataset.mode;
      button.setAttribute("aria-pressed", String(actual));
      button.setAttribute("aria-busy", String(pending));
      button.classList.toggle("is-pending", pending);
      button.disabled = !editable() || !!s.pending;
      $(".profile-caption", button).textContent = pending ? "提交中…" : actual ? s.phase === "ready" ? "当前生效" : "最后成功档位" : ({ powersave:"续航优先", balance:"日常默认", performance:"响应优先", fast:"游戏与重负载" })[button.dataset.mode];
    });
    present("#source-title", sourceLabel());
    present("#source-note", !s.online ? "连接后显示当前控制来源" : s.external ? "切换档位和修改规则暂不可用" : s.linked ? "档位与应用规则与 Scene 双向同步，两侧修改即时生效" : "本地默认档位与应用规则共同生效");
    present("#rules-context", !s.online ? "连接后可以编辑；已经读取的规则仍可查看。" : s.external ? "外部 Scene 调度器接管中，规则当前只读。" : s.linked ? "应用规则与 Scene 双向同步；在本页或 Scene 修改都会同步到另一侧。" : "应用进入前台时，使用对应档位；其余沿用默认档位。");
    $("#add-rule").disabled = !editable() || !!s.pending;
    for (const name of ["smooth", "extreme"]) {
      const field = $("#" + name + "-save");
      field.checked = s.pending?.kind === name ? s.pending.value : s[name];
      field.disabled = !editable(name) || !!s.pending || s[name + "Supported"] === false;
      field.setAttribute("aria-busy", String(s.pending?.kind === name));
    }
    let smoothNote = s.online && s.smoothSupported === false ? "当前处理器配置未提供此选项" : !s.online ? "连接后查看已保存的选择" : s.pending?.kind === "smooth" ? "提交中，等待服务确认…" : !s.smooth ? "未开启 · 默认关闭" : s.external ? "已保存 · 外部接管期间暂停" : s.phase === "degraded" ? "下发异常 · 请查看诊断" : s.effective !== "powersave" ? "已保存 · 切至省电档时生效" : s.powerProfile === "smooth" ? "已生效 · 流畅省电" : "已保存 · 等待参数下发";
    present("#smooth-note", smoothNote);
    present("#extreme-note", s.online && s.extremeSupported === false ? "当前处理器配置未提供此选项" : !s.online ? "连接后查看已保存的选择" : s.pending?.kind === "extreme" ? "提交中，等待服务确认…" : s.smooth ? "保留你的选择 · 当前流畅省电优先" : !s.extreme ? "未开启 · 使用标准省电" : actualPower === "极限节能" ? "已生效 · 极限节能" : "已开启 · 仅在省电档生效");
    present("#server-address", connectionAddress());
    present("#processor-tag", s.online && s.socId ? s.socId : "自动识别");
    present("#service-state", s.online ? "已连接" : s.connecting ? "连接中" : "离线");
    present("#diagnostics-summary", s.online && s.error ? "检测到下发错误 · 点按查看" : "真实状态、错误与只读诊断");
    present("#build-version", s.version ? "v" + s.version.replace(/^v/, "") : "v1.5.1");
    present("#footer-version", (s.version ? "v" + s.version.replace(/^v/, "") : "v1.5.1") + " · ZenJooo");
    renderRules(); renderSheetLive();
  }
  function emptyState(title, description, symbol = "apps", button = "") {
    return `<div class="empty-state">${icon(symbol)}<h3>${esc(title)}</h3><p>${esc(description)}</p>${button}</div>`;
  }
  function renderRules() {
    const query = $("#rule-search").value.trim().toLocaleLowerCase();
    const rules = s.rules.filter(rule => !query || (rule.package + " " + appInfo(rule.package).name).toLocaleLowerCase().includes(query));
    present("#rule-count", s.rules.length);
    present("#filtered-count", query ? `${rules.length} 个结果` : "");
    const key = JSON.stringify([query, editable(), !!s.pending, s.rulesLoaded, rules.map(r => [r.package,r.mode,appInfo(r.package).name])]);
    if (s.rulesRenderKey === key) return;
    s.rulesRenderKey = key;
    const root = $("#rule-list");
    if (!s.rulesLoaded) root.innerHTML = emptyState("等待应用规则", "成功连接后，这里会显示守护进程保存的应用策略。");
    else if (!s.rules.length) root.innerHTML = emptyState("从一个常用应用开始", "给游戏或视频应用设定独立档位，进入前台时自动切换。", "apps", `<button id="empty-add" class="secondary-button" type="button" ${editable() && !s.pending ? "" : "disabled"}>${icon("add")}添加应用</button>`);
    else if (!rules.length) root.innerHTML = emptyState("没有匹配的应用", "试试应用名称或包名的另一部分。", "search");
    else root.innerHTML = rules.map(rule => `<button class="rule-row" type="button" data-edit="${esc(rule.package)}" aria-label="${esc(appInfo(rule.package).name)}，${esc(MODES[rule.mode]?.name || "未知")}档，查看规则">${avatar(rule.package)}<span class="row-copy"><b>${esc(appInfo(rule.package).name)}</b><small>${esc(rule.package)}</small></span><span class="mode-badge">${esc(MODES[rule.mode]?.name || "未知")}</span>${icon("chevron_right")}</button>`).join("");
  }

  function parseStatus(text) {
    const result = {};
    for (const part of String(text).split(",")) { const i = part.indexOf(":"); if (i > 0) result[part.slice(0,i)] = part.slice(i+1); }
    return result;
  }
  function goodStatus(data) {
    const age = Number(data.heartbeatMs);
    return own(MODES, data.mode) && ["ready","degraded","suspended"].includes(data.phase) && data.heartbeatMs !== "" && Number.isFinite(age) && age >= 0;
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
  // Only read operations may negotiate an overloaded bridge signature.
  // Mutations use the signature established by the initial authenticated read.
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
    const received = (...args) => finish(...normalizeReply(args));
    window[callback] = received;
    const receive = value => { if (typeof value === "string" || value && typeof value === "object") received(value); };
    const first = bridgeSignature || (api.exec.length === 1 ? 1 : api.exec.length === 2 ? 2 : 3);
    for (let count = first; count >= 1; count--) {
      try {
        const result = count === 3 ? api.exec(command, "{}", callback)
          : count === 2 ? api.exec(command, callback) : api.exec(command + " 2>&1");
        bridgeSignature = count;
        if (result && typeof result.then === "function") result.then(receive, () => finish(1, "", "宿主执行失败，请检查 root 授权"));
        else receive(result);
        return;
      } catch (error) {
        if (!negotiate || count === 1 || !/argument|overload|signature|parameter/i.test(String(error?.message || error))) throw error;
      }
    }
  }
  function moduleCommand(command, suffix = "") {
    const candidates = [...new Set([MODULE, ...MODULE_CANDIDATES])].map(shellQuote).join(" ");
    return `for NOVA_PATH in ${candidates}; do if [ -x "$NOVA_PATH/bin/novasched" ] && grep -q '^id=NovaSched_Zen_Edition$' "$NOVA_PATH/module.prop"; then exec "$NOVA_PATH/bin/novasched" ${command} --module-dir "$NOVA_PATH" ${suffix}; fi; done; printf '%s\\n' 'novasched: 未找到本模块的可执行文件'; exit 1`;
  }
  function publicError(value) {
    // Never reflect successful credential JSON or a secret accidentally
    // echoed by an incompatible bridge. Render all remaining text via textContent.
    return String(value || "").replace(/[a-f0-9]{64}/gi, "[凭据已隐藏]")
      .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f]/g, " ").trim().slice(0, 900);
  }
  function extractCredentials(text) {
    const raw = String(text || "").trim();
    let parsed;
    try { parsed = JSON.parse(raw); } catch (_) {
      // Some hosts merge command output with shell noise; the daemon prints an
      // exact single object, so a strict embedded match is enough to recover it.
      const match = raw.match(/\{[^{}]*"port"\s*:\s*\d+[^{}]*"token"\s*:\s*"[a-f0-9]{64}"[^{}]*"origin"\s*:\s*"[^"]*"[^{}]*\}/);
      try { parsed = match ? JSON.parse(match[0]) : null; } catch (_) { parsed = null; }
    }
    return parsed && typeof parsed === "object" ? parsed : null;
  }
  function usableCredentials(value, origin) {
    return !!value && Number.isInteger(value.port) && value.port >= 1024 && value.port <= 65535
      && typeof value.token === "string" && /^[a-f0-9]{64}$/.test(value.token) && value.origin === origin;
  }
  function loadAuthentication(epoch, ready) {
    const api = bridge();
    const failed = (message, retry = true) => {
      if (epoch !== s.epoch || s.suspended) return;
      sessionToken = ""; s.connectionError = message;
      s.online = false; s.connecting = false; render(); feedback(message, true);
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
      failed("当前宿主没有开放 root 命令接口。请允许宿主的 root／Shell 权限；管理器没有 WebUI 入口时，可用 KsuWebUI 独立版打开本模块。", false); return;
    }
    const name = `nova_auth_${Date.now()}_${++bridgeCounter}`;
    const request = { name, timer:0, completed:false }; authRequest = request;
    const finish = (errno, stdout, stderr) => {
      if (request.completed) return;
      request.completed = true; clearTimeout(request.timer); delete window[name];
      if (authRequest === request) authRequest = null;
      if (epoch !== s.epoch || s.suspended) return;
      // print_current emits the credential JSON only after validating root,
      // daemon identity and page origin, so a complete credential in stdout is
      // authoritative even when the bridge misreports a nonzero exit code
      // (merged 2>&1 output, su wrappers, or an early WebView pipe close).
      // The WebSocket handshake still verifies the token against the live
      // daemon, so a fabricated or stale token simply fails closed.
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
      s.connectionError = ""; feedback(""); ready(credentials.port);
    };
    request.timer = setTimeout(() => finish(1, "", "等待宿主响应超过 30 秒，请完成 root 授权后重新连接。"), 30000);
    window[name] = finish;
    // Fixed root command; never send the credential to an HTTP endpoint.
    try { rootRead(moduleCommand("webui-session", `--origin ${shellQuote(origin)}`), name, finish); }
    catch (_) { finish(1, "", "宿主命令接口执行失败，请检查 root／Shell 授权。"); }
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
  function shellQuote(value) { return "'" + String(value).replace(/'/g, "'\\''") + "'"; }
  function cancelRootBridge() {
    clearTimeout(s.rootTimer); s.rootTimer = 0; s.rootJob = null; s.rootGeneration++;
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
      // Same tolerance as the credential read: a complete daemon snapshot is
      // authoritative even when the bridge misreports a nonzero exit code.
      return reply;
    });
  }
  function receiveRootSnapshot(reply) {
    if (reply.modes) {
      const status = parseStatus(reply.modes);
      if (!goodStatus(status)) throw new Error("守护心跳或状态未就绪");
      if (Number(status.heartbeatMs) >= 30000) throw new Error("守护心跳已过期");
      s.port = Number(status.port) || s.port; s.discovered = true;
      receiveModes(status);
      if (!s.pending && $("#mode-feedback").textContent === "正在通过宿主 root 通道连接守护进程…") feedback("");
    }
    if (reply.apps) receiveApps(reply.apps);
    if (s.page === "logs") {
      if (reply.logsReset) { s.logTail = ""; if (!s.logCursor) s.logs = []; }
      s.logCursor = reply.cursor;
      s.logConnected = true; s.logConnecting = false;
      if (reply.logs) receiveLogs(reply.logs); else renderLogs();
    }
  }
  function pollRootBridge(epoch) {
    if (epoch !== s.epoch || s.suspended || s.transport !== "root" || s.rootJob) return;
    const job = {}, generation = s.rootGeneration; s.rootJob = job;
    rootRequest({endpoint:"snapshot", logs:s.page === "logs", cursor:s.logCursor}).then(reply => {
      if (epoch !== s.epoch || s.suspended || generation !== s.rootGeneration) return;
      receiveRootSnapshot(reply);
    }).catch(error => {
      if (epoch !== s.epoch || s.suspended || generation !== s.rootGeneration) return;
      offline("root"); s.connectionError = "宿主备用通道失败：" + publicError(error.message); render(); feedback(s.connectionError, true);
    }).finally(() => {
      if (s.rootJob === job) s.rootJob = null;
      if (epoch === s.epoch && !s.suspended && s.transport === "root")
        s.rootTimer = setTimeout(() => pollRootBridge(epoch), 2000);
    });
  }
  function startRootBridge() {
    const epoch = ++s.epoch;
    clearTimeout(s.retry); cancelProbes(); cancelAuthentication(); cancelRootBridge();
    ["modes","apps","logs"].forEach(stopSocket);
    s.transport = "root"; s.online = false; s.connecting = true; s.discovered = false;
    s.connectionError = ""; s.logConnected = false; s.logConnecting = s.page === "logs";
    render(); feedback("正在通过宿主 root 通道连接守护进程…"); pollRootBridge(epoch);
  }
  function offline(reason, retry = true) {
    if (retry && s.transport !== "root" && !s.suspended && typeof bridge()?.exec === "function"
        && (reason === "handshake" || reason === "connection")) { startRootBridge(); return; }
    s.epoch++; cancelRootBridge(); s.transport = "";
    cancelAuthentication();
    s.online = false; s.connecting = false; s.discovered = false;
    s.connectionError = reason === "handshake" ? "已取得守护凭据，但 WebSocket 未连通。请读取启动诊断，并检查宿主是否允许本地连接。" : "本地连接已中断，请重新连接；若持续失败，请读取启动诊断。";
    stopSocket("modes"); stopSocket("apps"); stopSocket("logs"); s.logConnected = false; s.logConnecting = false;
    if (s.pending) finishPending(false, "连接中断，请重新连接后确认结果");
    if (s.sheet === "rule") present("#rule-error", "连接已中断；输入内容保留，重新连接后可继续保存。");
    render(); renderLogs(); feedback(s.connectionError, true);
    if (retry && !s.suspended) { clearTimeout(s.retry); s.retry = setTimeout(discover, 4000); }
  }
  // Ask the root bridge for the live daemon's port and ephemeral credential.
  // Never disclose a bearer credential while probing unrelated local ports.
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
          s.host = target.host; s.port = target.port; ui.port = target.port; saveUI(); s.discovered = true;
          bindSocket("modes", ws); receiveModes(status);
          openEndpoint("apps"); if (s.page === "logs") openEndpoint("logs");
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
      if (kind === "apps" && (s.pending?.kind === "rule" || s.pending?.kind === "delete")) finishPending(false, "应用规则连接中断，请重试");
      if (s.discovered && !s.suspended && (kind !== "logs" || s.page === "logs")) {
        const epoch = s.epoch;
        const retrySlot = { ws, deadline:0, retry:setTimeout(() => { if (epoch === s.epoch && !s.suspended) openEndpoint(kind); }, 2200) };
        s.sockets[kind] = retrySlot;
      }
    };
    const arm = () => { clearTimeout(slot.deadline); if (kind !== "logs") slot.deadline = setTimeout(fail, 33000); };
    ws.onopen = () => { if (!current()) return; if (kind === "logs") { s.logConnected = true; s.logConnecting = false; renderLogs(); } arm(); };
    ws.onmessage = event => {
      if (!current()) return;
      if (kind === "modes") { const parsed = parseStatus(event.data); if (!goodStatus(parsed)) return; receiveModes(parsed); }
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
      if (kind === "logs" && s.page === "logs") { s.logConnecting = !s.logConnected; renderLogs(); }
      if (!s.rootJob) { clearTimeout(s.rootTimer); pollRootBridge(s.epoch); }
      return;
    }
    if (!s.discovered || !sessionToken || s.suspended || (kind === "logs" && s.page !== "logs")) return;
    if (s.sockets[kind]?.ws?.readyState === WebSocket.OPEN || s.sockets[kind]?.ws?.readyState === WebSocket.CONNECTING) return;
    stopSocket(kind);
    const path = kind === "apps" ? "/app-modes" : "/logs";
    if (kind === "logs") { s.logConnecting = true; renderLogs(); }
    try { bindSocket(kind, new WebSocket(`ws://${s.host}:${s.port}${path}`, [KEY, AUTH_PREFIX + sessionToken])); }
    catch (_) { if (kind === "logs") { s.logConnecting = false; renderLogs(); } }
  }
  function receiveModes(data) {
    s.connectionError = "";
    s.mode = data.mode; s.effective = own(MODES, data.effective) ? data.effective : "";
    s.pkg = data.package || ""; s.controller = data.controller || "NovaSched";
    s.external = data.sceneActive === "true"; s.linked = data.sceneLinked === "true";
    s.phase = data.phase; s.heartbeat = Number(data.heartbeatMs); s.lastFrame = Date.now();
    s.online = s.heartbeat < 30000; s.connecting = false;
    s.smooth = data.smoothPowerSave === "true"; s.extreme = data.extremePowerSave === "true";
    s.powerProfile = data.powerSaveProfile || "";
    s.soc = data.socName || ""; s.socId = data.socId || ""; s.configProfile = data.configProfile || "";
    s.smoothSupported = own(data,"smoothSupported") ? data.smoothSupported === "true" : null;
    s.extremeSupported = own(data,"extremeSupported") ? data.extremeSupported === "true" : null;
    loadLabels([s.pkg]); confirmPending(); render();
  }
  function receiveApps(text) {
    try {
      const data = JSON.parse(text);
      if (data.type !== "app-modes" || !Array.isArray(data.rules)) return;
      s.rules = data.rules.filter(r => r && validPackage(r.package) && own(MODES,r.mode));
      s.rulesLoaded = true;
      s.error = typeof data.error === "string" ? data.error : "";
      s.version = typeof data.version === "string" ? data.version : "";
      // mode endpoint owns liveness; an app snapshot cannot manufacture it.
      // The modes stream alone owns controller state. A delayed rules frame
      // must not restore stale Scene ownership after an uninstall.
      s.smooth = data.smoothPowerSave === true; s.extreme = data.extremePowerSave === true;
      if (typeof data.powerSaveProfile === "string") s.powerProfile = data.powerSaveProfile;
      if (own(MODES,data.defaultMode)) s.mode = data.defaultMode;
      loadLabels(s.rules.map(r => r.package)); confirmPending(); render();
    } catch (_) { toast("应用规则数据无法解析，请重连后再试"); }
  }
  function send(kind, message) {
    if (!editable()) { toast(s.external ? "Scene 外部调度器接管中，请先在 Scene 停用它的调度" : "请先连接本地守护进程"); return false; }
    if (s.transport === "root") {
      const epoch = s.epoch, generation = ++s.rootGeneration;
      clearTimeout(s.rootTimer);
      rootRequest({endpoint:kind === "apps" ? "apps" : "modes", message}).then(reply => {
        if (epoch === s.epoch && generation === s.rootGeneration && !s.suspended) receiveRootSnapshot(reply);
      }).catch(error => {
        if (epoch === s.epoch && generation === s.rootGeneration) finishPending(false, "操作未确认：" + publicError(error.message));
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
    if (s.pending) return false;
    const endpoint = kind === "rule" || kind === "delete" ? "apps" : "modes";
    const message = kind === "mode" ? value : kind === "rule" ? `set\t${pkg}\t${value}` : kind === "delete" ? `delete\t${pkg}` : `${kind}\t${value ? "1" : "0"}`;
    // Set before sending: a synchronous host/test reply must not get lost.
    s.pending = { kind,value,pkg, baselineError:s.error };
    if (!send(endpoint,message)) { s.pending = null; render(); return false; }
    if (kind === "mode") feedback(`正在请求${MODES[value].name}档，等待守护进程反馈…`);
    pendingTimer = setTimeout(() => {
      if (!s.pending) return;
      const p = s.pending;
      finishPending(false, p.kind === "mode" && s.mode === p.value ? "默认档位已保存，但尚未确认实际生效；请查看应用规则或诊断" : "未收到确认，状态未标记为成功，请重连后检查");
    }, s.transport === "root" ? 30000 : 10000);
    render(); return true;
  }
  function confirmPending() {
    const p = s.pending;
    if (!p) return;
    if (s.external) return finishPending(false, "Scene 外部调度器接管中，档位请以 Scene 为准");
    if (!s.online) return;
    if (p.kind === "mode") {
      if (s.phase === "degraded" && s.mode === p.value) return finishPending(false, s.error || "策略下发失败，已保留最后成功档位");
      if (s.phase === "ready" && s.mode === p.value && s.effective === p.value) return finishPending(true, `${MODES[p.value].name}档已生效`);
      const rule = s.rules.find(r => r.package === s.pkg) || s.rules.find(r => r.package === basePackage(s.pkg));
      if (s.mode === p.value && rule && rule.mode === s.effective && s.effective !== p.value && s.phase === "ready") return finishPending(true, `${MODES[p.value].name}已设为默认；当前应用规则优先，仍使用${MODES[s.effective].name}档`);
    } else if (p.kind === "smooth" || p.kind === "extreme") {
      if (s[p.kind] === p.value) finishPending(true, "选项已保存；生效状态以设置页反馈为准");
    } else if (p.kind === "rule" && s.rules.some(r => r.package === p.pkg && r.mode === p.value)) finishPending(true, "应用规则已保存");
    else if (p.kind === "delete" && !s.rules.some(r => r.package === p.pkg)) finishPending(true, "应用规则已删除，今后沿用默认档位");
  }
  function finishPending(success, message) {
    const p = s.pending; clearTimeout(pendingTimer); pendingTimer = 0; s.pending = null;
    if (p?.kind === "mode") feedback(message, !success);
    if (success && (p?.kind === "rule" || p?.kind === "delete")) closeSheet();
    if (!success && (p?.kind === "rule" || p?.kind === "delete")) {
      const err = $("#rule-error"); if (err) { err.hidden = false; err.textContent = message; }
    }
    toast(message, 4300); render();
  }

  function setPage(page, updateHistory = true) {
    if (!["home","apps","logs","settings"].includes(page)) page = "home";
    const changed = s.page !== page;
    if (changed) scrollPositions[s.page] = window.scrollY;
    s.page = page;
    $$(".page").forEach(node => { node.hidden = node.id !== page; });
    $$(".nav-item").forEach(button => { if (button.dataset.page === page) button.setAttribute("aria-current","page"); else button.removeAttribute("aria-current"); });
    $(".glass-dock").style.setProperty("--tab", ["home","apps","logs","settings"].indexOf(page));
    if (updateHistory && location.hash !== "#" + page) history.pushState({page}, "", "#" + page);
    if (page === "logs") openEndpoint("logs"); else { stopSocket("logs"); s.logConnected = false; s.logConnecting = false; }
    if (changed) {
      window.scrollTo({ top:scrollPositions[page] || 0, behavior:"instant" });
      $("h1", $("#" + page)).focus({preventScroll:true});
    }
    render(); renderLogs();
  }
  function openSheet(kind, title, kicker, body) {
    const dialog = $("#sheet");
    if (!dialog.open) previousFocus = document.activeElement;
    s.sheet = kind; s.sheetEpoch++; present("#sheet-title",title); present("#sheet-kicker",kicker);
    $("#sheet-body").innerHTML = body; dialog.scrollTop = 0;
    if (!dialog.open) { dialog.showModal(); document.body.classList.add("dialog-open"); }
  }
  function closeSheet() {
    s.sheet = ""; s.sheetEpoch++; s.edit = null; $("#sheet").close(); document.body.classList.remove("dialog-open");
    previousFocus?.focus?.({preventScroll:true});
  }
  function detailRow(title, value, id = "") {
    return `<div class="detail-row"><dt>${esc(title)}</dt><dd ${id ? `id="${esc(id)}"` : ""}>${esc(value)}</dd></div>`;
  }
  function showForeground() {
    if (!s.online || !s.pkg) return;
    const raw = s.pkg; const a = appInfo(raw);
    openSheet("foreground", "前台应用", "正在使用", `<div class="detail-app">${avatar(raw)}<span class="row-copy"><b>${esc(a.name)}</b><span>${a.process ? "应用子进程" : a.system ? "系统应用" : "当前前台"}${a.version ? " · " + esc(a.version) : ""}</span></span></div><p>${a.known ? "应用名称来自设备信息；完整进程标识保留在下方。" : "当前管理器未提供这个应用的名称，完整标识保留在下方。"}</p><p class="package-detail">${esc(raw)}</p><div class="sheet-actions"><button id="copy-package" class="secondary-button" type="button">${icon("content_copy")}复制标识</button><button id="foreground-rule" class="primary-button" type="button" ${editable() ? "" : "disabled"}>配置规则</button></div>${a.process ? '<p class="sheet-caption">新规则默认使用主应用包名，以覆盖该应用的正常前台场景。</p>' : ""}`);
    $("#copy-package").onclick = () => copyText(raw,"完整标识已复制");
    $("#foreground-rule").onclick = () => showRule(a.base);
  }
  function showConnection() {
    openSheet("connection", "本地连接", "只在这台设备运行", `<p>页面通过管理器读取本机守护的连接凭据与真实状态。连接失败与进程退出需要分别诊断。</p><dl class="detail-list">${detailRow("服务状态",stateLabel(),"detail-state")}${detailRow("页面宿主","具备 root 命令接口的 WebUI")}${detailRow("服务地址",connectionAddress(),"detail-address")}${detailRow("控制来源",sourceLabel(),"detail-source")}${detailRow("调度心跳",s.online ? `${s.heartbeat} ms` : "尚未确认","detail-heartbeat")}</dl><p id="connection-error" class="package-detail">${esc(s.connectionError)}</p><p class="sheet-caption">Magisk / Alpha 如没有内置入口，可使用 KsuWebUI 独立版或 WebUI X，并授予宿主 root／Shell 权限。</p><p class="sheet-caption">页面优先使用本地连接；连接受限时自动使用已授权宿主的 root 通道。</p><div class="sheet-actions"><button id="reconnect" class="primary-button" type="button">${icon("refresh")}重新连接</button><button id="open-diagnostics" class="secondary-button" type="button">查看诊断</button></div>`);
    $("#reconnect").onclick = () => { closeSheet(); discover(); toast("正在重新发现本地服务"); };
    $("#open-diagnostics").onclick = showDiagnostics;
  }
  function showSource() {
    openSheet("source", "控制来源", "规则如何生效", `<dl class="detail-list">${detailRow("当前来源",sourceLabel(),"detail-source")}${detailRow("默认档位",MODES[s.mode]?.name || "等待连接","detail-default")}${detailRow("实际档位",s.online ? s.external ? "由外部调度器控制" : MODES[s.effective]?.name || "等待确认" : "等待连接","detail-effective")}</dl><p>${s.external ? "Scene 当前选用其他调度器。NovaSched 暂停节点写入，本页切档与规则编辑保持只读。" : s.linked ? "Scene 已接入 NovaSched 回调：调度执行由 Rust 守护进程完成，档位和应用规则在 Scene 与本页之间双向同步，任意一侧修改都会即时生效。Scene 的统计和采样仍由 Scene 自己完成。" : "应用专属规则优先于默认档位。点击概览的档位会更新默认选择；命中应用规则时，实际档位可能暂时不同。"}</p>`);
  }
  function renderSheetLive() {
    if (s.sheet === "connection") {
      present("#connection-error",s.connectionError);
      present("#detail-state",stateLabel()); present("#detail-address",connectionAddress());
      present("#detail-source",sourceLabel()); present("#detail-heartbeat",s.online ? `${s.heartbeat} ms` : "尚未确认");
    }
    if (s.sheet === "source") {
      present("#detail-source",sourceLabel()); present("#detail-default",MODES[s.mode]?.name || "等待连接");
      present("#detail-effective",s.online ? s.external ? "由外部调度器控制" : MODES[s.effective]?.name || "等待确认" : "等待连接");
    }
    if (s.sheet === "rule") {
      const lock = !editable() || !!s.pending;
      $$("#rule-editor input,#rule-editor button").forEach(node => { node.disabled = lock; });
      const note = $("#rule-lock"); if (note) { note.hidden = editable(); note.textContent = s.external ? "Scene 外部调度器接管中，规则暂不可修改。" : "当前离线，输入会保留；重连后可保存。"; }
      const save = $("#rule-save"); if (save) save.textContent = s.pending?.kind === "rule" ? "保存中…" : "保存应用规则";
      const remove = $("#rule-delete"); if (remove) remove.disabled = lock;
    }
    if (s.sheet === "delete") {
      const confirm = $("#delete-confirm");
      if (confirm) { confirm.disabled = !editable() || !!s.pending; confirm.textContent = s.pending?.kind === "delete" ? "删除中…" : "删除规则"; }
      const cancel = $("#delete-cancel"); if (cancel) cancel.disabled = !!s.pending;
    }
  }
  function showRule(pkg = "") {
    const existing = s.rules.find(rule => rule.package === pkg);
    const a = appInfo(pkg);
    s.edit = { pkg, mode:existing?.mode || s.mode || "balance" };
    openSheet("rule", existing ? "编辑应用规则" : "创建应用规则", pkg ? a.name : "手动输入", `<form id="rule-editor" autocomplete="off">${pkg ? `<div class="detail-app">${avatar(pkg)}<span class="row-copy"><b>${esc(a.name)}</b><span>为这个应用指定调度档位</span></span></div>` : ""}<label class="sheet-field-label" for="package-input">应用包名</label><input id="package-input" class="text-input" value="${esc(pkg)}" type="text" inputmode="url" autocapitalize="off" autocorrect="off" spellcheck="false" maxlength="255" placeholder="com.example.app" ${existing ? "readonly" : ""}><p id="package-error" class="input-error" hidden></p><p class="input-help">应用进入前台时，规则优先于默认档位。</p><span class="sheet-field-label">目标档位</span><div class="mode-options" role="group" aria-label="应用目标档位">${Object.entries(MODES).map(([mode,info]) => `<button class="mode-option" data-rule-mode="${mode}" type="button" aria-pressed="${s.edit.mode === mode}">${icon(info.icon)}${info.name}</button>`).join("")}</div><p id="rule-lock" class="sheet-alert" hidden></p><p id="rule-error" class="sheet-error" hidden role="alert"></p><div class="sheet-actions"><button id="rule-save" class="primary-button wide" type="submit">保存应用规则</button></div></form>${existing ? '<button id="rule-delete" class="text-button wide" type="button">删除这条规则</button>' : ""}`);
    // openSheet does not reset edit; the form remains intact through heartbeats.
    $$("[data-rule-mode]").forEach(button => button.onclick = () => {
      s.edit.mode = button.dataset.ruleMode;
      $$("[data-rule-mode]").forEach(node => node.setAttribute("aria-pressed",String(node === button)));
    });
    $("#rule-editor").onsubmit = event => {
      event.preventDefault(); const value = $("#package-input").value.trim();
      const field = $("#package-input"), error = $("#package-error");
      if (!validPackage(value)) { field.setAttribute("aria-invalid","true"); error.hidden = false; error.textContent = "请输入有效的 Android 包名，例如 com.example.app。"; field.focus(); return; }
      field.removeAttribute("aria-invalid"); error.hidden = true;
      beginPending("rule",s.edit.mode,value);
    };
    if (existing) $("#rule-delete").onclick = () => {
      openSheet("delete", "删除应用规则？", a.name, `<p>删除后，这个应用沿用默认档位。其他应用规则保留。</p><p class="package-detail">${esc(pkg)}</p><p id="rule-error" class="sheet-error" hidden></p><div class="sheet-actions"><button id="delete-cancel" class="secondary-button" type="button">返回编辑</button><button id="delete-confirm" class="danger-button" type="button">删除规则</button></div>`);
      $("#delete-cancel").onclick = () => showRule(pkg);
      $("#delete-confirm").onclick = () => { if (beginPending("delete",null,pkg)) $("#delete-confirm").disabled = true; };
    };
    renderSheetLive();
  }

  async function loadInstalled() {
    if (s.installed || s.loadingApps) return;
    s.loadingApps = true;
    try {
      const api = metadataBridge();
      if (typeof api?.listPackages !== "function" || typeof api?.getPackagesInfo !== "function") { s.installed = []; return; }
      const list = JSON.parse(api.listPackages("all"));
      if (!Array.isArray(list)) { s.installed = []; return; }
      const packages = [...new Set(list)].filter(validPackage).slice(0,4000);
      s.installed = [];
      for (let i = 0; i < packages.length && !s.suspended; i += 48) {
        const batch = packages.slice(i,i+48); loadLabels(batch); s.installed.push(...batch);
        if (s.sheet === "picker") renderPicker();
        await new Promise(resolve => setTimeout(resolve,0));
      }
      if (s.suspended) { s.installed = null; return; }
      s.installed.sort((a,b) => Number(appInfo(a).system) - Number(appInfo(b).system) || appInfo(a).name.localeCompare(appInfo(b).name,"zh-CN"));
    } catch (_) { s.installed = []; }
    finally { s.loadingApps = false; if (s.sheet === "picker") renderPicker(); }
  }
  function showPicker() {
    if (!editable() || s.pending) return;
    openSheet("picker", "选择应用", "创建专属策略", `<div class="search-box">${icon("search")}<input id="picker-search" type="search" placeholder="应用名称或包名" aria-label="搜索已安装应用" autocomplete="off"></div><div class="picker-tools"><span id="picker-count">正在读取应用…</span><button id="manual-package" class="text-button" type="button">手动输入包名</button></div><div id="picker-list" class="picker-list"></div>`);
    $("#picker-search").oninput = renderPicker;
    $("#manual-package").onclick = () => showRule();
    renderPicker(); void loadInstalled();
  }
  function renderPicker() {
    if (s.sheet !== "picker") return;
    const query = $("#picker-search").value.trim().toLocaleLowerCase();
    const list = (s.installed || []).filter(pkg => !query || (pkg + " " + appInfo(pkg).name).toLocaleLowerCase().includes(query));
    const shown = list.slice(0,80);
    present("#picker-count", s.loadingApps ? "正在读取应用名称…" : `${list.length} 个应用${list.length > 80 ? " · 搜索可定位更多" : ""}`);
    $("#picker-list").innerHTML = shown.length ? shown.map(pkg => `<button class="rule-row" type="button" data-pick="${esc(pkg)}">${avatar(pkg)}<span class="row-copy"><b>${esc(appInfo(pkg).name)}</b><small>${esc(pkg)}</small></span>${icon("chevron_right")}</button>`).join("") : emptyState(s.loadingApps ? "正在读取应用" : "没有可显示的应用", s.loadingApps ? "名称由设备的应用管理接口提供。" : query ? "换一个名称或使用手动包名输入。" : "当前管理器未提供应用列表，你仍可手动输入包名。", "search");
  }
  function showTheme() {
    openSheet("theme","外观","选择舒适的显示方式", `<div class="choice-list">${Object.entries(THEME_NAMES).map(([key,name]) => `<button class="choice" type="button" data-theme-choice="${key}" aria-pressed="${ui.theme === key}"><span>${name}</span>${icon("check")}</button>`).join("")}</div><p class="sheet-caption">外观选择仅影响本页，保存在当前 WebView。</p>`);
    $$("[data-theme-choice]").forEach(button => button.onclick = () => { ui.theme = button.dataset.themeChoice; saveUI(); applyAppearance(); closeSheet(); toast("外观已切换为" + THEME_NAMES[ui.theme]); });
  }
  function showAbout() {
    openSheet("about","NovaSched Zen Edition","作者 ZenJooo", `<dl class="detail-list">${detailRow("模块版本",s.version ? "v" + s.version.replace(/^v/,"") : "未连接")}${detailRow("界面版本","1.5.1")}${detailRow("当前处理器",s.online ? s.soc || "待识别" : "未连接")}${detailRow("配置文件",s.online ? s.configProfile || "待识别" : "未连接")}${detailRow("配置格式","NovaSched 2")}${detailRow("许可证","GPL-3.0-only")}</dl><p>按处理器和内核能力映射 CPU 策略，支持应用单独设置和 Scene 联动。流畅省电与极限节能默认关闭。</p><p class="sheet-caption">覆盖安装后请重启，让新的调度核心生效。开源许可与图标授权见模块内 NOTICE。</p>`);
  }
  function rootExec(command, timeout = 12000, negotiate = true) {
    return new Promise((resolve,reject) => {
      const api = bridge(); if (typeof api?.exec !== "function") return reject(new Error("当前管理器没有提供命令接口"));
      const name = `nova_exec_${Date.now()}_${++bridgeCounter}`;
      let completed = false;
      const finish = (err,result) => { if (completed) return; completed = true; clearTimeout(timer); delete window[name]; err ? reject(err) : resolve(result); };
      const timer = setTimeout(() => finish(new Error("只读诊断未在限定时间内完成")),timeout);
      window[name] = (errno,stdout,stderr) => finish(null,{errno:Number(errno),stdout:String(stdout || ""),stderr:String(stderr || "")});
      try { rootRead(command,name,window[name],negotiate); } catch (err) { finish(err); }
    });
  }
  function showDiagnostics() {
    const diagnostic = s.online ? "game-diagnose" : "diagnose";
    const diagnosticError = s.connectionError || s.error;
    const command = `su -c ${MODULE}/bin/novasched ${diagnostic}`;
    openSheet("diagnostics","诊断信息","保留真实错误", `<dl class="detail-list">${detailRow("调度阶段",stateLabel())}${detailRow("控制来源",sourceLabel())}${detailRow("当前处理器",s.online ? s.soc || "待识别" : "未连接")}${detailRow("配置模板",s.online ? s.configProfile || "待识别" : "未连接")}${detailRow("已保存默认档位",MODES[s.mode]?.name || "等待连接")}${detailRow("最近实际档位",s.online && !s.external ? MODES[s.effective]?.name || "待确认" : "未确认")}</dl><p>${diagnosticError ? "当前报告的错误：" : "当前连接没有报告具体的下发错误。可读取模块诊断进一步确认。"}</p>${diagnosticError ? `<pre class="report-output">${esc(diagnosticError)}</pre>` : ""}<div class="sheet-actions"><button id="read-diagnostics" class="primary-button" type="button" ${typeof bridge()?.exec === "function" ? "" : "disabled"}>读取只读诊断</button><button id="copy-diagnostic-command" class="secondary-button" type="button">复制命令</button></div><pre id="diagnostic-report" class="report-output" hidden></pre><p class="package-detail">${esc(command)}</p><p class="sheet-caption">${s.online ? "采样约 10 秒，读取实际节点与温度。" : "读取开机入口、进程身份与启动失败记录。"}仅诊断，不改变调度参数。命令适用于 MT 管理器终端。</p>`);
    $("#copy-diagnostic-command").onclick = () => copyText(command,"MT 管理器诊断命令已复制");
    $("#read-diagnostics").onclick = async () => {
      const epoch = s.sheetEpoch; const button = $("#read-diagnostics"); button.disabled = true; button.textContent = "读取中…";
      // Fixed command: no app label, package or user input enters the shell.
      try {
        const result = await rootExec(moduleCommand(diagnostic),30000);
        if (epoch !== s.sheetEpoch) return;
        const report = $("#diagnostic-report"); report.hidden = false; report.textContent = result.stdout + (result.stderr ? "\n" + result.stderr : "") || `诊断退出码：${result.errno}`;
      } catch (error) {
        if (epoch !== s.sheetEpoch) return;
        const report = $("#diagnostic-report"); report.hidden = false; report.textContent = error.message;
      } finally { if (epoch === s.sheetEpoch) { button.disabled = false; button.textContent = "重新读取诊断"; } }
    };
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
    // Bounded memory: a long-running diagnostics view must not grow forever.
    if (s.logTail.length > 12000) { s.logs.push(parseLog(s.logTail.slice(0,12000))); s.logTail = ""; }
    if (s.logs.length > 1200) s.logs.splice(0,s.logs.length-1000);
    renderLogs();
  }
  function visibleLogs() {
    const query = $("#log-search").value.trim().toLocaleLowerCase();
    const complete = s.logTail ? [...s.logs,parseLog(s.logTail)] : s.logs;
    return complete.filter(line => (s.level === "all" || line.level === s.level) && (!query || line.raw.toLocaleLowerCase().includes(query)));
  }
  function renderLogs() {
    present("#log-state", s.logConnected ? "实时记录中" : s.logConnecting ? "连接日志流…" : s.page !== "logs" ? "按需连接" : s.online ? "等待日志连接" : "本地服务未连接");
    const lines = visibleLogs();
    const errors = s.logs.filter(line => line.level === "error").length;
    present("#error-count", errors ? String(errors) : "");
    present("#log-count", $("#log-search").value || s.level !== "all" ? `${lines.length} / ${s.logs.length} 条` : `${s.logs.length} 条记录`);
    if (s.page !== "logs") return;
    const output = $("#log-output");
    const oldScroll = output.scrollTop;
    const key = JSON.stringify([s.level,$("#log-search").value,s.logConnected,s.logConnecting,s.logReceived,lines.length,lines[0]?.raw,lines[lines.length - 1]?.raw]);
    if (key === s.logsRenderKey) return;
    s.logsRenderKey = key;
    output.innerHTML = lines.length ? lines.map(line => `<article class="log-entry" data-level="${line.level}"><div class="log-meta"><time>${esc(line.time || "本地记录")}</time><span class="log-tag">${({error:"错误",warn:"警告",info:"信息"})[line.level]}</span></div><div class="log-message">${esc(line.message)}</div></article>`).join("") : emptyState(s.logReceived ? "暂时没有匹配的记录" : "等待运行记录", s.logReceived ? "可清除筛选条件，或等待新的日志。" : "进入日志页后按需连接，不在后台持续订阅。", "description");
    output.scrollTop = s.follow ? output.scrollHeight : oldScroll;
  }
  function showLogTools() {
    openSheet("log-tools","日志工具","仅操作当前视图", `<div class="tool-list"><button id="copy-log-view" class="secondary-button" type="button">${icon("content_copy")}复制筛选后的日志</button><button id="export-logs" class="secondary-button" type="button">${icon("download")}导出为文本文件</button><button id="clear-log-view" class="secondary-button" type="button">${icon("delete_outline")}清空当前视图</button></div><p class="sheet-caption">内部日志保留在 /data/adb/novasched/novasched.log。</p>`);
    $("#copy-log-view").onclick = () => copyText(visibleLogs().map(l => l.raw).join("\n"),"筛选日志已复制");
    $("#export-logs").onclick = () => {
      const text = visibleLogs().map(l => l.raw).join("\n"); if (!text) return toast("没有可导出的日志");
      try {
        const url = URL.createObjectURL(new Blob([text + "\n"],{type:"text/plain;charset=utf-8"}));
        const a = document.createElement("a"); a.href = url; a.download = "NovaSched-log.txt"; document.body.append(a); a.click(); a.remove();
        setTimeout(() => URL.revokeObjectURL(url),5000); toast("已发起文本下载；若管理器不支持，可使用复制日志");
      } catch (_) { toast("当前管理器不支持文本下载，请使用复制日志"); }
    };
    $("#clear-log-view").onclick = () => { s.logs = []; s.logTail = ""; s.logReceived = true; s.logsRenderKey = ""; renderLogs(); closeSheet(); toast("当前视图已清空"); };
  }

  $$(".profile").forEach(button => button.onclick = () => { if (button.dataset.mode === s.mode && button.dataset.mode === s.effective) toast("当前档位已生效"); else beginPending("mode",button.dataset.mode); });
  $$(".nav-item").forEach(button => button.onclick = () => setPage(button.dataset.page));
  $(".brand").onclick = event => { event.preventDefault(); setPage("home"); };
  $("#connection").onclick = $("#connection-details").onclick = showConnection;
  $("#foreground-card").onclick = showForeground;
  $("#source-card").onclick = showSource;
  $("#power-shortcut").onclick = () => { setPage("settings"); $("#smooth-save").focus({preventScroll:true}); };
  $("#add-rule").onclick = showPicker;
  $("#current-rule").onclick = () => showRule(basePackage(s.pkg));
  $("#rule-search").oninput = () => { $(`[data-clear="rule-search"]`).hidden = !$("#rule-search").value; renderRules(); };
  $("#rule-list").onclick = event => { const row = event.target.closest("[data-edit]"); if (row) showRule(row.dataset.edit); else if (event.target.closest("#empty-add")) showPicker(); };
  $("#sheet-body").addEventListener("click",event => { const row = event.target.closest("[data-pick]"); if (row && editable()) showRule(row.dataset.pick); });
  for (const name of ["smooth","extreme"]) $("#" + name + "-save").onchange = event => { if (!beginPending(name,event.target.checked)) render(); };
  $("#glass-effects").onchange = event => { ui.glass = event.target.checked; saveUI(); applyAppearance(); };
  // Guarded: the slider row may be absent when a newer script runs against an
  // older index.html (e.g. a manual in-place webroot hot swap).
  const glassSlider = $("#glass-intensity");
  if (glassSlider) {
    glassSlider.oninput = event => {
      const value = Number(event.target.value);
      if (!Number.isFinite(value)) return;
      ui.glassLevel = Math.min(100, Math.max(0, Math.round(value)));
      // Live preview while dragging; persistence happens once on change.
      document.documentElement.style.setProperty("--glass-level", String(ui.glassLevel));
      event.target.style.setProperty("--fill", ui.glassLevel + "%");
      present("#glass-value", ui.glassLevel + "%");
      present("#glass-note", glassNote(ui.glassLevel));
    };
    glassSlider.onchange = () => saveUI();
  }
  $("#reduce-motion").onchange = event => { ui.motion = event.target.checked; saveUI(); applyAppearance(); };
  $("#theme-picker").onclick = showTheme;
  $("#diagnostics").onclick = showDiagnostics;
  $("#about").onclick = showAbout;
  $("#log-menu").onclick = showLogTools;
  $("#log-search").oninput = () => { $(`[data-clear="log-search"]`).hidden = !$("#log-search").value; renderLogs(); };
  $$("[data-clear]").forEach(button => button.onclick = () => { const input = $("#" + button.dataset.clear); input.value = ""; input.oninput(); input.focus(); });
  $$(".segmented [data-level]").forEach(button => button.onclick = () => { s.level = button.dataset.level; $$(".segmented [data-level]").forEach(node => node.setAttribute("aria-pressed",String(node === button))); renderLogs(); });
  $("#log-follow").onclick = () => {
    s.follow = !s.follow; $("#log-follow").setAttribute("aria-pressed",String(s.follow));
    $("#follow-icon").setAttribute("href",s.follow ? "#i-pause" : "#i-play_arrow");
    $("#log-follow span").textContent = s.follow ? "跟随" : "已暂停";
    if (s.follow) $("#log-output").scrollTop = $("#log-output").scrollHeight;
  };
  $("#snackbar-dismiss").onclick = () => $("#snackbar").classList.remove("show");
  $("#sheet-close").onclick = closeSheet;
  $("#sheet").addEventListener("cancel",event => { event.preventDefault(); closeSheet(); });
  $("#sheet").addEventListener("click",event => {
    if (event.target !== $("#sheet")) return;
    const rect = event.target.getBoundingClientRect();
    if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) closeSheet();
  });
  window.addEventListener("popstate",() => { if (s.sheet) closeSheet(); setPage(location.hash.slice(1),false); });
  darkQuery?.addEventListener?.("change",applyAppearance);
  motionQuery?.addEventListener?.("change",applyAppearance);
  function suspend() {
    cancelRootBridge();
    if (s.suspended) return;
    s.suspended = true; s.epoch++; clearTimeout(s.retry); cancelProbes(); cancelAuthentication();
    ["modes","apps","logs"].forEach(stopSocket); s.logConnected = false; s.logConnecting = false;
    s.online = false; s.connecting = false; s.discovered = false;
    if (s.pending) finishPending(false,"页面进入后台，返回后请确认操作结果");
  }
  function resume() { if (s.suspended) { s.suspended = false; discover(); if (s.sheet === "picker") void loadInstalled(); } }
  document.addEventListener("visibilitychange",() => document.hidden ? suspend() : resume());
  window.addEventListener("pagehide",suspend);
  window.addEventListener("pageshow",resume);
  applyAppearance(); setPage(location.hash.slice(1) || "home",false); discover();
})();
