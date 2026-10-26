package io.github.dixonsolutions.lindroids;

import android.app.Activity;
import android.content.Intent;
import android.os.Build;
import android.os.Bundle;
import android.util.Log;
import android.widget.Toast;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.File;
import java.io.FileInputStream;

/** Where a home screen shortcut lands. Starts the exported program, then gets out of the way. */
public final class LaunchActivity extends Activity {
    @Override protected void onCreate(Bundle b) {
        super.onCreate(b);
        String id = getIntent().getStringExtra("id");
        try {
            launch(this, id);
        } catch (Throwable t) {
            Log.e(LinkService.TAG, "launch " + id + " failed", t);
            Toast.makeText(this, "Lindroids: " + t.getMessage(), Toast.LENGTH_LONG).show();
        }
        finish();
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
        if (mode.equals("termux")) {
            String[] argv = strings(spec.getJSONArray("argv"));
            Intent i = new Intent("com.termux.RUN_COMMAND");
            i.setClassName("com.termux", "com.termux.app.RunCommandService");
            i.putExtra("com.termux.RUN_COMMAND_PATH", argv[0]);
            String[] rest = new String[argv.length - 1];
            System.arraycopy(argv, 1, rest, 0, rest.length);
            i.putExtra("com.termux.RUN_COMMAND_ARGUMENTS", rest);
            i.putExtra("com.termux.RUN_COMMAND_BACKGROUND", true);
            i.putExtra("com.termux.RUN_COMMAND_SESSION_ACTION", "0");
            if (ctx.checkSelfPermission("com.termux.permission.RUN_COMMAND") != 0) {
                ctx.requestPermissions(new String[]{"com.termux.permission.RUN_COMMAND"}, 1);
                throw new IllegalStateException("allow Lindroids to run commands in Termux, then tap again");
            }
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
        LinkService.ensureLocalServer(ctx.getApplicationContext());
        final String libDir = ctx.getApplicationInfo().nativeLibraryDir;
        final File bundleDir = new File(LinkService.bundlesDir(), ExportActivity.safe(bundle));
        final File log = new File(LinkService.runtimeDir(), ExportActivity.safe(id) + ".log");
        new Thread(() -> {
            int pid = Native.bundleSpawn(libDir, bundleDir.getAbsolutePath(), LinkService.runtimeDir().getAbsolutePath(),
                    argv, env, log.getAbsolutePath());
            Log.i(LinkService.TAG, "started " + id + " as pid " + pid);
        }, "lindroids-spawn").start();
    }
}
