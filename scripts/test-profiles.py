"""ZenJooo: check power-policy bounds in every shipped processor profile.

This validates configuration, not phone power consumption or frame pacing.
"""
from pathlib import Path
import json
import re

root = Path(__file__).resolve().parents[1]
modes = ("powersave", "balance", "performance", "fast")
# 天玑 9300+（MT6989）采用用户确认的极限性能策略：性能上限优先、不做省电
# 甜点。极速档底频（≤35%）、输入脉冲（50/260ms）与满档上限是刻意标定，
# 不适用于其它模板的功耗护栏；仍要求不永久锁死、单调不降、热保护在线。
EXTREME_PERFORMANCE = {"MT6989"}
generation = int(re.search(r'PROFILE_VERSION: i64 = (\d+)', (root/'native/src/config.rs').read_text()).group(1))
count = 0
for path in sorted((root / "module-template/config").glob("*.json")):
    config = json.loads(path.read_text())
    extreme = path.stem in EXTREME_PERFORMANCE
    assert config["meta"]["author"] == "ZenJooo"
    assert config["meta"]["version"] == generation
    assert set(config["modes"]) == set(modes)
    for slot, policy in enumerate(config["policies"]):
        limits = []
        for mode in modes:
            profile = config["modes"][mode]
            assert all(profile["online"]), "Default tuning must not offline OEM cores"
            minimum = profile["min"][slot]
            maximum = profile["max"][slot]
            if policy < 0:
                assert minimum == maximum == "0"
                assert profile["governors"][slot] == ""
                assert profile["params"][slot] == {}
                continue
            assert minimum.endswith("%") and maximum.endswith("%")
            floor, ceiling = int(minimum[:-1]), int(maximum[:-1])
            assert 0 <= floor <= ceiling <= 100
            if extreme and mode == "fast":
                assert floor <= 35, "MT6989 fast floor must stay well below a permanent lock"
            else:
                assert floor <= 10, "A permanent high floor wastes idle power"
            limits.append(ceiling)
            assert profile["governors"][slot] == ("walt" if path.stem.startswith("SM") else "schedutil")
        if policy >= 0:
            if extreme:
                assert all(a <= b for a, b in zip(limits, limits[1:])), limits
            else:
                assert all(a < b for a, b in zip(limits, limits[1:])), limits
            assert 90 <= limits[-1] <= 100
    assert max(int(value[:-1]) for value, policy in zip(config["modes"]["fast"]["max"], config["policies"]) if policy >= 0) == 100
    features = config["features"]
    assert not features["disable_gpu_boost"]
    assert not features["perf_lock"]["enabled"] and not features["perf_lock"]["services"]
    assert not features["scheduler"]["enabled"] and not features["cpuset"]["enabled"]
    assert features["node_watchdog"]
    pulses = features["input_boost"]["modes"]
    assert set(pulses) == set(modes)
    floors = [int(pulses[mode]["top_app_min"]) for mode in modes]
    durations = [int(pulses[mode]["duration_ms"]) for mode in modes]
    assert floors == sorted(floors) and 0 < floors[0] < floors[-1] <= (50 if extreme else 40)
    assert durations == sorted(durations) and 50 <= min(durations) <= max(durations) <= (260 if extreme else 250)
    assert set(features["cpuctl"]["modes"]) == {"powersave", "balance"}
    assert 50 <= int(features["cpuctl"]["modes"]["powersave"]["background_max"]) < int(features["cpuctl"]["modes"]["balance"]["background_max"]) < 100
    gpu = features.get("gpu_devfreq")
    if gpu is not None:
        assert gpu["enabled"] is True and set(gpu["modes"]) == set(modes)
        for mode in modes:
            lo, hi = gpu["modes"][mode]["min"], gpu["modes"][mode]["max"]
            assert 0 <= lo <= hi <= 100, (path.stem, mode, lo, hi)
    count += 4
    print("PASS", path.stem, "four ordered profiles, bounded pulses, OEM safety controls")
print(f"PASS: {count} processor/mode configurations. No device energy measurements were made.")
