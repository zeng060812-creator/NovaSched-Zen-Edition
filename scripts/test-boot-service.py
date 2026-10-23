"""Verify the boot shell waits for unlock and cancels on manager disable."""
import os
import subprocess
import tempfile
from pathlib import Path

project = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix="nova-boot-", dir=os.environ.get("TMPDIR")) as folder:
    root = Path(folder)
    module = root / "module"
    (module / "bin").mkdir(parents=True)
    service = root / "service.sh"
    service.write_text((project / "module-template/service.sh").read_text().replace(
        "MODDIR=${0%/*}", 'MODDIR="$NOVA_TEST_MODULE"').replace("NOVA_STATE=/data/adb/novasched", 'NOVA_STATE="$NOVA_TEST_STATE"'))
    (root/"state/logs").mkdir(parents=True)
    core = module / "bin/novasched"
    core.write_text('''#!/usr/bin/env python3
import os, sys
from pathlib import Path
module = Path(os.environ['NOVA_TEST_MODULE'])
path = module / 'calls'
path.open('a').write(sys.argv[1] + '\\n')
if os.environ.get('NOVA_BROKEN'): sys.exit(127)
if sys.argv[1] == 'boot-ready' and os.environ.get('NOVA_USER_FAILURE'):
    print('novasched: current-user query failed: Binder stdout denied')
    sys.exit(1)
if sys.argv[1] == 'boot-ready':
    count = path.read_text().splitlines().count('boot-ready')
    sys.exit(1 if count < 3 or os.environ.get('NOVA_LOCKED') else 0)
if sys.argv[1] == 'start': (module / 'started').write_text('yes')
''')
    core.chmod(0o755)
    harness = '''
getprop() { echo 1; }
sleep() {
  NOVA_SLEEP_COUNT=$((NOVA_SLEEP_COUNT + 1))
  if [ -n "$NOVA_DISABLE_AFTER" ] && [ "$NOVA_SLEEP_COUNT" -ge "$NOVA_DISABLE_AFTER" ]; then
    touch "$NOVA_TEST_MODULE/disable"
  fi
}
. "$NOVA_SERVICE"
'''

    def run(**options):
        for name in ["calls", "started", "disable", "boot-entry.log"]:
            (module / name).unlink(missing_ok=True)
        env = dict(os.environ, NOVA_TEST_MODULE=str(module), NOVA_SERVICE=str(service), NOVA_SLEEP_COUNT="0", NOVA_TEST_STATE=str(root/"state"))
        env.update(options)
        # Sandboxed Windows hosts can take seconds per spawned process and the
        # happy path alone needs ~19 stand-in invocations; 10 s only suffices
        # on Unix. The logic under test is unchanged.
        return subprocess.run(["sh", "-c", harness], env=env, capture_output=True, text=True, timeout=600)

    result = run()
    assert result.returncode == 0, result.stderr
    calls = (module / "calls").read_text().splitlines()
    assert calls[-1] == "start" and calls.count("start") == 1
    # 2 次解锁前失败 + 1 次门控通过 + 8 次连续 settle 探测，随后 start。
    assert calls.count("boot-ready") >= 11, calls
    assert (module / "started").exists()
    print("PASS: first unlock gate and bounded settle precede native startup")
    for options in [{"NOVA_LOCKED": "1", "NOVA_DISABLE_AFTER": "3"}, {"NOVA_DISABLE_AFTER": "6"}]:
        result = run(**options)
        assert result.returncode == 0, result.stderr
        assert not (module / "started").exists()
        assert "start" not in (module / "calls").read_text().splitlines()
    print("PASS: disabling during unlock or settle cancels all startup writes")
    import shutil
    shutil.rmtree(root/'state')
    result=run(NOVA_DISABLE_AFTER='1')
    assert result.returncode==0 and not (root/'state').exists()
    (root/'state/logs').mkdir(parents=True)
    print('PASS: late boot does not recreate removed data directories')
    result = run(NOVA_BROKEN="1")
    assert result.returncode == 1
    # 退出码 127（ELF 无法执行）立即放弃，不再无限等待解锁，也不再有
    # 独立的 version 探针（boot-ready 失败本身已证明二进制不可用）。
    assert (module / 'calls').read_text().splitlines() == ['boot-ready']
    assert not (module / 'started').exists()
    print("PASS: a broken executable exits immediately instead of waiting for unlock")
    result = run(NOVA_USER_FAILURE="1", NOVA_DISABLE_AFTER="12")
    assert result.returncode == 0
    assert not (module / 'started').exists()
    entry = (root / 'state/logs/boot-entry.log').read_text()
    assert entry.count('startup-gate:') == 2, entry
    assert 'Binder stdout denied' in entry
    print("PASS: a failed user query remains blocked and its actual cause is logged without flooding")
