#!/system/bin/sh
# NovaSched_Zen_Edition_Scene_Provider
NOVA_MODULE=@NOVASCHED_MODULE_DIR@
exec "$NOVA_MODULE/bin/novasched" scene-mode "$1" --module-dir "$NOVA_MODULE"
