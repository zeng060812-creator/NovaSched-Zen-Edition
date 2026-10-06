"""Generate NovaSched defaults from one policy definition per supported SoC."""
from pathlib import Path
import argparse
import json

ROOT = Path(__file__).resolve().parents[1]
LAYOUTS = {
    'SM8450': [0, 4, 7, -1], 'SM8475': [0, 4, 7, -1],
    'SM8550': [0, 3, 7, -1], 'SM8650': [0, 2, 5, 7],
    'SM8750': [0, 6, -1, -1], 'SM8850': [0, 6, -1, -1],
}
# Per-SoC mode floors over the shared defaults. Slot order follows LAYOUTS
# (c0..c3 = the listed policy anchors, little/efficiency first, prime last);
# percentages resolve against each cluster's physical max on the device.
# Powersave/balance stay shared everywhere: percent ceilings plus per-device
# frequency tables already adapt them, and daily battery profile is sacred.
MODE_OVERRIDES = {
    # 8 Gen 1 / 8+ Gen 1: c0=4xA510 little, c1=3xA710 big, c2=X1/X2 prime.
    # A710 runs hot; floors stay moderate so fast mode helps frametimes
    # without cooking the older silicon.
    'SM8450': {
        'performance': {'min': ['20%', '25%', '25%', '0%']},
        'fast': {'min': ['30%', '40%', '45%', '0%']},
    },
    'SM8475': {
        'performance': {'min': ['20%', '25%', '25%', '0%']},
        'fast': {'min': ['30%', '40%', '45%', '0%']},
    },
    # 8 Gen 2: c0=3xA520 little, c1=4xA715/A710 big, c2=X3 prime.
    'SM8550': {
        'performance': {'min': ['20%', '25%', '25%', '0%']},
        'fast': {'min': ['35%', '45%', '50%', '0%']},
    },
    # 8 Gen 3: c0=2xA520@2.27 little, c1=3xA720@3.2 big, c2=2xA720@3.0 mid,
    # c3=X4@3.3 prime. Powersave caps target a ~1.8W daily profile: prime
    # keeps a 1.92GHz burst ceiling so launches stay snappy, mid/big trim the
    # heavy-worker power, little stays cheap but instant. Fast floors hold
    # game threads (X4 ~1.8GHz); powersave min stays 0% for deep idle.
    # hispeed_freq (percent of each cluster max, resolved at runtime) makes
    # walt JUMP above the floor on load spikes instead of ramping — the
    # system-scheduler trick for steady frametimes at low idle power. Nodes
    # missing on some kernels are skipped silently.
    'SM8650': {
        'powersave': {'max': ['62%', '55%', '52%', '58%']},
        'performance': {'min': ['15%', '20%', '25%', '25%']},
        'fast': {
            'min': ['30%', '40%', '50%', '55%'],
            'params': [
                {'hispeed_freq': '30%'},
                {'hispeed_freq': '50%'},
                {'hispeed_freq': '55%'},
                {'hispeed_freq': '65%'},
            ],
        },
    },
    # 8 Elite / 8 Elite Gen 5: c0=6x Oryon efficiency, c1=Oryon prime.
    # Two wide dynamic clocks; 45% keeps the ~4.3GHz prime near 1.9GHz.
    'SM8750': {
        'performance': {'min': ['15%', '20%', '0%', '0%']},
        'fast': {'min': ['35%', '45%', '0%', '0%']},
    },
    'SM8850': {
        'performance': {'min': ['15%', '20%', '0%', '0%']},
        'fast': {'min': ['35%', '45%', '0%', '0%']},
    },
}

def profile(soc, policies):
    def slots(value):
        return [value if policy >= 0 else '0' for policy in policies]
    modes = {}
    for name, minimum, maximum in [('powersave', '0%', '70%'), ('balance', '0%', '100%'),
                                   ('performance', '15%', '100%'), ('fast', '25%', '100%')]:
        modes[name] = {
            'min': slots(minimum), 'max': slots(maximum),
            'governors': ['auto' if p >= 0 else '' for p in policies],
            'params': [{}, {}, {}, {}], 'online': [True] * 8,
        }
    for name, fields in MODE_OVERRIDES.get(soc, {}).items():
        for key, values in fields.items():
            empty = {} if key == 'params' else '0'
            modes[name][key] = [values[i] if p >= 0 else empty for i, p in enumerate(policies)]
    features = {
        'node_watchdog': True,
        'cpuset': {'enabled': False, 'top_app': '0-7', 'foreground': '0-7',
                   'restricted': '0-3', 'system_background': '0-3', 'background': '0-1'},
        'launch_boost': {'enabled': False, 'rate_limit_ms': 500, 'min': ['0'] * 4},
        'disable_gpu_boost': False,
        'scheduler': {'enabled': False, 'energy_aware': True, 'schedstats': False,
                      'latency_ns': '10000000', 'migration_cost_ns': '500000',
                      'min_granularity_ns': '1000000', 'wakeup_granularity_ns': '1000000',
                      'nr_migrate': '32', 'util_clamp_min': '0', 'util_clamp_max': '1024'},
        'foreground_ignore': [],
        'extreme': {'enabled': False, 'max': slots('50%'), 'uclamp_max': '1024'},
        'smooth': {'enabled': False, 'max': slots('70%'), 'uclamp_max': '1024',
                   'uclamp_min_limit': '1024', 'up_rate_limit_us': '0', 'restore_stock_response': True},
        'perf_lock': {'enabled': False, 'services': []},
    }
    if soc == 'SM8650':
        # 8 Gen 3 verified layout (little-first): little cpu0-1, big cpu2-4,
        # mid cpu5-6, prime cpu7. cpuset sheds little cores from foreground
        # in fast mode and pins background there; cpuctl clamps give the
        # foreground a util floor in fast mode and cap background util in
        # powersave (cgroup-v2 nodes, skipped where unsupported).
        features['cpuset'] = {
            'enabled': True, 'top_app': '0-7', 'foreground': '0-7',
            'restricted': '0-1', 'system_background': '0-1', 'background': '0-1',
            'modes': {'fast': {'top_app': '2-7', 'foreground': '2-7'}},
        }
        features['cpuctl'] = {
            'enabled': True,
            'modes': {
                'fast': {'top_app_min': '30'},
                'powersave': {'background_max': '60'},
            },
        }
    return {
        'schema': 'novasched/2', 'module': 'NovaSched_Zen_Edition',
        'meta': {'name': 'NovaSched Zen Edition', 'author': 'ZenJooo', 'version': 230,
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
    print('Six NovaSched profiles verified' if check else 'Six NovaSched profiles generated')

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--check', action='store_true')
    generate(parser.parse_args().check)
