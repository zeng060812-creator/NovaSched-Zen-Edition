#!/system/bin/sh
MODDIR=${0%/*}
umask 077
export PATH="/system/bin:/system/xbin:/data/adb/ksu/bin:/data/adb/ap/bin:/data/adb/magisk:$PATH"
# Leave evidence even when ELF startup fails before Rust can create an identity.
NOVA_STATE=/data/adb/novasched
mkdir -p "$NOVA_STATE" || exit 1
exec >>"$NOVA_STATE/boot-entry.log" 2>&1 || exit 1
printf '%s service.sh entered; waiting for boot completion\n' "$(date '+%Y-%m-%d %H:%M:%S')"
until [ "$(getprop sys.boot_completed)" = "1" ]; do
  sleep 2
done
"$MODDIR/bin/novasched" start --module-dir "$MODDIR"
NOVA_EXIT=$?
printf '%s start exited: %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$NOVA_EXIT"
exit "$NOVA_EXIT"
