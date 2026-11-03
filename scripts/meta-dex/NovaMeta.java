// NovaMeta · app_process 元数据辅助（十七）。
// 由 scripts/build-meta-dex.sh 用 javac + d8 编译为 NovaMeta.dex；
// 运行：CLASSPATH=/data/adb/novasched/bin/NovaMeta.dex app_process /system/bin NovaMeta <pkg>...
// 输出：每行一个 JSON 对象（label/icon 为 UTF-8；icon 为 96px PNG 的 base64，
// 失败时为空串）。不做任何写操作；包名参数由调用方（Novasched native meta）
// 校验后传入。
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.PixelFormat;
import android.graphics.drawable.Drawable;
import android.util.Base64;

import java.io.ByteArrayOutputStream;
import java.util.LinkedHashMap;
import java.util.Map;

public class NovaMeta {
    private static final int ICON_SIZE = 96;

    public static void main(String[] args) {
        android.content.Context context = null;
        try {
            context = (android.content.Context) Class
                .forName("android.app.ActivityThread")
                .getMethod("currentApplication")
                .invoke(null);
        } catch (Throwable ignored) {
            // app_process 无应用上下文：用 ActivityThread.systemMain 的
            // 上下文兜底（隐藏路径，反射隔离在 try 内）。
        }
        if (context == null) {
            try {
                Object activityThread = Class.forName("android.app.ActivityThread")
                    .getMethod("systemMain").invoke(null);
                context = (android.content.Context) activityThread.getClass()
                    .getMethod("getSystemContext").invoke(activityThread);
            } catch (Throwable ignored) {
            }
        }
        if (context == null) {
            System.out.println("{\"error\":\"no context\"}");
            return;
        }
        android.content.pm.PackageManager pm = context.getPackageManager();
        for (String raw : args) {
            String pkg = raw.trim();
            if (pkg.isEmpty() || pkg.length() > 255 || !pkg.matches("[A-Za-z][A-Za-z0-9_]*(\\.[A-Za-z0-9_-]+)+")) {
                emit(pkg, "", "", false, false, "invalid package");
                continue;
            }
            try {
                android.content.pm.ApplicationInfo info = pm.getApplicationInfo(pkg, 0);
                String label = String.valueOf(pm.getApplicationLabel(info));
                String icon = encodeIcon(pm, info);
                boolean system = (info.flags & android.content.pm.ApplicationInfo.FLAG_SYSTEM) != 0;
                // ApplicationInfo.enabled：公开字段，反映用户在系统设置里的
                // 启用/停用状态（停用应用的图标与名称通常仍可读取）。
                emit(pkg, label, icon, system, info.enabled, "");
            } catch (Throwable error) {
                emit(pkg, "", "", false, false, String.valueOf(error.getMessage()));
            }
        }
    }

    private static void emit(String pkg, String label, String icon, boolean system, boolean enabled, String error) {
        // JSON 转义：仅输出 ASCII 安全形式，控制字符与引号必转义。
        StringBuilder json = new StringBuilder(128);
        json.append("{\"package\":\"").append(json(pkg)).append('"');
        json.append(",\"label\":\"").append(json(label)).append('"');
        json.append(",\"icon\":\"").append(icon).append('"');
        json.append(",\"system\":").append(system);
        json.append(",\"enabled\":").append(enabled);
        if (!error.isEmpty()) json.append(",\"error\":\"").append(json(error)).append('"');
        json.append("}");
        System.out.println(json);
    }

    private static String json(String value) {
        StringBuilder out = new StringBuilder(value.length() + 8);
        for (int i = 0; i < value.length(); i++) {
            char c = value.charAt(i);
            switch (c) {
                case '"': out.append("\\\""); break;
                case '\\': out.append("\\\\"); break;
                case '\n': out.append("\\n"); break;
                case '\r': out.append("\\r"); break;
                case '\t': out.append("\\t"); break;
                default:
                    if (c < 0x20) out.append(String.format("\\u%04x", (int) c));
                    else out.append(c);
            }
        }
        return out.toString();
    }

    /** Drawable 渲染为 96px 透明底 PNG：等比缩放居中，不裁切不变形。 */
    private static String encodeIcon(android.content.pm.PackageManager pm,
                                     android.content.pm.ApplicationInfo info) {
        try {
            Drawable drawable = info.loadIcon(pm);
            if (drawable == null) return "";
            int width = drawable.getIntrinsicWidth();
            int height = drawable.getIntrinsicHeight();
            if (width <= 0 || height <= 0) { width = height = ICON_SIZE; }
            double scale = Math.min((double) ICON_SIZE / width, (double) ICON_SIZE / height);
            int drawWidth = Math.max(1, (int) (width * scale));
            int drawHeight = Math.max(1, (int) (height * scale));
            int left = (ICON_SIZE - drawWidth) / 2;
            int top = (ICON_SIZE - drawHeight) / 2;
            Bitmap bitmap = Bitmap.createBitmap(ICON_SIZE, ICON_SIZE, Bitmap.Config.ARGB_8888);
            Canvas canvas = new Canvas(bitmap);
            drawable.setBounds(left, top, left + drawWidth, top + drawHeight);
            drawable.draw(canvas);
            ByteArrayOutputStream buffer = new ByteArrayOutputStream();
            bitmap.compress(Bitmap.CompressFormat.PNG, 100, buffer);
            bitmap.recycle();
            return Base64.encodeToString(buffer.toByteArray(), Base64.NO_WRAP);
        } catch (Throwable error) {
            return "";
        }
    }
}
