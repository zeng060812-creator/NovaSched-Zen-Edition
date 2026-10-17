#!/system/bin/sh
# ZenJooo · Update an installed NovaSched WebUI without replacing its daemon.
set -eu
export PATH="/system/bin:/system/xbin:/data/adb/ksu/bin:/data/adb/ap/bin:/data/adb/magisk:$PATH"
if [ "$(id -u)" != 0 ]; then
  echo "请在 root 终端运行这个脚本。" >&2
  exit 1
fi
SCRIPT_DIR=$(CDPATH= cd "${0%/*}" && pwd)
SOURCE_DIR="$SCRIPT_DIR/../module-template/webroot"
MODULE_DIR=${1:-/data/adb/modules/NovaSched_Zen_Edition}
if [ ! -f "$MODULE_DIR/module.prop" ] && [ "$#" -eq 0 ]; then
  MODULE_DIR=/data/adb/ap/modules/NovaSched_Zen_Edition
fi
NOVA_VERSION=$(sed -n 's/^version=//p' "$SCRIPT_DIR/../module-template/module.prop")
if [ -z "$NOVA_VERSION" ] || [ ! -f "$MODULE_DIR/module.prop" ] || ! grep -q '^id=NovaSched_Zen_Edition$' "$MODULE_DIR/module.prop" || ! grep -qx "version=$NOVA_VERSION" "$MODULE_DIR/module.prop" || [ ! -x "$MODULE_DIR/bin/novasched" ]; then
  echo "请先完整安装 $NOVA_VERSION，再更新同一版本的界面。" >&2
  exit 1
fi
for file in index.html assets/app.js assets/app.css assets/liquid-glass.js assets/host.js assets/version.js; do
  if [ ! -s "$SOURCE_DIR/$file" ]; then
    echo "源码包缺少 WebUI 文件：$file" >&2
    exit 1
  fi
done
STAGING="$MODULE_DIR/.webroot-new-$$"
BACKUP="$MODULE_DIR/.webroot-before-update-$$"
LOCK="$MODULE_DIR/.webui-update.lock"
mkdir "$LOCK" 2>/dev/null || { echo "另一个界面更新正在进行" >&2; exit 1; }
rollback() {
  code=$?
  trap - EXIT HUP INT TERM
  if [ "$code" -ne 0 ] && [ -d "$BACKUP" ] && [ ! -d "$MODULE_DIR/webroot" ]; then
    mv "$BACKUP" "$MODULE_DIR/webroot" || true
  fi
  [ ! -d "$STAGING" ] || rm -rf "$STAGING"
  if [ "$code" -eq 0 ]; then [ ! -d "$BACKUP" ] || rm -rf "$BACKUP"; fi
  rmdir "$LOCK" || true
  exit "$code"
}
trap rollback EXIT
trap 'exit 1' HUP INT TERM
mkdir "$STAGING"
cp -R "$SOURCE_DIR/." "$STAGING/"
find "$STAGING" -type d -exec chmod 0755 {} \;
find "$STAGING" -type f -exec chmod 0644 {} \;
if [ -d "$MODULE_DIR/webroot" ]; then mv "$MODULE_DIR/webroot" "$BACKUP"; fi
mv "$STAGING" "$MODULE_DIR/webroot"
echo "WebUI 已更新。关闭管理器内的页面，再重新打开。"
echo "此脚本仅更新界面；FAS 后端与四档新策略需要完整编译安装并重启。"
echo "临时回滚副本已清理。"
