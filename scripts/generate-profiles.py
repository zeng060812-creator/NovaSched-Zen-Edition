"""Generate NovaSched defaults from one policy definition per supported SoC.

v2.7.0 调优标准（功耗优先）：
- 省电档：极限压低各簇上限，只服务信息流与日用，输入靠短脉冲保跟手；
- 均衡档：日用甜点，给剪辑 / AI 生产留余量但不拉大频率；
- 性能档：按王者荣耀稳定 120 帧的每芯片甜点频率，只留 8% 底频；
- 极速档：开放中/大核全频段给 3D 游戏，小核封顶 90-94%（封顶收益低），
  动态部分交给 NovaFAS（features.fas），不再使用 hispeed_freq 强推。
"""
from pathlib import Path
import argparse
import json

ROOT = Path(__file__).resolve().parents[1]
LAYOUTS = {
    'SM8450': [0, 4, 7, -1], 'SM8475': [0, 4, 7, -1],
    'SM8550': [0, 3, 7, -1], 'SM8650': [0, 2, 5, 7],
    'SM8750': [0, 6, -1, -1], 'SM8850': [0, 6, -1, -1],
    'SM8845': [0, 6, -1, -1],
    'DIMENSITY8350': [0, 4, 7, -1], 'DIMENSITY8400': [0, 4, 7, -1],
    'DIMENSITY9400PLUS': [0, 4, 7, -1],
    'MT6989': [0, 4, 7, -1],
    'XRING_O3': [4, 0, 1, -1],
}
# Cluster order follows LAYOUTS. Ceilings are percentages of the detected
# physical range and snap to the device OPP table. These are tuning defaults,
# not measured energy-model sweet spots. No template disables system thermal.
#          (powersave, balance, performance, fast) per cluster, percent of max
CEILINGS = {
    'SM8450':            ([54, 46, 44, 0],  [60, 54, 52, 0],  [84, 80, 80, 0],  [92, 98, 100, 0]),
    'SM8475':            ([54, 46, 44, 0],  [60, 54, 52, 0],  [84, 80, 80, 0],  [92, 98, 100, 0]),
    'SM8550':            ([54, 46, 46, 0],  [60, 54, 54, 0],  [84, 82, 82, 0],  [92, 98, 100, 0]),
    'SM8650':            ([60, 45, 49, 45], [70, 58, 56, 59], [85, 84, 85, 82], [95, 100, 100, 100]),
    'SM8750':            ([52, 46, 0, 0],   [58, 50, 0, 0],   [80, 72, 0, 0],   [90, 100, 0, 0]),
    'SM8850':            ([52, 46, 0, 0],   [58, 50, 0, 0],   [80, 70, 0, 0],   [90, 100, 0, 0]),
    # SM8845（骁龙 8 Gen 5，一加 PLR110 真机：6+2 Oryon，policy0=cpu0-5 /
    # policy6=cpu6-7）：两簇差异比 SM8850 更收敛，性能档大簇放宽到 74%。
    'SM8845':            ([52, 45, 0, 0],   [60, 52, 0, 0],   [82, 74, 0, 0],   [92, 100, 0, 0]),
    'DIMENSITY8350':     ([56, 48, 46, 0],  [62, 56, 54, 0],  [86, 84, 84, 0],  [94, 100, 100, 0]),
    'DIMENSITY8400':     ([56, 48, 46, 0],  [62, 56, 54, 0],  [86, 84, 84, 0],  [94, 100, 100, 0]),
    'DIMENSITY9400PLUS': ([52, 46, 44, 0],  [58, 52, 50, 0],  [80, 78, 80, 0],  [90, 100, 100, 0]),
    # MT6989（天玑 9300+，vivo V2417A 真机：4×A720 + 3×X4 + 1×X4）单独采用
    # 极限性能取向：性能/极速档不做省电甜点，三簇上限接近或等于全频段；
    # 不永久锁最高频（min 仅部分抬起），热保护与回落仍交给内核和 NovaFAS。
    'MT6989':            ([60, 42, 38, 0],  [70, 60, 55, 0],  [90, 92, 100, 0], [100, 100, 100, 0]),
    'XRING_O3':          ([54, 46, 48, 0],  [60, 52, 54, 0],  [82, 74, 80, 0],  [92, 100, 100, 0]),
}
# NovaFAS per-mode limits: ceiling = top-app uclamp.min max percent;
# freq_scale = per-cluster minimum-frequency max, percent of the cluster's
# current ceiling (little cluster ×0.6 inside the engine).
FAS_MODES = {
    'powersave':   {'ceiling': '28', 'freq_scale': '25'},
    'balance':     {'ceiling': '45', 'freq_scale': '45'},
    'performance': {'ceiling': '80', 'freq_scale': '60'},
    'fast':        {'ceiling': '95', 'freq_scale': '80'},
}
# 每颗处理器显式写入最优调速器（不再是 auto）：
# 骁龙 = walt（WALT 窗口负载跟踪，小米 14 真机确认 OEM 默认即 walt）；
# 天玑 / 玄戒 = schedutil（MTK 系唯一现代全簇调速器）。
# 内核缺失时运行时自动回退 OEM 调速器（hardware::select_governor）。
GOVERNORS = {
    'SM8450': 'walt', 'SM8475': 'walt', 'SM8550': 'walt',
    'SM8650': 'walt', 'SM8750': 'walt', 'SM8850': 'walt', 'SM8845': 'walt',
    'DIMENSITY8350': 'schedutil', 'DIMENSITY8400': 'schedutil',
    'DIMENSITY9400PLUS': 'schedutil', 'MT6989': 'schedutil',
    'XRING_O3': 'schedutil',
}

# ---- 天玑 9300+（MT6989）极限性能策略覆写 -------------------------------
# 性能上限优先：更高的极速档底频、更快的 schedutil 升频/更慢的降频、
# FAS 上限放开到 100%、更强的输入突频。均不永久锁最高频，不做省电甜点。
MODE_MIN_OVERRIDES = {
    'MT6989': {'fast': ['25%', '30%', '35%', '0']},
}
# schedutil 速率限制（微秒）：升频快、降频慢，减少帧间隔内的频率迟疑。
SCHEDUTIL_PARAMS = {
    'MT6989': {
        'performance': {'up_rate_limit_us': '1000', 'down_rate_limit_us': '8000'},
        'fast': {'up_rate_limit_us': '500', 'down_rate_limit_us': '20000'},
    },
}
FAS_MODE_OVERRIDES = {
    'MT6989': {
        'performance': {'ceiling': '90', 'freq_scale': '75'},
        'fast': {'ceiling': '100', 'freq_scale': '90'},
    },
}
INPUT_BOOST_OVERRIDES = {
    'MT6989': {'fast': {'top_app_min': '50', 'duration_ms': '260'}},
}
CPUCTL_OVERRIDES = {
    'MT6989': {'powersave': {'background_max': '60'},
               'balance': {'background_max': '72'}},
}
# GPU devfreq 档位（百分比，0 = 物理端点不限）。按运行时检测到的 GPU OPP
# 表 snap；节点缺失时守护仅告警降级，不影响 CPU 调度。仅新适配的处理器
# 默认启用（SM8845 = kgsl-3d0，MT6989 = mali，均已真机确认节点存在）。
GPU_DEVFREQ = {
    'SM8845': {'powersave': {'min': 0, 'max': 55}, 'balance': {'min': 0, 'max': 80},
               'performance': {'min': 0, 'max': 100}, 'fast': {'min': 0, 'max': 100}},
    'MT6989': {'powersave': {'min': 0, 'max': 55}, 'balance': {'min': 0, 'max': 80},
               'performance': {'min': 0, 'max': 100}, 'fast': {'min': 25, 'max': 100}},
}

def profile(soc, policies):
    def slots(value):
        return [value if policy >= 0 else '0' for policy in policies]
    min_overrides = MODE_MIN_OVERRIDES.get(soc, {})
    schedutil = SCHEDUTIL_PARAMS.get(soc, {})
    modes = {}
    for name, minimum in [('powersave', '0%'), ('balance', '0%'),
                          ('performance', '8%'), ('fast', '0%')]:
        min_slots = list(min_overrides[name]) if name in min_overrides else slots(minimum)
        if name in schedutil:
            params = [dict(schedutil[name]) if p >= 0 else {} for p in policies]
        else:
            params = [{}, {}, {}, {}]
        modes[name] = {
            'min': min_slots, 'max': slots('100%'),
            'governors': [GOVERNORS[soc] if p >= 0 else '' for p in policies],
            'params': params, 'online': [True] * (10 if soc == 'XRING_O3' else 8),
        }
    for name, values in zip(('powersave', 'balance', 'performance', 'fast'), CEILINGS[soc]):
        modes[name]['max'] = [f'{value}%' if p >= 0 else '0' for value, p in zip(values, policies)]
    fas_modes = {name: dict(FAS_MODE_OVERRIDES.get(soc, {}).get(name, base))
                 for name, base in FAS_MODES.items()}
    cpuctl_modes = {name: dict(values) for name, values in CPUCTL_OVERRIDES.get(
        soc, {'powersave': {'background_max': '65'}, 'balance': {'background_max': '75'}}).items()}
    input_modes = {
        'powersave': {'top_app_min': '15', 'duration_ms': '100'},
        'balance': {'top_app_min': '20', 'duration_ms': '140'},
        'performance': {'top_app_min': '28', 'duration_ms': '180'},
        'fast': {'top_app_min': '40', 'duration_ms': '220'},
    }
    input_modes.update({name: dict(values)
                        for name, values in INPUT_BOOST_OVERRIDES.get(soc, {}).items()})
    features = {
        'node_watchdog': True,
        'cpuset': {'enabled': False, 'top_app': '0-7', 'foreground': '0-7',
                   'restricted': '0-3', 'system_background': '0-3', 'background': '0-1'},
        'cpuctl': {'enabled': True, 'modes': cpuctl_modes},
        'input_boost': {'enabled': True, 'top_app_min': '30', 'duration_ms': '300', 'modes': input_modes},
        'launch_boost': {'enabled': False, 'rate_limit_ms': 500, 'min': ['0'] * 4},
        'disable_gpu_boost': False,
        'scheduler': {'enabled': False, 'energy_aware': True, 'schedstats': False,
                      'latency_ns': '10000000', 'migration_cost_ns': '500000',
                      'min_granularity_ns': '1000000', 'wakeup_granularity_ns': '1000000',
                      'nr_migrate': '32', 'util_clamp_min': '0', 'util_clamp_max': '1024'},
        'foreground_ignore': [],
        'perf_lock': {'enabled': False, 'services': []},
        'fas': {
            'enabled': True, 'override_scene': True,
            'step_ms': '200', 'sample_ms': '1000',
            'modes': fas_modes,
        },
    }
    if soc in GPU_DEVFREQ:
        features['gpu_devfreq'] = {
            'enabled': True,
            'modes': {name: dict(values) for name, values in GPU_DEVFREQ[soc].items()},
        }
    if soc == 'XRING_O3':
        features['cpuset'].update(top_app='0-9', foreground='0-9',
                                 restricted='4-7', system_background='4-7', background='4-5')
    return {
        'schema': 'novasched/2', 'module': 'NovaSched_Zen_Edition',
        'meta': {'name': 'NovaSched Zen Edition', 'author': 'ZenJooo', 'version': 391,
                 'soc': soc, 'loglevel': 'INFO'},
        'policies': policies,
        'features': features,
        'modes': modes,
    }

def generate(check=False):
    for soc, policies in LAYOUTS.items():
        path = ROOT / 'module-template/config' / (soc + '.json')
        text = json.dumps(profile(soc, policies), ensure_ascii=False, indent=2) + '\n'
        if check:
            assert path.read_bytes() == text.encode('utf-8'), f'{path.name} differs from generated defaults'
        else:
            # Write bytes so Windows hosts keep the LF endings Android expects.
            path.write_bytes(text.encode('utf-8'))
    print(f'{len(LAYOUTS)} NovaSched profiles ' + ('verified' if check else 'generated'))

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--check', action='store_true')
    generate(parser.parse_args().check)
