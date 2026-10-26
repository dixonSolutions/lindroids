package io.github.dixonsolutions.lindroids;

import android.app.Activity;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ShortcutInfo;
import android.content.pm.ShortcutManager;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.drawable.Icon;
import android.os.Build;
import android.os.Bundle;
import android.util.Base64;
import android.util.Log;

import org.json.JSONObject;

import java.io.File;
import java.io.FileOutputStream;

/**
 * Pins a home screen shortcut for an exported Linux program. The program's description is kept in
 * the app, so the shortcut works with the terminal closed.
 */
public final class ExportActivity extends Activity {
    static void start(Context ctx, int link, String json) {
        Intent i = new Intent(ctx, ExportActivity.class).putExtra("link", link).putExtra("json", json);
        i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        try {
            ctx.startActivity(i);
        } catch (Throwable t) {
            Native.linkExportResult(link, "", false, "Lindroids could not come to the front: " + t);
        }
    }

    @Override protected void onCreate(Bundle b) {
        super.onCreate(b);
        int link = getIntent().getIntExtra("link", -1);
        String id = "";
        try {
            JSONObject req = new JSONObject(getIntent().getStringExtra("json"));
            id = req.getString("id");
            String name = req.getString("name");
            byte[] png = Base64.decode(req.getString("icon_png_b64"), Base64.DEFAULT);
            save(id, req, png);
            Bitmap icon = BitmapFactory.decodeByteArray(png, 0, png.length);
            String msg = pin(this, id, name, icon);
            Native.linkExportResult(link, id, true, msg);
        } catch (Throwable t) {
            Log.e(LinkService.TAG, "export failed", t);
            Native.linkExportResult(link, id, false, String.valueOf(t.getMessage()));
        }
        finish();
    }

    static void save(String id, JSONObject req, byte[] png) throws Exception {
        File dir = LinkService.exportsDir();
        try (FileOutputStream o = new FileOutputStream(new File(dir, safe(id) + ".json"))) {
            o.write(req.toString(2).getBytes("UTF-8"));
        }
        try (FileOutputStream o = new FileOutputStream(new File(dir, safe(id) + ".png"))) {
            o.write(png);
        }
    }

    static String safe(String id) { return id.replaceAll("[^A-Za-z0-9._-]", "_"); }

    static Intent launchIntent(Context ctx, String id) {
        Intent i = new Intent(Intent.ACTION_MAIN);
        i.setClassName(ctx.getPackageName(), LaunchActivity.class.getName());
        i.putExtra("id", id);
        i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TASK);
        return i;
    }

    /**
     * The program's icon as an adaptive icon: drawn into the inner safe zone of a 108dp canvas over
     * a light background, so launchers mask it into their shape instead of shrinking it into a
     * white disc.
     */
    static Icon adaptive(Bitmap icon) {
        if (Build.VERSION.SDK_INT < 26) return Icon.createWithBitmap(icon);
        int size = 432;
        Bitmap out = Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888);
        android.graphics.Canvas c = new android.graphics.Canvas(out);
        c.drawColor(0xfff4f6f8);
        int inner = (int) (size * 0.58);
        int off = (size - inner) / 2;
        android.graphics.Paint p = new android.graphics.Paint(android.graphics.Paint.FILTER_BITMAP_FLAG | android.graphics.Paint.ANTI_ALIAS_FLAG);
        c.drawBitmap(icon, null, new android.graphics.Rect(off, off, off + inner, off + inner), p);
        return Icon.createWithAdaptiveBitmap(out);
    }

    /** Returns a description of what happened, for the terminal. */
    static String pin(Context ctx, String id, String name, Bitmap icon) {
        Intent launch = launchIntent(ctx, id);
        if (Build.VERSION.SDK_INT >= 26) {
            ShortcutManager sm = ctx.getSystemService(ShortcutManager.class);
            ShortcutInfo info = new ShortcutInfo.Builder(ctx, "app-" + id)
                    .setShortLabel(name)
                    .setLongLabel(name)
                    .setIcon(adaptive(icon))
                    .setIntent(launch)
                    .build();
            // Pinned before: refresh it (icon, name) instead of adding a second one.
            try {
                for (ShortcutInfo s : sm.getPinnedShortcuts()) {
                    if (s.getId().equals(info.getId())) {
                        sm.enableShortcuts(java.util.Collections.singletonList(info.getId()));
                        sm.updateShortcuts(java.util.Collections.singletonList(info));
                        return "updated the \"" + name + "\" shortcut on the home screen";
                    }
                }
            } catch (Throwable ignored) {}
            if (sm.isRequestPinShortcutSupported()) {
                sm.requestPinShortcut(info, null);
                return "asked the launcher to add \"" + name + "\" to the home screen";
            }
        }
        // Android 7.x, or a launcher without pinning support: the legacy broadcast.
        Intent add = new Intent("com.android.launcher.action.INSTALL_SHORTCUT");
        add.putExtra(Intent.EXTRA_SHORTCUT_INTENT, launch);
        add.putExtra(Intent.EXTRA_SHORTCUT_NAME, name);
        add.putExtra(Intent.EXTRA_SHORTCUT_ICON, icon);
        add.putExtra("duplicate", false);
        ctx.sendBroadcast(add);
        return "sent \"" + name + "\" to the launcher (legacy shortcut)";
    }
}
