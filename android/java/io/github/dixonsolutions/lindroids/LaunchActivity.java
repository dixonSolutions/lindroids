package io.github.dixonsolutions.lindroids;

import android.app.Activity;
import android.content.Intent;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Color;
import android.os.Build;
import android.os.Bundle;
import android.util.Log;
import android.util.TypedValue;
import android.view.Gravity;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.TextView;
import android.widget.Toast;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.File;
import java.io.FileInputStream;

/**
 * Where a home screen shortcut lands. Starts the exported program and stays up as a splash until
 * its first window opens: a visible activity is what lets Android open that window right away.
 */
public final class LaunchActivity extends Activity {
    /** The splash waiting for a window, if any. */
    static LaunchActivity waiting;

    @Override protected void onCreate(Bundle b) {
        super.onCreate(b);
        LinkService.app = getApplicationContext();
        String id = getIntent().getStringExtra("id");
        String name = id;
        try { name = load(id).getString("name"); } catch (Exception ignored) {}
        splash(id, name);
        try {
            launch(this, id);
            waiting = this;
            // Give up waiting after a while (the program may have no window, or failed).
            LinkService.main.postDelayed(() -> { if (waiting == this) finish(); }, 60000);
        } catch (Throwable t) {
            Log.e(LinkService.TAG, "launch " + id + " failed", t);
            Toast.makeText(this, "Lindroids: " + t.getMessage(), Toast.LENGTH_LONG).show();
            finish();
        }
    }

    @Override protected void onDestroy() {
        if (waiting == this) waiting = null;
        super.onDestroy();
    }

    /** Called when any Linux window opens. */
    static void windowOpened() {
        LaunchActivity a = waiting;
        waiting = null;
        if (a != null) LinkService.main.postDelayed(a::finish, 400);
    }

    void splash(String id, String name) {
        float d = getResources().getDisplayMetrics().density;
        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        col.setGravity(Gravity.CENTER);
        col.setBackgroundColor(0xff0f1720);
        ImageView icon = new ImageView(this);
        Bitmap bmp = BitmapFactory.decodeFile(new File(LinkService.exportsDir(), ExportActivity.safe(id) + ".png").getPath());
        if (bmp != null) icon.setImageBitmap(bmp);
        col.addView(icon, new LinearLayout.LayoutParams((int) (96 * d), (int) (96 * d)));
        TextView t = new TextView(this);
        t.setText(name);
        t.setTextColor(Color.WHITE);
        t.setTextSize(TypedValue.COMPLEX_UNIT_SP, 20);
        t.setPadding(0, (int) (16 * d), 0, (int) (16 * d));
        col.addView(t);
        col.addView(new ProgressBar(this));
        setContentView(col);
    }

    static JSONObject load(String id) throws Exception {
        File f = new File(LinkService.exportsDir(), ExportActivity.safe(id) + ".json");
        if (!f.exists()) throw new IllegalStateException(id + " is no longer exported");
        byte[] buf = new byte[(int) f.length()];
        try (FileInputStream in = new FileInputStream(f)) {
            int off = 0;
            while (off < buf.length) off += in.read(buf, off, buf.length - off);
        }
        return new JSONObject(new String(buf, "UTF-8"));
    }

    static String[] strings(JSONArray a) throws Exception {
        String[] s = new String[a.length()];
        for (int i = 0; i < s.length; i++) s[i] = a.getString(i);
        return s;
    }

    static void launch(Activity ctx, String id) throws Exception {
        JSONObject spec = load(id).getJSONObject("launch");
        String mode = spec.getString("mode");
        LinkService.ensureRunning(ctx);
        if (mode.equals("termux")) {
            String[] argv = strings(spec.getJSONArray("argv"));
            if (ctx.checkSelfPermission("com.termux.permission.RUN_COMMAND") != 0) {
                ctx.requestPermissions(new String[]{"com.termux.permission.RUN_COMMAND"}, 1);
                throw new IllegalStateException("allow Lindroids to run commands in Termux, then tap again");
            }
            Intent i = new Intent("com.termux.RUN_COMMAND");
            i.setClassName("com.termux", "com.termux.app.RunCommandService");
            i.putExtra("com.termux.RUN_COMMAND_PATH", argv[0]);
            String[] rest = new String[argv.length - 1];
            System.arraycopy(argv, 1, rest, 0, rest.length);
            i.putExtra("com.termux.RUN_COMMAND_ARGUMENTS", rest);
            i.putExtra("com.termux.RUN_COMMAND_BACKGROUND", true);
            if (Build.VERSION.SDK_INT >= 26) ctx.startForegroundService(i); else ctx.startService(i);
            return;
        }
        // Bundle: run it right here, under the app's own proot and server.
        final String bundle = spec.getString("bundle");
        final String[] argv = strings(spec.getJSONArray("argv"));
        JSONArray envArr = spec.optJSONArray("env");
        final String[] env = new String[envArr == null ? 0 : envArr.length()];
        for (int i = 0; i < env.length; i++) {
            JSONArray kv = envArr.getJSONArray(i);
            env[i] = kv.getString(0) + "=" + kv.getString(1);
        }
        final File bundleDir = new File(LinkService.bundlesDir(), ExportActivity.safe(bundle));
        if (!new File(bundleDir, "manifest.json").exists()) throw new IllegalStateException(bundle + " was not copied into Lindroids");
        LinkService.ensureLocalServer(ctx.getApplicationContext());
        final String libDir = ctx.getApplicationInfo().nativeLibraryDir;
        final File log = new File(LinkService.runtimeDir(), ExportActivity.safe(id) + ".log");
        new Thread(() -> {
            int pid = Native.bundleSpawn(libDir, bundleDir.getAbsolutePath(), LinkService.runtimeDir().getAbsolutePath(),
                    argv, env, log.getAbsolutePath());
            Log.i(LinkService.TAG, "started " + id + " as pid " + pid + " (log " + log + ")");
        }, "lindroids-spawn").start();
    }
}
