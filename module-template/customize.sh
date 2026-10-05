#!/system/bin/sh

SKIPUNZIP=0

 ui_print "- NovaSched Zen Edition v0.2.18-rc2"
ui_print "- 作者：ZenJooo"
ui_print "- 骁龙 8 Gen 1 / 8+ Gen 1 / 8 Gen 2 / 8 Gen 3 / 8 Elite / 8 Elite Gen 5"
ui_print "- 按处理器与内核能力校验，不限制手机品牌或机型"
ui_print "- 支持 Magisk / Alpha / KernelSU 系列 / APatch，不按管理器版本号拦截"

if [ "$ARCH" != "arm64" ] && [ "$ARCH" != "arm64-v8a" ]; then
  abort "! 仅支持 arm64-v8a"
fi


LEGACY_MODULE="/data/adb/modules/LittleYouran_CTS_Rust"
if [ -d "$LEGACY_MODULE" ] && [ ! -e "$LEGACY_MODULE/disable" ] && [ ! -e "$LEGACY_MODULE/remove" ]; then
  abort "! 检测到仍启用的旧模块，请先在 root 管理器中停用/卸载旧模块并重启，再安装 NovaSched。"
fi

set_perm_recursive "$MODPATH" 0 0 0755 0644
set_perm "$MODPATH/bin/novasched" 0 0 0755
set_perm "$MODPATH/action.sh" 0 0 0755
set_perm "$MODPATH/service.sh" 0 0 0755
set_perm "$MODPATH/uninstall.sh" 0 0 0755
set_perm "$MODPATH/vtools/powercfg.sh" 0 0 0755

run_check() {
  CHECK_LOG="$MODPATH/install-check.log"
  "$MODPATH/bin/novasched" "$1" --module-dir "$MODPATH" > "$CHECK_LOG" 2>&1
  CHECK_STATUS=$?
  while IFS= read -r line || [ -n "$line" ]; do
    ui_print "$line"
  done < "$CHECK_LOG"
  return "$CHECK_STATUS"
}

ui_print "- 检查运行核心和配置解析"
run_check self-test || abort "! 二进制自检失败，安装已中止，请保留输出"
ui_print "- 使用 Rust 核心自动识别处理器、CPU policy、cgroup 和可用调速器（不下发策略）"
run_check probe || abort "! 硬件检测未通过，具体原因见上方输出和 install-check.log"

# 转换旧配置时保留参数；替换前在运行目录保存用户配置。
NOVA_STATE="/data/adb/novasched"
mkdir -p "$NOVA_STATE" || abort "! 无法创建运行时目录"
run_check prepare-config || abort "! 处理器配置初始化失败；原配置已保留，请保留输出"
ui_print "- 使用 NovaSched 自有配置格式；保留手动参数，转换前保存原配置"
if [ -f "$NOVA_STATE/options.txt" ]; then
  # 覆盖安装保留已有选择，只为旧格式补上新增开关。
  if ! grep -q '^extreme_powersave=' "$NOVA_STATE/options.txt"; then
    printf '\nextreme_powersave=0\n' >> "$NOVA_STATE/options.txt" || abort "! 无法写入极限节能选项"
  fi
  if ! grep -q '^smooth_powersave=' "$NOVA_STATE/options.txt"; then
    printf '\nsmooth_powersave=0\n' >> "$NOVA_STATE/options.txt" || abort "! 无法写入流畅省电选项"
  fi
else
  printf '# NovaSched Zen Edition\nextreme_powersave=0\nsmooth_powersave=0\n' > "$NOVA_STATE/options.txt" || abort "! 无法初始化选项"
fi
set_perm "$NOVA_STATE/options.txt" 0 0 0664
ui_print "- 新装默认关闭极限节能与流畅省电；覆盖安装保留已有选择"

ui_print "- 安装完成；首次启动将自动保存原厂节点快照"
ui_print "- 检测到已安装 Scene 时自动注册联动；卸载后由 WebUI 接管"
ui_print "- 内部日志：/data/adb/novasched/novasched.log"
ui_print "- 外部日志镜像：/sdcard/Android/NovaSched/log.txt（不可用时静默跳过）"
ui_print "- 没有内置 WebUI 的管理器：用 KsuWebUI 独立版打开本模块并授予 root"
ui_print "- 开机入口日志：/data/adb/novasched/boot-entry.log"
ui_print "- 覆盖安装后必须重启手机，以退出旧版进程并加载本版本"
