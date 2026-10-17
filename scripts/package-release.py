"""Package the already-built ELF with current assets; verify archive contents.

Use --source-only to package an editable checkout without an Android toolchain.

Run scripts/build.sh first. This does not compile or alter the scheduler.
"""
from pathlib import Path
import hashlib
import json
import argparse
import re
import runpy
import stat
import zipfile

ROOT = Path(__file__).resolve().parents[1]
MODULE = ROOT / 'module-template'
DIST = ROOT / 'dist'
DIST.mkdir(exist_ok=True)
# module.prop owns release identity; profile generations are checked separately.
props = dict(line.split('=', 1) for line in (MODULE/'module.prop').read_text().splitlines() if '=' in line)
VERSION = props['version'].removeprefix('v')
VERSION_CODE = int(props['versionCode'])
CONFIG_VERSION = int(re.search(r'PROFILE_VERSION: i64 = (\d+)', (ROOT/'native/src/config.rs').read_text()).group(1))
parser = argparse.ArgumentParser()
parser.add_argument('--source-only', action='store_true', help='Package source without requiring an Android binary')
args = parser.parse_args()
profiles = runpy.run_path(str(ROOT/'scripts/generate-profiles.py'))
profiles['generate'](check=True)
SUPPORTED = profiles['LAYOUTS']

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

assert f'version=v{VERSION}' in (MODULE/'module.prop').read_text()
assert f'versionCode={VERSION_CODE}' in (MODULE/'module.prop').read_text()
for soc in SUPPORTED:
    profile=json.loads((MODULE/'config'/(soc+'.json')).read_text())
    assert set(profile['modes']) == {'powersave','balance','performance','fast'}
    assert profile['schema'] == 'novasched/2' and profile['module'] == 'NovaSched_Zen_Edition'
    assert profile['meta']['author'] == 'ZenJooo' and profile['meta']['version'] == CONFIG_VERSION
    assert profile['meta']['soc'] == soc
    assert not {'smooth','extreme'} & profile['features'].keys()
assert len(list((MODULE/'config').glob('*.json'))) == len(SUPPORTED)
for retired in [ROOT/'profile-references',MODULE/'profile-migrations']:
    assert not retired.exists(), 'Historical template copies must not ship'
assert json.loads((MODULE/'vtools/powercfg.json').read_text())['versionCode'] == VERSION_CODE
assert json.loads((MODULE/'vtools/powercfg.json').read_text())['version'] == f'v{VERSION}', 'powercfg.json version string must track VERSION'
assert f'version = "{VERSION}"' in (ROOT/'native/Cargo.toml').read_text()
assert f'version = "{VERSION}"' in (ROOT/'native/Cargo.lock').read_text()
assert json.loads((ROOT/'package.json').read_text())['version'] == VERSION
assert json.loads((ROOT/'package-lock.json').read_text())['version'] == VERSION
release_ui = (MODULE/'webroot/assets/version.js').read_text()
assert f'version: "{VERSION}"' in release_ui and f'versionCode: {VERSION_CODE}' in release_ui
assert props['author'] == 'ZenJooo'
installer = (MODULE/'customize.sh').read_text()
assert '"$MODPATH/module.prop"' in installer, 'Installer must read release metadata'
assert 'NovaSched Zen Edition $NOVA_VERSION' in installer and 'NovaSched $NOVA_VERSION · ZenJooo' in installer
assert not re.search(r'NovaSched[^\n]*v\d+\.\d+\.\d+', installer), 'Hardcoded installer version'

ui = (MODULE/'webroot/assets/app.js').read_text()
html = (MODULE/'webroot/index.html').read_text()
css = (MODULE/'webroot/assets/app.css').read_text()
ws = (ROOT/'native/src/websocket.rs').read_text()
key = re.search(r'const KEY = "([^"]+)"', ui).group(1)
assert f'"{key}"' in ws, 'UI and Rust WebSocket keys do not match'
auth = (ROOT/'native/src/web_auth.rs').read_text()
prefix = re.search(r'const AUTH_PREFIX = "([^"]+)"', ui).group(1)
assert f'"{prefix}"' in auth, 'UI and Rust authentication prefixes do not match'
assert '[KEY, AUTH_PREFIX + sessionToken]' in ui, 'Authenticated WebSocket setup missing'
assert 'moduleCommand("webui-session"' in ui, 'Root credential getter missing'
assert 'session.accepts(item)' in ws, 'Server token verification missing'
assert 'web_auth::origin_is_allowed' in ws and 'origin_allowed(value)' in ws, 'Exact dynamic Origin validation missing'
assert 'register_origin(Path::new(STATE_DIR), origin)' in auth, 'Root Origin registration missing'
assert '--origin ${shellQuote(origin)}' in ui, 'Origin-bound root credential request missing'
# The liquid glass renderer is the only glass engine; no vendored third-party
# distribution ships in the module.
assert not (MODULE/'webroot/vendor').exists(), 'vendored glass library should be removed'
assert 'assets/liquid-glass.js' in html
assert 'assets/host.js' in html and 'assets/version.js' in html
assert f'v{VERSION}' in html
assert not re.search(r'v2\.(?!'+re.escape(VERSION[2:])+r')\d+\.\d+', html)
assert 'assets/glass.js' not in html and 'stat-pkg-raw' in html
for page in ['overview', 'rules', 'logs', 'about']:
    assert f'id="page-{page}"' in html
assert 'id="page-modes"' not in html, 'Mode grid lives on overview'
assert 'scenePresence' in ui
assert 'data.fasEngine' in ui and 'fasEngine:{}' in ws, 'NovaFAS status wiring missing'
assert f'const UI_VERSION = window.NovaRelease?.version || "{VERSION}";' in ui, 'Stale UI version fallback'

# Explicit source roots work both in a checkout and an extracted source archive.
# Build output, dependencies and runtime state are never part of the source ZIP.
source_paths = set()
for directory in ['native/src', 'native/tests', 'module-template', 'scripts', 'docs', '.github']:
    base = ROOT / directory
    if base.exists():
        source_paths.update(path for path in base.rglob('*') if path.is_file())
for name in ['.gitignore', 'README.md', 'CHANGELOG.md', 'LICENSE', 'NOTICE',
             'package.json', 'package-lock.json', 'native/Cargo.toml', 'native/Cargo.lock', 'native/games.list']:
    path = ROOT / name
    if path.is_file():
        source_paths.add(path)
source_entries = []
for path in sorted(source_paths):
    name = path.relative_to(ROOT).as_posix()
    if (name == 'module-template/bin/novasched' or '__pycache__' in path.parts
            or path.suffix == '.pyc' or name.endswith('.log') or name.endswith('webui.session')):
        continue
    source_entries.append((path, f'NovaSched-Zen-Edition/{name}', path.suffix == '.sh' or path.name == 'update-binary'))
source_zip = DIST / f'NovaSched-v{VERSION}-source.zip'
archive(source_zip, source_entries)
with zipfile.ZipFile(source_zip) as z:
    assert 'NovaSched-Zen-Edition/native/src/options.rs' not in z.namelist()
    assert not any('/target/' in name or '/node_modules/' in name or name.endswith('/bin/novasched') for name in z.namelist())
    for soc in SUPPORTED:
        assert f'NovaSched-Zen-Edition/module-template/config/{soc}.json' in z.namelist()
if args.source_only:
    (DIST/f'NovaSched-v{VERSION}-SHA256SUMS.txt').write_text(''.join(digest(path)+'  '+path.name+'\n' for path in [source_zip]))
    print('Source only. No Android ELF exists; these archives are not flashable.')
    raise SystemExit(0)

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
assert load_records, 'ELF has no readable LOAD program headers'
for record in load_records:
    assert record.split()[-1] == '0x4000', 'ELF LOAD alignment is not 16KB'
def executable(f):
    return f.suffix == '.sh' or f == binary or f.name == 'update-binary'

allowed_roots={'META-INF','bin','config','vtools','webroot'}
allowed_files={'LICENSE','NOTICE','action.sh','customize.sh','service.sh','uninstall.sh','module.prop','skip_mount'}
for file in MODULE.rglob('*'):
    if not file.is_file():continue
    rel=file.relative_to(MODULE)
    assert rel.parts[0] in allowed_roots or str(rel) in allowed_files, f'Unexpected module file: {rel}'
    assert not file.is_symlink() and not any(p.startswith('.') for p in rel.parts), f'Hidden/link file: {rel}'
    assert not any(s in file.name for s in ['.backup','.bak','.tmp','.log','.exit','session']), f'Runtime file: {rel}'
assert (MODULE/'bin/cloud-update.sh').is_file(), 'Updater missing'
assert json.loads((MODULE/'vtools/powercfg.json').read_text())['features']['pedestal'] is False
module_entries = [(f, f.relative_to(MODULE).as_posix(), executable(f))
                  for f in sorted(MODULE.rglob('*')) if f.is_file() and f.suffix.lower() != '.md']
flashable = DIST / f'NovaSched-v{VERSION}-release.zip'
archive(flashable, module_entries)
with zipfile.ZipFile(flashable) as z:
    assert z.read('bin/novasched') == binary.read_bytes()
    assert not any(name.endswith('webui.session') for name in z.namelist()), 'Private session included in release archive'
    assert z.read('META-INF/com/google/android/updater-script').strip() == b'#MAGISK'
    for name in ['bin/novasched','bin/cloud-update.sh','action.sh','service.sh','customize.sh','uninstall.sh','vtools/powercfg.sh','vtools/powercfg-base.sh','META-INF/com/google/android/update-binary']:
        assert (z.getinfo(name).external_attr >> 16) & 0o777 == 0o755

print('Final release archive passed CRC, metadata, executable modes, protocol key and ELF consistency checks.')

sums = DIST / f'NovaSched-v{VERSION}-SHA256SUMS.txt'
sums.write_text(''.join(digest(path)+'  '+path.name+'\n' for path in [source_zip,flashable]))
print(sums.name)
