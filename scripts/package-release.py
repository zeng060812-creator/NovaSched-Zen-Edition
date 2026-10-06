"""Package the already-built ELF with current assets; verify archive contents.

Run scripts/build.sh first. This does not compile or alter the scheduler.
"""
from pathlib import Path
import hashlib
import json
import os
import re
import runpy
import stat
import zipfile

ROOT = Path(__file__).resolve().parents[1]
MODULE = ROOT / 'module-template'
DIST = ROOT / 'dist'
DIST.mkdir(exist_ok=True)
VERSION = '1.1.0'

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def archive(path, entries):
    with zipfile.ZipFile(path, 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        for source, target, executable in entries:
            info = zipfile.ZipInfo(target)
            info.create_system = 3
            info.external_attr = (stat.S_IFREG | (0o755 if executable else 0o644)) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, source.read_bytes())
    with zipfile.ZipFile(path) as z:
        assert z.testzip() is None, 'ZIP CRC check failed'
    print(path.name, path.stat().st_size, digest(path))

binary = MODULE / 'bin/novasched'
built = ROOT / 'native/target-static/aarch64-linux-android/release/novasched'
assert binary.read_bytes() == built.read_bytes(), 'Module ELF differs from build output'
assert binary.read_bytes()[:4] == b'\x7fELF'
header = (ROOT / 'native/target-static/elf-header.txt').read_text()
programs = (ROOT / 'native/target-static/elf-programs.txt').read_text()
dynamic = (ROOT / 'native/target-static/elf-dynamic.txt').read_text()
assert 'AArch64' in header and 'EXEC (Executable file)' in header
assert 'INTERP' not in programs and 'NEEDED' not in dynamic
# GNU binutils wraps each program header onto a second line that carries the
# alignment column; llvm-readelf (NDK) keeps one line per record. Accept both.
program_lines = programs.splitlines()
load_records = []
for index, line in enumerate(program_lines):
    if line.strip().startswith('LOAD'):
        load_records.append(line + program_lines[index + 1] if len(line.split()) == 4 and index + 1 < len(program_lines) else line)
for record in load_records:
    assert record.split()[-1] == '0x4000', 'ELF LOAD alignment is not 16KB'
assert f'version=v{VERSION}' in (MODULE/'module.prop').read_text()
assert 'versionCode=229' in (MODULE/'module.prop').read_text()
runpy.run_path(str(ROOT/'scripts/generate-profiles.py'))['generate'](check=True)
for soc in ['SM8450','SM8475','SM8550','SM8650','SM8750','SM8850']:
    profile=json.loads((MODULE/'config'/(soc+'.json')).read_text())
    assert set(profile['modes']) == {'powersave','balance','performance','fast'}
    assert profile['schema'] == 'novasched/2' and profile['module'] == 'NovaSched_Zen_Edition'
    assert profile['meta']['author'] == 'ZenJooo' and profile['meta']['version'] == 229
    assert profile['meta']['soc'] == soc
assert len(list((MODULE/'config').glob('*.json'))) == 6
for retired in [ROOT/'profile-references',MODULE/'profile-migrations']:
    assert not retired.exists(), 'Historical template copies must not ship'
assert json.loads((MODULE/'vtools/powercfg.json').read_text())['versionCode'] == 229
assert f'version = "{VERSION}"' in (ROOT/'native/Cargo.toml').read_text()

js = (MODULE/'webroot/assets/zen.js').read_text()
ws = (ROOT/'native/src/websocket.rs').read_text()
key = re.search(r'const KEY = "([^"]+)"', js).group(1)
assert f'"{key}"' in ws, 'UI and Rust WebSocket keys do not match'
auth = (ROOT/'native/src/web_auth.rs').read_text()
prefix = re.search(r'const AUTH_PREFIX = "([^"]+)"', js).group(1)
assert f'"{prefix}"' in auth, 'UI and Rust authentication prefixes do not match'
assert '[KEY, AUTH_PREFIX + sessionToken]' in js, 'Authenticated WebSocket setup missing'
assert 'moduleCommand("webui-session"' in js, 'Root credential getter missing'
assert 'session.accepts(item)' in ws, 'Server token verification missing'
assert 'web_auth::origin_is_allowed' in ws and 'origin_allowed(value)' in ws, 'Exact dynamic Origin validation missing'
assert 'register_origin(Path::new(STATE_DIR), origin)' in auth, 'Root Origin registration missing'
assert '--origin ${shellQuote(origin)}' in js, 'Origin-bound root credential request missing'
assert 'cts-v101' not in (MODULE/'webroot/index.html').read_text()

def executable(f):
    return f.suffix == '.sh' or f == binary or f.name == 'update-binary'

module_entries = [(f, f.relative_to(MODULE).as_posix(), executable(f))
                  for f in sorted(MODULE.rglob('*')) if f.is_file() and f.suffix.lower() != '.md']
flashable = DIST / f'NovaSched-v{VERSION}-release.zip'
archive(flashable, module_entries)
with zipfile.ZipFile(flashable) as z:
    assert z.read('bin/novasched') == binary.read_bytes()
    assert not any(name.endswith('webui.session') for name in z.namelist()), 'Private session included in release archive'
    assert z.read('META-INF/com/google/android/updater-script').strip() == b'#MAGISK'
    for name in ['bin/novasched','action.sh','service.sh','customize.sh','uninstall.sh','vtools/powercfg.sh','META-INF/com/google/android/update-binary']:
        assert (z.getinfo(name).external_attr >> 16) & 0o777 == 0o755

print('Final release archive passed CRC, metadata, executable modes, protocol key and ELF consistency checks.')

sums = DIST / 'NovaSched-v0229-SHA256SUMS.txt'
sums.write_text(''.join(digest(path)+'  '+path.name+'\n' for path in [flashable,binary]))
print(sums.name)
