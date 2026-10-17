"""Exercise the installer shell without Android writes or a root framework."""
import json
import os
import subprocess
import tempfile
from pathlib import Path

project = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix='nova-installer-', dir=os.environ.get('TMPDIR')) as folder:
    root = Path(folder)
    module = root / 'module'
    (module / 'bin').mkdir(parents=True)
    props = (project/'module-template/module.prop').read_text()
    version = dict(line.split('=', 1) for line in props.splitlines() if '=' in line)['version']
    (module / 'module.prop').write_text(props)
    state = root / 'state'
    script = root / 'customize.sh'
    script.write_text((project/'module-template/customize.sh').read_text().replace(
        'NOVA_STATE="/data/adb/novasched"', 'NOVA_STATE="$NOVA_TEST_STATE"'))
    core = module / 'bin/novasched'
    core.write_text('''#!/usr/bin/env python3
import os,sys
from pathlib import Path
phase=sys.argv[2]; module=Path(sys.argv[4]); state=Path(os.environ['NOVA_TEST_STATE'])
(module/'calls').open('a').write(phase+'\\n')
if os.environ.get('NOVA_NO_RECEIPT')==phase:sys.exit(139)
code=int(os.environ.get('NOVA_FAIL_CODE','1')) if os.environ.get('NOVA_FAIL')==phase else 0
(state/f'run/install/{phase}.exit').write_text(str(code)+'\\n')
output='storage_deferred='+os.environ.get('NOVA_DEFERRED','false')+'\\n' if phase=='prepare-storage' else 'scene_presence='+os.environ.get('NOVA_SCENE','available')+'\\n' if phase=='scene-status' else 'automatic_enabled=true\\n' if phase=='prepare-auto' else 'compatibility='+os.environ.get('NOVA_COMPAT','standard')+'\\n' if phase=='probe' else phase+'=ok\\n'
(state/f'run/install/{phase}.log').write_text(output)
(state/'logs/install.log').open('a').write(phase+': exit='+str(code)+'\\n')
print(output,end='');sys.exit(code)
''')
    core.chmod(0o755)
    harness = '''
ui_print() { echo "$*"; }
abort() { echo "$*"; exit 70; }
set_perm() { :; }
set_perm_recursive() { :; }
getprop() { case "$1" in ro.product.cpu.abi) echo arm64-v8a ;; *) echo 'Fixture phone' ;; esac; }
. "$CUSTOMIZE"
'''
    def install(**extra):
        if (module/'calls').exists(): (module/'calls').unlink()
        env = {key:value for key,value in os.environ.items() if key not in {
            'KSU','KSU_NEXT','APATCH','MAGISK_VER','MAGISK_VER_CODE','NOVA_FAIL','NOVA_NO_RECEIPT'}}
        env.update(ARCH='arm64', MODPATH=str(module), CUSTOMIZE=str(script), NOVA_TEST_STATE=str(state), NOVA_ANIM_DELAY='0')
        env.update(extra)
        return subprocess.run(['sh','-c',harness],env=env,capture_output=True,text=True)
    for name, flags in [
        ('Magisk',{'MAGISK_VER_CODE':'1'}), ('Magisk Alpha',{'MAGISK_VER':'alpha'}),
        ('KernelSU',{'KSU':'true'}), ('KernelSU Next',{'KSU':'true','KSU_NEXT':'true'}),
        ('APatch',{'APATCH':'true'}), ('other capability-compatible forks',{}),
        ('SukiSU',{'KSU':'true','SUKISU':'true'}), ('KernelSU-compatible fork',{'KSU':'true','KSU_VER_CODE':'1'}),
    ]:
        run=install(**flags)
        assert run.returncode==0,(name,run.stdout,run.stderr)
        assert (module/'calls').read_text().splitlines()==['self-test','probe','prepare-storage','prepare-config','prepare-auto','scene-status','scene-install']
        assert f'NovaSched Zen Edition {version}' in run.stdout
        assert f'NovaSched {version} · ZenJooo' in run.stdout
        assert 'v2.6.1' not in run.stdout
        assert '已确认安装 Scene' in run.stdout
        print('PASS:',name)
    next_version = 'v9.8.7'
    (module/'module.prop').write_text(props.replace(f'version={version}', f'version={next_version}'))
    run=install()
    assert run.returncode==0
    assert f'NovaSched Zen Edition {next_version}' in run.stdout
    assert f'NovaSched {next_version} · ZenJooo' in run.stdout
    print('PASS: both installation banners follow module.prop')
    (module/'module.prop').unlink()
    run=install()
    assert run.returncode==70 and '无法读取模块版本' in run.stdout
    assert not (module/'calls').exists()
    print('PASS: missing module version stops before core execution')
    (module/'module.prop').write_text(props)
    for phase in ['self-test','probe','prepare-storage','prepare-config','prepare-auto']:
        run=install(NOVA_FAIL=phase)
        assert run.returncode==70,(phase,run.stdout)
        calls=(module/'calls').read_text().splitlines()
        assert calls[-1]==phase
        assert (state/'logs/install.log').read_text().endswith(phase+': exit=1\n')
        assert '失败记录已保留' in run.stdout
        print('PASS: failed',phase,'stops later stages')
    run=install(NOVA_NO_RECEIPT='self-test')
    assert run.returncode==70 and '未留下结束记录' in run.stdout
    print('PASS: missing native receipt cannot count as success')
    run=install(NOVA_FAIL='probe',NOVA_FAIL_CODE='124')
    assert run.returncode==70 and '超时' in run.stdout
    print('PASS: timeout receipt reported')
    run=install(ARCH='x86')
    assert run.returncode==70 and not (module/'calls').exists()
    print('PASS: unsupported architecture rejected before core execution')
    run=install(ARCH='')
    assert run.returncode==0
    print('PASS: missing manager ABI falls back to Android arm64 property')
    run=install(NOVA_COMPAT='samsung')
    assert run.returncode==0 and '三星保守策略' in run.stdout
    print('PASS: Samsung compatibility is reported by the hardware check')
    for presence, message in [('available','已确认安装 Scene'),('absent','未安装 Scene'),('unknown','检测暂不可用')]:
        run=install(NOVA_SCENE=presence)
        assert run.returncode==0 and message in run.stdout
        print('PASS: installer reports Scene',presence)
    run=install(NOVA_FAIL='scene-status')
    assert run.returncode==0 and '检测暂不可用' in run.stdout
    print('PASS: failed Scene query stays unknown and installation completes')
    (state/'run/mode.txt').write_text('performance\n')
    run=install()
    assert run.returncode==0 and (state/'run/mode.txt').read_text()=='performance\n'
    print('PASS: reinstall preserves selected mode')

    run=install(NOVA_DEFERRED='true')
    assert run.returncode==0,(run.stdout,run.stderr)
    assert (module/'calls').read_text().splitlines()==['self-test','probe','prepare-storage']
    assert not (state/'run/install').exists()
    assert not list(module.glob('install-*.log')) and not list(module.glob('install-*.exit'))
    print('PASS: live service defers migration; no install scratch remains')

with tempfile.TemporaryDirectory(prefix='nova-bootstrap-',dir=os.environ.get('TMPDIR')) as folder:
    root=Path(folder);utility=root/'util_functions.sh';entry=root/'update-binary'
    entry.write_text((project/'module-template/META-INF/com/google/android/update-binary').read_text()
        .replace('/data/adb/magisk/util_functions.sh',utility.as_posix()), newline='\n')
    for version,code in [(1,0),(999999,0),(27000,42)]:
        utility.write_text(f'MAGISK_VER_CODE={version}\ninstall_module() {{ echo "installer:$OUTFD:$ZIPFILE"; return {code}; }}\n', newline='\n')
        run=subprocess.run(['sh',entry.as_posix(),'3','9','fixture.zip'],capture_output=True,text=True)
        assert run.returncode==code and 'installer:9:fixture.zip' in run.stdout,(version,run.returncode,run.stdout,run.stderr)
        print('PASS: bootstrap returns installer status',code)
    utility.write_text('MAGISK_VER_CODE=999999\n', newline='\n')
    run=subprocess.run(['sh',entry.as_posix(),'3','9','fixture.zip'],capture_output=True,text=True)
    assert run.returncode==1 and '缺少 install_module' in run.stdout
    print('PASS: missing installation API rejected')
