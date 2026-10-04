#!/system/bin/sh
MODDIR=${0%/*}
export PATH="/system/bin:/system/xbin:/data/adb/ksu/bin:/data/adb/ap/bin:/data/adb/magisk:$PATH"
mkdir -p /data/adb/novasched || exit 1
"$MODDIR/bin/novasched" restore --module-dir "$MODDIR" >>/data/adb/novasched/uninstall.log 2>&1 || exit 1
exec "$MODDIR/bin/novasched" scene-restore --module-dir "$MODDIR" >>/data/adb/novasched/uninstall.log 2>&1
