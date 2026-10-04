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
    return {
        'schema': 'novasched/2', 'module': 'NovaSched_Zen_Edition',
        'meta': {'name': 'NovaSched Zen Edition', 'author': 'ZenJooo', 'version': 218,
                 'soc': soc, 'loglevel': 'INFO'},
        'policies': policies,
        'features': {
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
            'extreme': {'enabled': False, 'max': slots('55%'), 'uclamp_max': '1024'},
            'smooth': {'enabled': False, 'max': slots('70%'), 'uclamp_max': '1024',
                       'uclamp_min_limit': '1024', 'up_rate_limit_us': '0', 'restore_stock_response': True},
            'perf_lock': {'enabled': False, 'services': []},
        },
        'modes': modes,
    }

def generate(check=False):
    for soc, policies in LAYOUTS.items():
        path = ROOT / 'module-template/config' / (soc + '.json')
        text = json.dumps(profile(soc, policies), ensure_ascii=False, indent=2) + '\n'
        if check:
            assert path.read_text() == text, f'{path.name} differs from generated defaults'
        else:
            path.write_text(text)
    print('Six NovaSched profiles verified' if check else 'Six NovaSched profiles generated')

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--check', action='store_true')
    generate(parser.parse_args().check)
