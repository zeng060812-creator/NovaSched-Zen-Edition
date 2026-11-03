#!/system/bin/sh
# NovaSched_Zen_Edition_Scene_Provider
# 原入口仅用于卸载恢复；调度请求由 NovaSched 统一执行。
NOVA_MODULE=@NOVASCHED_MODULE_DIR@
# ZenJooo · Scene 每次切档都会执行本回调。把请求档位写入系统属性做存档：
# 守护与 WebUI 据此显示「Scene 接管中」并锁定四档按钮；属性非 persist，
# 重启自动清零，无需额外恢复逻辑。init 是 Scene 的初始化调用，视为重置。
# standby 是 Scene 的待机语义，按本模块四档映射为 powersave。
case "$1" in
  standby) setprop novasched.scene.mode powersave ;;
  powersave|balance|performance|fast) setprop novasched.scene.mode "$1" ;;
  init) setprop novasched.scene.mode "" ;;
esac
case "$1" in
  powersave|balance|performance|fast|standby|init)
    [ ! -e "$NOVA_MODULE/disable" ] && [ ! -e "$NOVA_MODULE/remove" ] && [ ! -e "$NOVA_MODULE/.uninstalling" ] || exit 0
    exec "$NOVA_MODULE/bin/novasched" scene-mode "$1" --module-dir "$NOVA_MODULE" ;;
  pedestal) exit 0 ;;
esac
exit 0
