#!/system/bin/sh
MODDIR=${0%/*}
export PATH="/system/bin:/system/xbin:/data/adb/ksu/bin:/data/adb/ap/bin:/data/adb/magisk:$PATH"
exec "$MODDIR/bin/novasched" restart --module-dir "$MODDIR"
