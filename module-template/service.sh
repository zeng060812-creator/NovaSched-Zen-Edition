#!/system/bin/sh
MODDIR=${0%/*}
nova_module_available() {
  [ -d "$MODDIR" ] && [ ! -e "$MODDIR/remove" ] &&
    [ ! -e "$MODDIR/disable" ] && [ ! -e "$MODDIR/.uninstalling" ]
}
nova_module_available || exit 0
umask 077
export PATH="/system/bin:/system/xbin:/data/adb/ksu/bin:/data/adb/ap/bin:/data/adb/magisk:$PATH"
# The installer prepares this directory. A late shell must not recreate it after uninstall.
NOVA_STATE=/data/adb/novasched
[ ! -L "$NOVA_STATE" ] && [ ! -L "$NOVA_STATE/logs" ] || exit 1
nova_module_available || exit 0
NOVA_BOOT_LOG="$NOVA_STATE/logs/boot-entry.log"
[ ! -L "$NOVA_BOOT_LOG" ] || exit 1
if [ -d "$NOVA_STATE/logs" ]; then
  exec >"$NOVA_BOOT_LOG" 2>&1 || exit 1
else
  printf '%s\n' 'novasched: 启动日志目录缺失；完整安装模块可重建' >&2
fi
printf '%s service.sh entered; waiting for boot and first unlock\n' "$(date '+%Y-%m-%d %H:%M:%S')"
until [ "$(getprop sys.boot_completed)" = "1" ]; do
  nova_module_available || exit 0
  sleep 2
done
NOVA_READY_WAIT=0
until NOVA_READY_ERROR=$("$MODDIR/bin/novasched" boot-ready --module-dir "$MODDIR" 2>&1); do
  nova_module_available || exit 0
  if [ $((NOVA_READY_WAIT % 10)) -eq 0 ]; then
    printf '%s startup-gate: %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$NOVA_READY_ERROR"
  fi
  "$MODDIR/bin/novasched" version >/dev/null 2>&1 || exit 1
  NOVA_READY_WAIT=$((NOVA_READY_WAIT + 1))
  sleep 3
done
printf '%s startup-gate passed; settling for 30 seconds\n' "$(date '+%Y-%m-%d %H:%M:%S')"
# Let SystemUI and the vendor power stack finish their first-unlock work.
# Recheck availability during the wait so a disable/uninstall cancels it.
NOVA_SETTLE=0
while [ "$NOVA_SETTLE" -lt 15 ]; do
  nova_module_available || exit 0
  sleep 2
  if "$MODDIR/bin/novasched" boot-ready --module-dir "$MODDIR" >/dev/null 2>&1; then
    NOVA_SETTLE=$((NOVA_SETTLE + 1))
  else
    NOVA_SETTLE=0
  fi
done
nova_module_available || exit 0
"$MODDIR/bin/novasched" start --module-dir "$MODDIR"
NOVA_EXIT=$?
printf '%s start exited: %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$NOVA_EXIT"
exit "$NOVA_EXIT"
