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
NOVA_READY_LIMIT=100
while :; do
  nova_module_available || exit 0
  NOVA_READY_ERROR=$("$MODDIR/bin/novasched" boot-ready --module-dir "$MODDIR" 2>&1)
  NOVA_READY_CODE=$?
  [ "$NOVA_READY_CODE" -eq 0 ] && break
  if [ "$NOVA_READY_CODE" -eq 126 ] || [ "$NOVA_READY_CODE" -eq 127 ]; then
    # 126/127 = ELF 无法执行（架构/ABI/装载失败）：重试没有意义，立即
    # 放弃并留下诊断；解锁等待（退出码 1）继续按上限重试。
    printf '%s startup-gate: 二进制无法执行(退出码 %s)，放弃本次开机自启\n%s\n' \
      "$(date '+%Y-%m-%d %H:%M:%S')" "$NOVA_READY_CODE" "$NOVA_READY_ERROR"
    exit 1
  fi
  if [ $((NOVA_READY_WAIT % 10)) -eq 0 ]; then
    printf '%s startup-gate: %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$NOVA_READY_ERROR"
  fi
  NOVA_READY_WAIT=$((NOVA_READY_WAIT + 1))
  if [ "$NOVA_READY_WAIT" -ge "$NOVA_READY_LIMIT" ]; then
    # 有界等待：解锁状态始终无法确认时放弃本次开机自启（留有诊断日志），
    # 不再无限循环占用启动服务——那正是“守护未启动、WebUI 一直离线”的
    # 一种根因。WebUI 打开时的补启动会重试同一门控并向用户展示原因。
    printf '%s startup-gate: 等待 %s 次后放弃，解锁状态无法确认；可打开 WebUI 手动启动\n' \
      "$(date '+%Y-%m-%d %H:%M:%S')" "$NOVA_READY_WAIT"
    exit 1
  fi
  sleep 3
done
printf '%s startup-gate passed; settling\n' "$(date '+%Y-%m-%d %H:%M:%S')"
# Let SystemUI and the vendor power stack finish their first-unlock work.
# Recheck availability during the wait so a disable/uninstall cancels it.
# 连续通过即可继续；带总时限，避免探测抖动导致永远到不了启动那一步
# （守护内部的启动保护仍会独立确认稳定）。
NOVA_SETTLE=0
NOVA_SETTLE_DEADLINE=$(( $(date +%s) + 150 ))
while [ "$NOVA_SETTLE" -lt 8 ]; do
  nova_module_available || exit 0
  sleep 2
  if [ "$(date +%s)" -ge "$NOVA_SETTLE_DEADLINE" ]; then
    printf '%s settle: 达到等待上限，继续启动\n' "$(date '+%Y-%m-%d %H:%M:%S')"
    break
  fi
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
