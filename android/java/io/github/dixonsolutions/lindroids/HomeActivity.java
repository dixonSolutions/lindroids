package io.github.dixonsolutions.lindroids;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.Intent;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Color;
import android.graphics.Typeface;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Environment;
import android.provider.Settings;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.View;
import android.widget.Button;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

import org.json.JSONObject;

import java.io.File;

/** Setup and status: permissions, connected sandboxes, exported apps. */
public final class HomeActivity extends Activity {
    LinearLayout col;

    @Override protected void onCreate(Bundle b) {
        super.onCreate(b);
        LinkService.app = getApplicationContext();
        LinkService.ensureRunning(this);
    }

    /** Sandboxes connect and windows open while this screen is up: keep it current. */
    final Runnable refresh = new Runnable() {
        @Override public void run() {
            build();
            LinkService.main.postDelayed(this, 2000);
        }
    };

    @Override protected void onResume() {
        super.onResume();
        refresh.run();
    }

    @Override protected void onPause() {
        LinkService.main.removeCallbacks(refresh);
        super.onPause();
    }

    int dp(int v) { return (int) (v * getResources().getDisplayMetrics().density + 0.5f); }

    TextView text(String s, int sp, boolean bold) {
        TextView t = new TextView(this);
        t.setText(s);
        t.setTextSize(TypedValue.COMPLEX_UNIT_SP, sp);
        t.setTextColor(Color.WHITE);
        if (bold) t.setTypeface(Typeface.DEFAULT_BOLD);
        t.setPadding(0, dp(6), 0, dp(6));
        col.addView(t);
        return t;
    }

    void button(String label, View.OnClickListener l) {
        Button btn = new Button(this);
        btn.setText(label);
        btn.setAllCaps(false);
        btn.setOnClickListener(l);
        col.addView(btn);
    }

    ScrollView shown;

    void build() {
        final int scrollY = shown == null ? 0 : shown.getScrollY();
        ScrollView sv = new ScrollView(this);
        sv.setBackgroundColor(0xff101418);
        col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        col.setPadding(dp(20), dp(24), dp(20), dp(24));
        sv.addView(col);

        text("Lindroids", 28, true);
        text("Linux apps in Android windows.", 15, false);

        int open = LinkService.links.size();
        text(open == 0 ? "No sandbox connected right now." :
                open + " sandbox" + (open > 1 ? "es" : "") + " connected, " + LinkService.windows.size() + " window(s) open.", 15, false);

        for (final int uid : new java.util.ArrayList<>(LinkService.announcedPairing)) {
            if (LinkService.linked(uid)) continue;
            final String name = LinkService.sandboxName(uid, null);
            button("Pair with " + name + " (enter the code lindroids shows)",
                    v -> ApproveActivity.ask(this, uid, name, true, true));
        }

        text("Permissions", 20, true);
        boolean overlay = Build.VERSION.SDK_INT < 23 || Settings.canDrawOverlays(this);
        perm("Open windows from the background", overlay, () -> startActivity(
                new Intent(Settings.ACTION_MANAGE_OVERLAY_PERMISSION, Uri.parse("package:" + getPackageName()))));
        boolean files = Build.VERSION.SDK_INT >= 30 ? Environment.isExternalStorageManager()
                : checkSelfPermission(android.Manifest.permission.WRITE_EXTERNAL_STORAGE) == 0;
        perm("Files and folders (for exported apps)", files, () -> {
            if (Build.VERSION.SDK_INT >= 30) {
                startActivity(new Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
                        Uri.parse("package:" + getPackageName())));
            } else {
                requestPermissions(new String[]{android.Manifest.permission.READ_EXTERNAL_STORAGE,
                        android.Manifest.permission.WRITE_EXTERNAL_STORAGE}, 2);
            }
        });
        if (Build.VERSION.SDK_INT >= 33) {
            perm("Notifications (keeps windows alive)", checkSelfPermission("android.permission.POST_NOTIFICATIONS") == 0,
                    () -> requestPermissions(new String[]{"android.permission.POST_NOTIFICATIONS"}, 3));
        }
        if (isInstalled("com.termux")) {
            perm("Start Termux from shortcuts", checkSelfPermission("com.termux.permission.RUN_COMMAND") == 0,
                    () -> requestPermissions(new String[]{"com.termux.permission.RUN_COMMAND"}, 4));
        }

        text("Exported apps", 20, true);
        File[] specs = LinkService.exportsDir().listFiles((d, n) -> n.endsWith(".json"));
        if (specs == null || specs.length == 0) {
            text("None yet. In your terminal: lindroids export <command>", 14, false);
        } else {
            for (File f : specs) exported(f);
        }

        text("Get started", 20, true);
        TextView how = text("In Termux:\n\n"
                + "  curl -fsSL https://dixonsolutions.github.io/lindroids/install.sh | sh\n"
                + "  lindroids run <command>\n"
                + "  lindroids export <command>\n\n"
                + "Other terminals (proot Debian/Ubuntu, UserLAnd, Andronix): the same install line.", 13, false);
        how.setTypeface(Typeface.MONOSPACE);
        how.setTextIsSelectable(true);
        text("Version " + version(), 12, false).setAlpha(0.6f);
        setContentView(sv);
        shown = sv;
        sv.post(() -> sv.scrollTo(0, scrollY));
    }

    String version() {
        try { return getPackageManager().getPackageInfo(getPackageName(), 0).versionName; } catch (Exception e) { return "?"; }
    }

    boolean isInstalled(String pkg) {
        try { getPackageManager().getPackageInfo(pkg, 0); return true; } catch (Exception e) { return false; }
    }

    void perm(String what, boolean granted, Runnable ask) {
        if (granted) text("✓ " + what, 15, false);
        else button("Allow: " + what, v -> ask.run());
    }

    void exported(File spec) {
        final String id = spec.getName().substring(0, spec.getName().length() - 5);
        String name = id;
        try { name = LaunchActivity.load(id).getString("name"); } catch (Exception ignored) {}
        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(0, dp(6), 0, dp(6));
        ImageView icon = new ImageView(this);
        Bitmap bmp = BitmapFactory.decodeFile(new File(LinkService.exportsDir(), id + ".png").getPath());
        if (bmp != null) icon.setImageBitmap(bmp);
        row.addView(icon, new LinearLayout.LayoutParams(dp(40), dp(40)));
        TextView label = new TextView(this);
        label.setText(name);
        label.setTextColor(Color.WHITE);
        label.setTextSize(TypedValue.COMPLEX_UNIT_SP, 16);
        label.setPadding(dp(12), 0, 0, 0);
        row.addView(label, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1));
        final String shown = name;
        row.setOnClickListener(v -> {
            try { LaunchActivity.launch(this, id); } catch (Exception e) { text("Could not start: " + e.getMessage(), 14, false); }
        });
        row.setOnLongClickListener(v -> {
            new AlertDialog.Builder(this).setTitle(shown)
                    .setItems(new CharSequence[]{"Add to home screen", "Remove"}, (d, which) -> {
                        if (which == 0) {
                            if (bmp != null) ExportActivity.pin(this, id, shown, bmp);
                        } else {
                            remove(id);
                            build();
                        }
                    }).show();
            return true;
        });
        col.addView(row);
    }

    void remove(String id) {
        LinkService.removeExport(this, id);
    }

    static void deleteTree(File f) {
        // Do not follow symlinks out of the bundle.
        if (f.isDirectory() && !isSymlink(f)) {
            File[] kids = f.listFiles();
            if (kids != null) for (File k : kids) deleteTree(k);
        }
        f.delete();
    }

    static boolean isSymlink(File f) {
        try {
            return android.system.OsConstants.S_ISLNK(android.system.Os.lstat(f.getPath()).st_mode);
        } catch (Exception e) {
            return true;
        }
    }
}
