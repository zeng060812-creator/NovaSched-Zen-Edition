#!/system/bin/sh
# NovaSched_Zen_Edition_Scene_Provider
# 原入口仅用于卸载恢复；调度请求由 NovaSched 统一执行。
NOVA_MODULE=@NOVASCHED_MODULE_DIR@
case "$1" in
  powersave|balance|performance|fast|standby|init)
    [ ! -e "$NOVA_MODULE/disable" ] && [ ! -e "$NOVA_MODULE/remove" ] && [ ! -e "$NOVA_MODULE/.uninstalling" ] || exit 0
    exec "$NOVA_MODULE/bin/novasched" scene-mode "$1" --module-dir "$NOVA_MODULE" ;;
  pedestal) exit 0 ;;
esac
exit 0
