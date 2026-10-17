#!/system/bin/sh
# ZenJooo · Verified release updates; temporary data stays inside the module data root.
set -u
umask 077
export PATH="/system/bin:/system/xbin:/data/adb/ksu/bin:/data/adb/ap/bin:/data/adb/magisk:$PATH"
NOVA_MODULE=${0%/bin/*}
NOVA_ROOT=/data/adb/novasched
NOVA_REPO=https://github.com/zeng060812-creator/NovaSched-Zen-Edition
NOVA_ACTION=${1:-check}
NOVA_TAG=${2:-}
fail() { printf 'UPDATE_FAIL=%s\n' "$1"; exit 1; }
valid_tag() { printf '%s\n' "$1" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$'; }
[ "$(id -u)" = 0 ] || fail '需要 root 授权'
[ -f "$NOVA_MODULE/module.prop" ] && grep -qx 'id=NovaSched_Zen_Edition' "$NOVA_MODULE/module.prop" || fail '模块身份不匹配'
case "$NOVA_ACTION" in check|install) ;; *) fail '未知更新操作' ;; esac
[ -z "$NOVA_TAG" ] || valid_tag "$NOVA_TAG" || fail '版本格式不正确'
[ ! -L "$NOVA_ROOT" ] && [ ! -L "$NOVA_ROOT/run" ] || fail '运行目录不安全'
mkdir -p "$NOVA_ROOT/run" || fail '无法创建更新目录'
NOVA_LOCK="$NOVA_ROOT/run/update.lock"
[ ! -L "$NOVA_LOCK" ] || fail '更新锁不安全'
if ! mkdir "$NOVA_LOCK" 2>/dev/null; then
  NOVA_OLD_BOOT=$(sed -n '1p' "$NOVA_LOCK/owner" 2>/dev/null)
  NOVA_OLD_PID=$(sed -n '2p' "$NOVA_LOCK/owner" 2>/dev/null)
  NOVA_BOOT=$(cat /proc/sys/kernel/random/boot_id 2>/dev/null)
  case "$NOVA_OLD_PID" in ''|*[!0-9]*) fail '更新锁未就绪，请稍后重试' ;; esac
  if [ -n "$NOVA_BOOT" ] && [ "$NOVA_BOOT" != "$NOVA_OLD_BOOT" ] || ! kill -0 "$NOVA_OLD_PID" 2>/dev/null; then
    [ ! -d "$NOVA_ROOT/run/update-$NOVA_OLD_PID" ] || rm -rf "$NOVA_ROOT/run/update-$NOVA_OLD_PID"
    rm -f "$NOVA_LOCK/owner" && rmdir "$NOVA_LOCK" && mkdir "$NOVA_LOCK" || fail '无法清理上次更新锁'
  else fail '已有更新任务正在执行'; fi
fi
NOVA_WORK="$NOVA_ROOT/run/update-$$"
cleanup() {
  NOVA_EXIT=$?
  trap - EXIT HUP INT TERM
  [ ! -d "$NOVA_WORK" ] || rm -rf "$NOVA_WORK"
  rm -f "$NOVA_LOCK/owner"
  rmdir "$NOVA_LOCK" 2>/dev/null || true
  exit "$NOVA_EXIT"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
printf '%s\n%s\n' "$(cat /proc/sys/kernel/random/boot_id 2>/dev/null)" "$$" > "$NOVA_LOCK/owner" || fail '无法记录更新任务'
mkdir "$NOVA_WORK" || fail '无法创建临时目录'
NOVA_CURRENT=$(sed -n 's/^version=//p' "$NOVA_MODULE/module.prop")
valid_tag "$NOVA_CURRENT" || fail '无法读取当前版本'
printf 'CURRENT=%s\n' "$NOVA_CURRENT"
fetch() {
  rm -f "$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fLsS --proto '=https' --proto-redir '=https' --connect-timeout 6 --max-time 40 --max-filesize 33554432 -o "$2" "$1" 2>"$NOVA_WORK/network-error"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -T 20 -O "$2" "$1" 2>"$NOVA_WORK/network-error"
  else return 1; fi
}
if [ -z "$NOVA_TAG" ]; then
  if command -v curl >/dev/null 2>&1; then
    NOVA_URL=$(curl -fLsS --proto '=https' --proto-redir '=https' --connect-timeout 6 --max-time 30 -o /dev/null -w '%{url_effective}' "$NOVA_REPO/releases/latest" 2>"$NOVA_WORK/network-error") || fail '无法连接 Release 页面，请稍后重试'
    NOVA_TAG=${NOVA_URL##*/}
  else
    wget -S --spider -T 20 "$NOVA_REPO/releases/latest" 2>"$NOVA_WORK/headers" || true
    NOVA_TAG=$(sed -n 's@.*[Ll]ocation: .*/releases/tag/\(v[0-9][0-9.]*\).*@\1@p' "$NOVA_WORK/headers" | tail -n 1)
  fi
fi
valid_tag "$NOVA_TAG" || fail '没有获得有效的发布版本'
printf 'LATEST=%s\n' "$NOVA_TAG"
NOVA_NEWER=$(awk -v a="${NOVA_TAG#v}" -v b="${NOVA_CURRENT#v}" 'BEGIN{split(a,x,".");split(b,y,".");for(i=1;i<=3;i++){if(x[i]+0>y[i]+0){print 1;exit}if(x[i]+0<y[i]+0){print 0;exit}}print 0}')
if [ "$NOVA_NEWER" != 1 ]; then printf 'UPDATE_OK=latest\n'; exit 0; fi
printf 'UPDATE_AVAILABLE=1\n'
[ "$NOVA_ACTION" = install ] || exit 0
NOVA_ASSET="NovaSched-$NOVA_TAG-release.zip"
NOVA_SUMS="NovaSched-$NOVA_TAG-SHA256SUMS.txt"
# Only ZIP bytes use mirrors. The checksum must come from the publisher.
fetch "$NOVA_REPO/releases/download/$NOVA_TAG/$NOVA_SUMS" "$NOVA_WORK/SHA256SUMS" || fail '无法读取官方校验文件，未安装任何内容'
NOVA_EXPECTED=$(awk -v file="$NOVA_ASSET" '$2==file && length($1)==64 && $1 !~ /[^0-9a-fA-F]/ {print tolower($1)}' "$NOVA_WORK/SHA256SUMS")
[ "${#NOVA_EXPECTED}" = 64 ] || fail '官方校验文件缺少唯一的模块摘要'
command -v sha256sum >/dev/null 2>&1 || fail '系统缺少 SHA256 工具'
NOVA_DOWNLOAD="$NOVA_REPO/releases/download/$NOVA_TAG/$NOVA_ASSET"
NOVA_OK=false
for NOVA_PREFIX in '' 'https://ghfast.top/'; do
  printf 'UPDATE_STAGE=download\n'
  if fetch "$NOVA_PREFIX$NOVA_DOWNLOAD" "$NOVA_WORK/module.zip"; then
    NOVA_SIZE=$(wc -c < "$NOVA_WORK/module.zip")
    if [ "$NOVA_SIZE" -gt 1024 ] && [ "$NOVA_SIZE" -le 33554432 ]; then
      NOVA_ACTUAL=$(sha256sum "$NOVA_WORK/module.zip")
      NOVA_ACTUAL=${NOVA_ACTUAL%% *}
      if [ "$NOVA_ACTUAL" = "$NOVA_EXPECTED" ]; then NOVA_OK=true; break; fi
    fi
  fi
  rm -f "$NOVA_WORK/module.zip"
done
[ "$NOVA_OK" = true ] || fail '下载或 SHA256 校验失败，请打开 Release 页面手动下载'
command -v unzip >/dev/null 2>&1 || fail '系统缺少 ZIP 检查工具'
unzip -p "$NOVA_WORK/module.zip" module.prop > "$NOVA_WORK/module.prop" 2>/dev/null || fail '发布包不完整'
grep -qx 'id=NovaSched_Zen_Edition' "$NOVA_WORK/module.prop" && grep -qx "version=$NOVA_TAG" "$NOVA_WORK/module.prop" || fail '发布包的模块身份或版本不符'
[ ! -e "$NOVA_MODULE/remove" ] && [ ! -e "$NOVA_MODULE/.uninstalling" ] || fail '模块正在卸载，已取消更新'
[ "$(sed -n 's/^version=//p' "$NOVA_MODULE/module.prop")" = "$NOVA_CURRENT" ] || fail '模块版本已改变，请重新检查'
printf 'UPDATE_STAGE=install\n'
for NOVA_TOOL in "$(command -v apd 2>/dev/null)" /data/adb/apd /data/adb/ap/bin/apd "$(command -v ksud 2>/dev/null)" /data/adb/ksud /data/adb/ksu/bin/ksud "$(command -v magisk 2>/dev/null)" /data/adb/magisk/magisk; do
  [ -n "$NOVA_TOOL" ] && [ -x "$NOVA_TOOL" ] || continue
  NOVA_HELP=$("$NOVA_TOOL" --help 2>&1 || true)
  if printf '%s\n' "$NOVA_HELP" | grep -q -- '--install-module'; then
    "$NOVA_TOOL" --install-module "$NOVA_WORK/module.zip" || fail '管理器安装失败，请查看上方输出'
  elif printf '%s\n' "$NOVA_HELP" | grep -qw module && "$NOVA_TOOL" module --help 2>&1 | grep -qw install; then
    "$NOVA_TOOL" module install "$NOVA_WORK/module.zip" || fail '管理器安装失败，请查看上方输出'
  else continue; fi
  printf 'MANAGER=%s\nUPDATE_OK=installed\n' "${NOVA_TOOL##*/}"
  exit 0
done
fail '未找到兼容的管理器安装命令，请手动刷入'
