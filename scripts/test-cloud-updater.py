"""Updater protocol fixtures; no network or real manager is used."""
import hashlib,os,subprocess,tempfile,zipfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix='nova-update-',dir=os.environ.get('TMPDIR')) as tmp:
 root=Path(tmp);module=root/'module';(module/'bin').mkdir(parents=True);(module/'module.prop').write_text('id=NovaSched_Zen_Edition\nversion=v2.11.0\n', newline='\n')
 state=root/'data';script=module/'bin/cloud-update.sh';script.write_text((ROOT/'module-template/bin/cloud-update.sh').read_text().replace('NOVA_ROOT=/data/adb/novasched','NOVA_ROOT="'+state.as_posix()+'"'), newline='\n')
 shim=root/'shim';shim.mkdir();record=root/'installs';network=root/'requests';archive=root/'release.zip'
 def asset(identity='NovaSched_Zen_Edition'):
  with zipfile.ZipFile(archive,'w') as z:z.writestr('module.prop','id='+identity+'\nversion=v9.9.9\n');z.writestr('bin/novasched',os.urandom(4096))
 asset()
 curl=shim/'curl';curl.write_text('''#!/usr/bin/env python3
import sys,os,pathlib,hashlib
args=sys.argv[1:];url=args[-1];pathlib.Path(os.environ['FIX_NETWORK']).open('a').write(url+'\\n')
if '-w' in args:print('https://github.com/zeng060812-creator/NovaSched-Zen-Edition/releases/tag/v9.9.9',end='');sys.exit(0)
out=pathlib.Path(args[args.index('-o')+1]);data=pathlib.Path(os.environ['FIX_ASSET']).read_bytes()
if url.endswith('SHA256SUMS.txt'):
 if os.environ.get('SUM_FAIL'):sys.exit(22)
 out.write_text(hashlib.sha256(data).hexdigest()+'  NovaSched-v9.9.9-release.zip\\n', newline='\\n')
else:out.write_bytes(b'bad' if os.environ.get('BAD_DIRECT') and not url.startswith('https://ghfast.top/') else data)
''', newline='\n');curl.chmod(0o755)
 # cloud-update.sh requires uid 0; CI containers usually are root, local
 # developer hosts are not. Shimming `id` keeps the product check intact.
 idshim=shim/'id';idshim.write_text('#!/bin/sh\nif [ "$1" = "-u" ];then echo 0;else exec /usr/bin/id "$@";fi\n', newline='\n');idshim.chmod(0o755)
 manager=shim/'apd';manager.write_text('''#!/bin/sh
case "$*" in
 --help*) echo 'Compatible fork CLI: module';;
 'module --help') echo 'Commands: install';;
 'module install '*) echo "$*" >> "$FIX_INSTALLS";exit "${MANAGER_FAIL:-0}";;
 *) exit 1;;
esac
''', newline='\n');manager.chmod(0o755)
 def run(*args,**opts):
  record.unlink(missing_ok=True);network.unlink(missing_ok=True);env=dict(os.environ,PATH=str(shim)+':'+os.environ['PATH'],FIX_NETWORK=str(network),FIX_ASSET=str(archive),FIX_INSTALLS=str(record));env.update(opts)
  # The script derives MODDIR via ${0%/*}; pass a POSIX path so the
  # truncation works on Windows hosts too.
  r=subprocess.run(['sh',script.as_posix(),*args],env=env,capture_output=True,text=True,timeout=300)
  assert not list(state.glob('run/update-*')) and not (state/'run/update.lock').exists(),(r.stdout,r.stderr)
  return r
 r=run('check');assert r.returncode==0 and 'UPDATE_AVAILABLE=1' in r.stdout and not record.exists();print('PASS updater: release check does not install')
 r=run('install','v9.9.9',BAD_DIRECT='1');assert r.returncode==0 and 'UPDATE_OK=installed' in r.stdout,(r.stdout,r.stderr);assert len(record.read_text().splitlines())==1 and 'https://ghfast.top/' in network.read_text();print('PASS updater: bad download falls back and invokes manager once')
 r=run('install','v9.9.9',SUM_FAIL='1');assert r.returncode!=0 and not record.exists();print('PASS updater: missing official checksum blocks installation')
 r=run('install','v9.9.9;reboot');assert r.returncode!=0 and not record.exists() and not network.exists();print('PASS updater: invalid version is rejected before networking')
 asset('foreign_module');r=run('install','v9.9.9');assert r.returncode!=0 and not record.exists();print('PASS updater: hash-valid foreign module is rejected')
 asset();r=run('install','v9.9.9',MANAGER_FAIL='7');assert r.returncode!=0 and '管理器安装失败' in r.stdout;print('PASS updater: manager failure is visible and temporary files are removed')
