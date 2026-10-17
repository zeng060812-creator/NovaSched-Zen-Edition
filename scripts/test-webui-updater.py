"""Exercise the WebUI updater and its rollback inside temporary module directories."""
from pathlib import Path
import os
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / 'scripts/install-webui.sh'
# geteuid only exists on Unix; on Windows hosts the fixture stays inside a
# temporary directory and is safe to run unprivileged.
if hasattr(os, 'geteuid') and os.geteuid() != 0:
    raise SystemExit('Run this fixture as root; it never writes to a device module directory.')
with tempfile.TemporaryDirectory(prefix='webui-updater-', dir=ROOT) as temporary:
    base=Path(temporary)
    # install-webui.sh requires uid 0; shim `id` on non-root hosts so the
    # product check stays intact while the fixture runs unprivileged.
    idshim_dir = base / 'id-shim'
    idshim_dir.mkdir()
    idshim = idshim_dir / 'id'
    idshim.write_text('#!/bin/sh\nif [ "$1" = "-u" ];then echo 0;else exec /usr/bin/id "$@";fi\n', newline='\n')
    idshim.chmod(0o755)
    os.environ['PATH'] = str(idshim_dir) + ':' + os.environ['PATH']
    def module(name, valid=True):
        path=base/name
        (path/'bin').mkdir(parents=True)
        (path/'module.prop').write_text((ROOT/'module-template/module.prop').read_text().replace('id=NovaSched_Zen_Edition','id='+('NovaSched_Zen_Edition' if valid else 'unrelated_module')), newline='\n')
        binary=path/'bin/novasched';binary.write_bytes(b'#!/bin/sh\nfixture executable\n');binary.chmod(0o755)
        (path/'webroot').mkdir();(path/'webroot/index.html').write_text('old UI', newline='\n')
        return path
    target=module('normal')
    result=subprocess.run(['sh',SCRIPT.as_posix(),target.as_posix()],capture_output=True,text=True)
    assert result.returncode==0,result.stderr
    assert (target/'webroot/index.html').read_bytes()==(ROOT/'module-template/webroot/index.html').read_bytes()
    assert (target/'bin/novasched').read_bytes()==b'#!/bin/sh\nfixture executable\n'
    assert not list(target.glob('.webroot-*')) and not (target/'.webui-update.lock').exists()
    print('PASS updater: retained daemon and removed rollback scratch')
    target=module('wrong-id',False)
    result=subprocess.run(['sh',SCRIPT.as_posix(),target.as_posix()],capture_output=True,text=True)
    assert result.returncode!=0
    assert (target/'webroot/index.html').read_text()=='old UI'
    print('PASS updater: rejects unrelated modules before changing files')
    target=module('rollback')
    shim=base/'command-shim';shim.mkdir()
    mv=shim/'mv'
    real_mv=shutil.which('mv')
    mv.write_text('#!/bin/sh\ncase "$1" in */.webroot-new-*) exit 13;; esac\nexec "'+real_mv.replace('\\','/')+'" "$@"\n', newline='\n');mv.chmod(0o755)
    environment=dict(os.environ,PATH=str(shim)+':'+os.environ['PATH'])
    result=subprocess.run(['sh',SCRIPT.as_posix(),target.as_posix()],capture_output=True,text=True,env=environment)
    assert result.returncode!=0
    assert (target/'webroot/index.html').read_text()=='old UI'
    assert not list(target.glob('.webroot-new-*'))
    print('PASS updater: failed final swap restores old UI and removes staging')
