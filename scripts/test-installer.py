"""Exercise admission order without touching Android runtime paths."""
import os
import subprocess
import tempfile
import json
from pathlib import Path

project = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix="nova-installer-", dir=os.environ.get("TMPDIR")) as temporary:
    module = Path(temporary) / "module"
    (module / "bin").mkdir(parents=True)
    binary = module / "bin/novasched"
    binary.write_text('#!/bin/sh\ncase "$1" in\nself-test) exit 0;;\nprobe) echo "PROBE_CALLED" >&2; exit "$PROBE_EXIT";;\n*) exit 88;;\nesac\n')
    binary.chmod(0o755)
    harness = '''
ui_print() { echo "$*"; }
abort() { echo "$*"; exit 70; }
set_perm() { :; }
set_perm_recursive() { :; }
mkdir() { echo "STATE_WRITE_REACHED"; exit 77; }
cp() { echo "UNEXPECTED_WRITE"; exit 78; }
getprop() { echo "UNEXPECTED_MODEL_GATE"; exit 79; }
. "$CUSTOMIZE"
'''
    for name, framework, arch, probe_exit, expected, marker in [
        ("KernelSU", {"KSU":"true","KSU_VER_CODE":"1"}, "arm64", "0", 77, "STATE_WRITE_REACHED"),
        ("KernelSU Next", {"KSU":"true","KSU_NEXT":"true"}, "arm64", "0", 77, "STATE_WRITE_REACHED"),
        ("Magisk no numeric gate", {"MAGISK_VER_CODE":"1"}, "arm64", "0", 77, "STATE_WRITE_REACHED"),
        ("Magisk Alpha", {"MAGISK_VER":"alpha","MAGISK_VER_CODE":"999999"}, "arm64", "0", 77, "STATE_WRITE_REACHED"),
        ("APatch", {"APATCH":"true","APATCH_VER_CODE":"1"}, "arm64", "0", 77, "STATE_WRITE_REACHED"),
        ("capable installer without brand flags", {}, "arm64-v8a", "0", 77, "STATE_WRITE_REACHED"),
        ("ARM64 required", {"KSU":"true"}, "x86", "0", 70, "仅支持 arm64-v8a"),
        ("unsupported SoC/capabilities", {"APATCH":"true"}, "arm64", "1", 70, "硬件检测未通过"),
    ]:
        env = {k:v for k,v in os.environ.items() if k not in {"KSU","APATCH","MAGISK_VER","MAGISK_VER_CODE","KSU_NEXT"}}
        env.update(framework)
        env.update(ARCH=arch, PROBE_EXIT=probe_exit,
                   MODPATH=str(module), CUSTOMIZE=str(project / "module-template/customize.sh"))
        run = subprocess.run(["bash", "-c", harness], env=env, capture_output=True, text=True)
        assert run.returncode == expected, (name, run.returncode, run.stdout, run.stderr)
        assert marker in run.stdout, (name, run.stdout)
        assert "UNEXPECTED" not in run.stdout, (name, run.stdout)
        if probe_exit != "0":
            assert "STATE_WRITE_REACHED" not in run.stdout, "admission failure wrote state"
            assert "PROBE_CALLED" in run.stdout, "native stderr must reach installer UI"
        print(f"PASS: {name}")

# Exercise the actual standard Magisk ZIP entry with a supplied installer API.
with tempfile.TemporaryDirectory(prefix="nova-magisk-", dir=os.environ.get("TMPDIR")) as temporary:
    root = Path(temporary)
    utility = root / "util_functions.sh"
    script = root / "update-binary"
    script.write_text((project / "module-template/META-INF/com/google/android/update-binary").read_text()
                      .replace('/data/adb/magisk/util_functions.sh', str(utility)))
    for version, code in [(1,0), (999999,0), (27000,42)]:
        utility.write_text(f'MAGISK_VER_CODE={version}\ninstall_module() {{ printf "installer_called:%s:%s\\n" "$OUTFD" "$ZIPFILE"; return {code}; }}\n')
        run = subprocess.run(['sh',str(script),'3','9','test.zip'],capture_output=True,text=True)
        assert run.returncode == code, (run.stdout,run.stderr)
        assert 'installer_called:9:test.zip' in run.stdout
        print(f'PASS: Magisk bootstrap version={version} propagates installer exit={code}')
    utility.write_text('MAGISK_VER_CODE=999999\n')
    run = subprocess.run(['sh',str(script),'3','9','test.zip'],capture_output=True,text=True)
    assert run.returncode == 1 and '缺少 install_module' in run.stdout
    print('PASS: missing installer API fails with capability reason')

# The thin boot shell must leave a record even when no Rust identity can exist.
with tempfile.TemporaryDirectory(prefix="nova-service-", dir=os.environ.get("TMPDIR")) as temporary:
    root = Path(temporary)
    (root / 'bin').mkdir()
    state = root / 'state'
    service = root / 'service.sh'
    service.write_text((project / 'module-template/service.sh').read_text()
                       .replace('NOVA_STATE=/data/adb/novasched', f'NOVA_STATE="{state}"'))
    getprop = root / 'bin/getprop'
    getprop.write_text('#!/bin/sh\necho 1\n'); getprop.chmod(0o755)
    core = root / 'bin/novasched'
    for exitcode in [0,1,139]:
        core.write_text(f'#!/bin/sh\necho "fake core received:$1"\nexit {exitcode}\n'); core.chmod(0o755)
        run = subprocess.run(['sh',str(service)],env=dict(os.environ, PATH=str(root/'bin')+':'+os.environ['PATH']))
        log = (state/'boot-entry.log').read_text()
        assert run.returncode == exitcode and f'start exited: {exitcode}' in log and 'service.sh entered' in log
        assert 'fake core received:start' in log
        print(f'PASS: service preserves bootstrap output and exit={exitcode}')

assert '/data/adb/ksu' not in (project/'native/src/scheduler.rs').read_text(), 'Rust preflight still gates on KSU directory'

# Isolate the shell's prepare-config delegation; real selection/rollback is
# exercised by native profiles::tests against actual files.
with tempfile.TemporaryDirectory(prefix="nova-upgrade-", dir=os.environ.get("TMPDIR")) as temporary:
    root=Path(temporary); module=root/"module"; (module/"bin").mkdir(parents=True); (module/"config").mkdir()
    names={"SM8450":"SM8450.json","SM8475":"SM8475.json","SM8550":"SM8550.json","SM8650":"SM8650.json","SM8750":"SM8750.json","SM8850":"SM8850.json"}
    for file in names.values(): (module/"config"/file).write_bytes((project/"module-template/config"/file).read_bytes())
    binary=module/"bin/novasched"
    binary.write_text(r'''#!/usr/bin/env python3
import os,sys,shutil
from pathlib import Path
if sys.argv[1]!='prepare-config':sys.exit(0)
if os.environ.get('NOVA_CONFIG_EXIT','0')!='0':sys.exit(1)
state=Path(os.environ['NOVA_TEST_STATE']);module=Path(os.environ['MODPATH'])
names={'SM8450':'SM8450.json','SM8475':'SM8475.json','SM8550':'SM8550.json','SM8650':'SM8650.json','SM8750':'SM8750.json','SM8850':'SM8850.json'}
soc=os.environ.get('NOVA_TEST_SOC','SM8650');src=module/'config'/names[soc]
if not (state/'config.json').exists():shutil.copyfile(src,state/'config.json')
shutil.copyfile(src,state/'config.default.json');(state/'config.profile').write_text(soc+'\n')
''');binary.chmod(0o755)
    customize=root/"customize.sh";original=(project/"module-template/customize.sh").read_text()
    assert 'NOVA_STATE="/data/adb/novasched"' in original
    customize.write_text(original.replace('NOVA_STATE="/data/adb/novasched"','NOVA_STATE="$NOVA_TEST_STATE"'))
    harness='ui_print() { :; }; abort() { echo "$*"; exit 70; }; set_perm() { :; }; set_perm_recursive() { :; }; . "$CUSTOMIZE"'
    default=(project/"module-template/config/SM8650.json").read_bytes();custom=json.loads(default)
    custom['modes']['balance']['max'][0]='1234567';custom['features']['launch_boost']['enabled']=True
    custom=(json.dumps(custom,ensure_ascii=False,indent=3)+'\n').encode()
    def install(state,**extra):
        env=dict(os.environ,KSU="true",ARCH="arm64",MODPATH=str(module),CUSTOMIZE=str(customize),NOVA_TEST_STATE=str(state),**extra)
        return subprocess.run(["bash","-c",harness],env=env,capture_output=True,text=True)
    for name,old,expected in [
        ("new install",None,{"extreme_powersave":"0","smooth_powersave":"0"}),
        ("legacy preferences","extreme_powersave=0\n",{"extreme_powersave":"0","smooth_powersave":"0"}),
        ("legacy extreme enabled","extreme_powersave=1\n",{"extreme_powersave":"1","smooth_powersave":"0"}),
        ("existing both disabled","extreme_powersave=0\nsmooth_powersave=0\n",{"extreme_powersave":"0","smooth_powersave":"0"}),
        ("missing extreme preference","smooth_powersave=1\n",{"extreme_powersave":"0","smooth_powersave":"1"}),
        ("existing smooth preference","extreme_powersave=1\nsmooth_powersave=1\n",{"extreme_powersave":"1","smooth_powersave":"1"}),
    ]:
        state=root/name.replace(' ','-');state.mkdir()
        if old is not None:(state/'options.txt').write_text(old);(state/'config.json').write_bytes(custom)
        run=install(state);assert run.returncode==0,(name,run.stdout,run.stderr)
        options=dict(line.split('=',1) for line in (state/'options.txt').read_text().splitlines() if line and not line.startswith('#'))
        assert options==expected,(name,options)
        assert (state/'config.json').read_bytes()==(custom if old is not None else default)
        assert (state/'config.default.json').read_bytes()==default
        print(f'PASS: {name}')
    for name,code in [("valid custom config and existing backup",0),("invalid custom config preserved on abort",1)]:
        state=root/name.replace(' ','-');state.mkdir();previous=custom if code==0 else b'invalid user config\n'
        (state/'config.json').write_bytes(previous);(state/'config.json.bak').write_bytes(b'older backup\n')
        run=install(state,NOVA_CONFIG_EXIT=str(code));assert run.returncode==(0 if code==0 else 70),(name,run.stdout,run.stderr)
        assert (state/'config.json').read_bytes()==previous;assert (state/'config.json.bak').read_bytes()==b'older backup\n'
        if code:assert not (state/'options.txt').exists()
        print(f'PASS: {name}')
    for soc,file in names.items():
        state=root/soc;state.mkdir();run=install(state,NOVA_TEST_SOC=soc)
        assert run.returncode==0,(soc,run.stdout,run.stderr)
        assert (state/'config.json').read_bytes()==(project/'module-template/config'/file).read_bytes()
        assert (state/'config.profile').read_text()==soc+'\n'
        print(f'PASS: shell delegates automatic profile initialization {soc}')
