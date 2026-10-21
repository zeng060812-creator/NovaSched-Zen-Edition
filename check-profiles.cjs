#!/usr/bin/env node
/* 本地复刻 generate-profiles.py 的生成逻辑并做字节级一致性检查。
 * CI 里跑 `python3 scripts/generate-profiles.py --check`；两边表格必须同步修改。 */
"use strict";
const fs = require("fs");
const path = require("path");

const ROOT = path.join(__dirname, "..");
const LAYOUTS = {
  SM8450: [0, 4, 7, -1], SM8475: [0, 4, 7, -1],
  SM8550: [0, 3, 7, -1], SM8650: [0, 2, 5, 7],
  SM8750: [0, 6, -1, -1], SM8850: [0, 6, -1, -1],
  SM8845: [0, 6, -1, -1],
  DIMENSITY8350: [0, 4, 7, -1], DIMENSITY8400: [0, 4, 7, -1],
  DIMENSITY9400PLUS: [0, 4, 7, -1],
  MT6989: [0, 4, 7, -1],
  XRING_O3: [4, 0, 1, -1],
};
const CEILINGS = {
  SM8450:            [[54, 46, 44, 0],  [60, 54, 52, 0],  [84, 80, 80, 0],  [92, 98, 100, 0]],
  SM8475:            [[54, 46, 44, 0],  [60, 54, 52, 0],  [84, 80, 80, 0],  [92, 98, 100, 0]],
  SM8550:            [[54, 46, 46, 0],  [60, 54, 54, 0],  [84, 82, 82, 0],  [92, 98, 100, 0]],
  SM8650:            [[60, 45, 49, 45], [70, 58, 56, 59], [85, 84, 85, 82], [95, 100, 100, 100]],
  SM8750:            [[52, 46, 0, 0],   [58, 50, 0, 0],   [80, 72, 0, 0],   [90, 100, 0, 0]],
  SM8850:            [[52, 46, 0, 0],   [58, 50, 0, 0],   [80, 70, 0, 0],   [90, 100, 0, 0]],
  /* SM8845（骁龙 8 Gen 5，一加 PLR110 真机：6+2 Oryon）：两簇差异更收敛。 */
  SM8845:            [[52, 45, 0, 0],   [60, 52, 0, 0],   [82, 74, 0, 0],   [92, 100, 0, 0]],
  DIMENSITY8350:     [[56, 48, 46, 0],  [62, 56, 54, 0],  [86, 84, 84, 0],  [94, 100, 100, 0]],
  DIMENSITY8400:     [[56, 48, 46, 0],  [62, 56, 54, 0],  [86, 84, 84, 0],  [94, 100, 100, 0]],
  DIMENSITY9400PLUS: [[52, 46, 44, 0],  [58, 52, 50, 0],  [80, 78, 80, 0],  [90, 100, 100, 0]],
  /* MT6989（天玑 9300+）极限性能取向：性能/极速档不做省电甜点，不永久锁最高频。 */
  MT6989:            [[60, 42, 38, 0],  [70, 60, 55, 0],  [90, 92, 100, 0], [100, 100, 100, 0]],
  XRING_O3:          [[54, 46, 48, 0],  [60, 52, 54, 0],  [82, 74, 80, 0],  [92, 100, 100, 0]],
};
const FAS_MODES = {
  powersave: { ceiling: "28", freq_scale: "25" },
  balance: { ceiling: "45", freq_scale: "45" },
  performance: { ceiling: "80", freq_scale: "60" },
  fast: { ceiling: "95", freq_scale: "80" },
};
/* 每颗处理器显式写入最优调速器（不再是 auto）：
 * 骁龙 = walt（WALT 窗口负载跟踪，小米 14 真机确认 OEM 默认即 walt）；
 * 天玑 / 玄戒 = schedutil（MTK 系唯一现代全簇调速器）。
 * 内核缺失时运行时自动回退 OEM 调速器（hardware::select_governor）。 */
const GOVERNORS = {
  SM8450: "walt", SM8475: "walt", SM8550: "walt",
  SM8650: "walt", SM8750: "walt", SM8850: "walt", SM8845: "walt",
  DIMENSITY8350: "schedutil", DIMENSITY8400: "schedutil",
  DIMENSITY9400PLUS: "schedutil", MT6989: "schedutil",
  XRING_O3: "schedutil",
};

/* ---- 天玑 9300+（MT6989）极限性能策略覆写 ----------------------------- */
const MODE_MIN_OVERRIDES = {
  MT6989: { fast: ["25%", "30%", "35%", "0"] },
};
const SCHEDUTIL_PARAMS = {
  MT6989: {
    performance: { up_rate_limit_us: "1000", down_rate_limit_us: "8000" },
    fast: { up_rate_limit_us: "500", down_rate_limit_us: "20000" },
  },
};
const FAS_MODE_OVERRIDES = {
  MT6989: {
    performance: { ceiling: "90", freq_scale: "75" },
    fast: { ceiling: "100", freq_scale: "90" },
  },
};
const INPUT_BOOST_OVERRIDES = {
  MT6989: { fast: { top_app_min: "50", duration_ms: "260" } },
};
const CPUCTL_OVERRIDES = {
  MT6989: { powersave: { background_max: "60" }, balance: { background_max: "72" } },
};
/* GPU devfreq 档位（百分比，0 = 物理端点不限）；仅新适配处理器默认启用。 */
const GPU_DEVFREQ = {
  SM8845: { powersave: { min: 0, max: 55 }, balance: { min: 0, max: 80 },
            performance: { min: 0, max: 100 }, fast: { min: 0, max: 100 } },
  MT6989: { powersave: { min: 0, max: 55 }, balance: { min: 0, max: 80 },
            performance: { min: 0, max: 100 }, fast: { min: 25, max: 100 } },
};

const clone = (value) => JSON.parse(JSON.stringify(value));
const slots = (policies, value) => policies.map((policy) => (policy >= 0 ? value : "0"));

function profile(soc, policies) {
  const minOverrides = MODE_MIN_OVERRIDES[soc] || {};
  const schedutil = SCHEDUTIL_PARAMS[soc] || {};
  const modes = {};
  for (const [name, minimum] of [["powersave", "0%"], ["balance", "0%"], ["performance", "8%"], ["fast", "0%"]]) {
    const minSlots = minOverrides[name] ? minOverrides[name].slice() : slots(policies, minimum);
    const params = schedutil[name]
      ? policies.map((p) => (p >= 0 ? { ...schedutil[name] } : {}))
      : [{}, {}, {}, {}];
    modes[name] = {
      min: minSlots, max: slots(policies, "100%"),
      governors: policies.map((p) => (p >= 0 ? GOVERNORS[soc] : "")),
      params, online: Array(8).fill(true),
    };
  }
  for (const [name, values] of Object.entries({ powersave: 0, balance: 1, performance: 2, fast: 3 })) {
    modes[name].max = CEILINGS[soc][values].map((value, index) => (policies[index] >= 0 ? `${value}%` : "0"));
  }
  const fasModes = {};
  for (const [name, base] of Object.entries(FAS_MODES)) {
    fasModes[name] = { ...((FAS_MODE_OVERRIDES[soc] || {})[name] || base) };
  }
  const cpuctlModes = clone(CPUCTL_OVERRIDES[soc] || { powersave: { background_max: "65" }, balance: { background_max: "75" } });
  const inputModes = {
    powersave: { top_app_min: "15", duration_ms: "100" },
    balance: { top_app_min: "20", duration_ms: "140" },
    performance: { top_app_min: "28", duration_ms: "180" },
    fast: { top_app_min: "40", duration_ms: "220" },
  };
  Object.assign(inputModes, clone(INPUT_BOOST_OVERRIDES[soc] || {}));
  const features = {
    node_watchdog: true,
    cpuset: { enabled: false, top_app: "0-7", foreground: "0-7", restricted: "0-3", system_background: "0-3", background: "0-1" },
    cpuctl: { enabled: true, modes: cpuctlModes },
    input_boost: { enabled: true, top_app_min: "30", duration_ms: "300", modes: inputModes },
    launch_boost: { enabled: false, rate_limit_ms: 500, min: ["0", "0", "0", "0"] },
    disable_gpu_boost: false,
    scheduler: {
      enabled: false, energy_aware: true, schedstats: false,
      latency_ns: "10000000", migration_cost_ns: "500000",
      min_granularity_ns: "1000000", wakeup_granularity_ns: "1000000",
      nr_migrate: "32", util_clamp_min: "0", util_clamp_max: "1024",
    },
    foreground_ignore: [],
    perf_lock: { enabled: false, services: [] },
    fas: { enabled: true, override_scene: true, step_ms: "200", sample_ms: "1000", modes: fasModes },
  };
  if (GPU_DEVFREQ[soc]) features.gpu_devfreq = { enabled: true, modes: clone(GPU_DEVFREQ[soc]) };
  if (soc === "XRING_O3") Object.assign(features.cpuset, { restricted: "4-7", system_background: "4-7", background: "4-5" });
  return {
    schema: "novasched/2", module: "NovaSched_Zen_Edition",
    meta: { name: "NovaSched Zen Edition", author: "ZenJooo", version: 390, soc, loglevel: "INFO" },
    policies,
    features,
    modes,
  };
}

/* Python json.dumps(indent=2, ensure_ascii=False) 的等价输出。 */
function dumps(value, depth = 0) {
  const pad = "  ".repeat(depth);
  const padIn = "  ".repeat(depth + 1);
  if (value === null || typeof value === "boolean") return JSON.stringify(value);
  if (typeof value === "number") return String(value);
  if (typeof value === "string") return JSON.stringify(value);
  if (Array.isArray(value)) {
    if (!value.length) return "[]";
    return "[\n" + value.map((item) => padIn + dumps(item, depth + 1)).join(",\n") + "\n" + pad + "]";
  }
  const keys = Object.keys(value);
  if (!keys.length) return "{}";
  return "{\n" + keys.map((key) => `${padIn}${JSON.stringify(key)}: ${dumps(value[key], depth + 1)}`).join(",\n") + "\n" + pad + "}";
}

let failures = 0;
for (const [soc, policies] of Object.entries(LAYOUTS)) {
  const file = path.join(ROOT, "module-template", "config", `${soc}.json`);
  const expected = dumps(profile(soc, policies)) + "\n";
  const actual = fs.readFileSync(file, "utf8");
  if (actual !== expected) {
    failures++;
    console.error(`MISMATCH ${soc}.json`);
    for (let i = 0; i < Math.max(expected.length, actual.length); i++) {
      if (expected[i] !== actual[i]) {
        console.error(`  first diff at ${i}: expected ${JSON.stringify(expected.slice(i, i + 60))} got ${JSON.stringify(actual.slice(i, i + 60))}`);
        break;
      }
    }
  } else {
    console.log(`OK ${soc}.json`);
  }
}
if (failures) { console.error(`${failures} profile mismatches`); process.exitCode = 1; }
else console.log("12 profiles verified byte-exact");
