#!/system/bin/sh

SKIPUNZIP=0

ui_print "- NovaSched Zen Edition v1.5.0"
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

# ---------- ASoulOpt 交互安装：音量键选择预设 ----------
NOVA_KEY_TIMEOUT=6

asoul_key() {
  # 等待一次音量键：输出 UP / DOWN，超时输出空。
  deadline=$(( $(date +%s) + NOVA_KEY_TIMEOUT ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    event=$(timeout 1 getevent -qlc 1 /dev/input 2>/dev/null | awk 'NF>=3 {print $3; exit}')
    case "$event" in
      KEY_VOLUMEUP) echo UP; return 0 ;;
      KEY_VOLUMEDOWN) echo DOWN; return 0 ;;
    esac
  done
  return 1
}

if [ -f "$NOVA_STATE/options.txt" ]; then
  # 覆盖安装保留已有选择，只为旧格式补上新增开关。
  if ! grep -q '^extreme_powersave=' "$NOVA_STATE/options.txt"; then
    printf '\nextreme_powersave=0\n' >> "$NOVA_STATE/options.txt" || abort "! 无法写入极限节能选项"
  fi
  if ! grep -q '^smooth_powersave=' "$NOVA_STATE/options.txt"; then
    printf '\nsmooth_powersave=0\n' >> "$NOVA_STATE/options.txt" || abort "! 无法写入流畅省电选项"
  fi
  ui_print "- 检测到已有 ASoulOpt 配置，保留你的选择（重装本模块可重新配置）"
else
  ui_print " "
  ui_print "  ╔══════════════════════════════╗"
  ui_print "  ║  ASoulOpt · 交互式安装向导   ║"
  ui_print "  ╚══════════════════════════════╝"
  if ! command -v getevent >/dev/null 2>&1 || ! command -v timeout >/dev/null 2>&1; then
    ui_print "- 当前环境不支持按键交互，使用均衡预设"
    preset_mode=balance
    extreme=0
  else
    idx=2
    while :; do
      case $idx in
        1) name=省电 ;;
        2) name=均衡 ;;
        3) name=性能 ;;
        4) name=极速（游戏线程） ;;
      esac
      ui_print "  当前预设：$name"
      ui_print "- 音量上=切换  音量下=确认（${NOVA_KEY_TIMEOUT}s 内无操作=当前）"
      key=$(asoul_key)
      if [ -z "$key" ]; then
        ui_print "  未检测到按键，使用当前预设"
        break
      fi
      if [ "$key" = UP ]; then idx=$(( idx % 4 + 1 )); else break; fi
    done
    case $idx in
      1) preset_mode=powersave ;;
      2) preset_mode=balance ;;
      3) preset_mode=performance ;;
      4) preset_mode=fast ;;
    esac
    extreme=0
    if [ "$preset_mode" = powersave ]; then
      ui_print "  省电预设进阶：是否同时开启极限节能？"
      ui_print "- 音量上=开启  音量下=跳过（${NOVA_KEY_TIMEOUT}s 内）"
      key=$(asoul_key)
      if [ "$key" = UP ]; then
        extreme=1
        ui_print "- 极限节能将在省电档生效"
      fi
    fi
  fi
  case $preset_mode in
    powersave) preset_name=省电 ;;
    performance) preset_name=性能 ;;
    fast) preset_name=极速 ;;
    *) preset_name=均衡 ;;
  esac
  printf '# NovaSched Zen Edition（ASoulOpt 交互安装）\nextreme_powersave=%s\nsmooth_powersave=0\n' "$extreme" > "$NOVA_STATE/options.txt" || abort "! 无法初始化选项"
  printf '%s\n' "$preset_mode" > "$NOVA_STATE/mode.txt" || abort "! 无法写入默认档位"
  set_perm "$NOVA_STATE/options.txt" 0 0 0664
  set_perm "$NOVA_STATE/mode.txt" 0 0 0664
  ui_print "- 已选择：$preset_name 预设（默认档位已写入）"
  if [ "$preset_mode" = fast ]; then
    ui_print "- ASoulOpt 游戏线程：突频+满频上限已就绪"
    ui_print "- 建议在 WebUI→应用 为常玩游戏配置专属规则"
  fi
fi
set_perm "$NOVA_STATE/options.txt" 0 0 0664
ui_print "- 极限节能/流畅省电随预设写入；覆盖安装保留已有选择"

ui_print "- 安装完成；首次启动将自动保存原厂节点快照"
ui_print "- 检测到已安装 Scene 时自动注册联动；卸载后由 WebUI 接管"
ui_print "- 内部日志：/data/adb/novasched/novasched.log"
ui_print "- 外部日志镜像：/sdcard/Android/NovaSched/log.txt（不可用时静默跳过）"
ui_print "- 没有内置 WebUI 的管理器：用 KsuWebUI 独立版打开本模块并授予 root"
ui_print "- 开机入口日志：/data/adb/novasched/boot-entry.log"
ui_print "- 覆盖安装后必须重启手机，以退出旧版进程并加载本版本"
