package io.github.dixonsolutions.lindroids;

import android.app.Notification;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.ApplicationInfo;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.util.Log;

import java.io.File;
import java.security.SecureRandom;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * Keeps the app alive while Linux windows may open, and owns every link to a server.
 *
 * A server listens on the abstract socket "lindroids.server.&lt;uid&gt;" of the sandbox app it runs
 * in (Termux, a proot-based terminal...). The CLI may announce it with a broadcast; otherwise this
 * service finds it by trying the sockets of approved and installed apps every few seconds.
 */
public final class LinkService extends Service {
    static final String TAG = "lindroids";
    static final int NOTE_RUNNING = 1;
    static final int NOTE_PAIR = 2;

    static final class Link {
        final int id, uid;
        final String socket;
        String sandbox;
        Link(int id, int uid, String socket, String sandbox) { this.id = id; this.uid = uid; this.socket = socket; this.sandbox = sandbox; }
    }

    static final class Win {
        int link, win;
        String title, appId;
        Bitmap icon;
        WindowActivity activity;
        boolean closedByServer;
    }

    static final Map<Integer, Link> links = new HashMap<>();
    static final Map<Long, Win> windows = new HashMap<>();
    static final Handler main = new Handler(Looper.getMainLooper());
    static final ExecutorService connector = Executors.newSingleThreadExecutor();
    static int nextLink = 1;
    static Context app;
    static LinkService instance;
    /** Uids we already told the user about (pairing needed), so the notification is not repeated. */
    static final Set<Integer> announcedPairing = new HashSet<>();
    static List<Integer> installedUids = new ArrayList<>();
    static long installedAt;

    static long key(int link, int win) { return ((long) link << 32) | (win & 0xffffffffL); }

    // ---------------------------------------------------------------- service lifecycle

    @Override public void onCreate() {
        super.onCreate();
        instance = this;
        app = getApplicationContext();
        startForeground(NOTE_RUNNING, buildNotification());
        main.postDelayed(scanner, 500);
    }

    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        updateNotification();
        return START_STICKY;
    }

    @Override public void onDestroy() {
        main.removeCallbacks(scanner);
        instance = null;
        super.onDestroy();
    }

    @Override public IBinder onBind(Intent intent) { return null; }

    static void ensureRunning(Context ctx) {
        app = ctx.getApplicationContext();
        Intent i = new Intent(app, LinkService.class);
        try {
            if (Build.VERSION.SDK_INT >= 26) app.startForegroundService(i); else app.startService(i);
        } catch (Throwable t) {
            Log.w(TAG, "cannot start the service now", t);
        }
    }

    Notification buildNotification() {
        int n = windows.size();
        String text = n == 0 ? "Ready for Linux apps" : (n == 1 ? "1 window open" : n + " windows open");
        if (!links.isEmpty()) {
            StringBuilder sb = new StringBuilder();
            for (Link l : links.values()) sb.append(sb.length() == 0 ? "" : ", ").append(l.sandbox);
            text += " · " + sb;
        }
        PendingIntent pi = PendingIntent.getActivity(this, 0, new Intent(this, HomeActivity.class),
                PendingIntent.FLAG_UPDATE_CURRENT | (Build.VERSION.SDK_INT >= 23 ? PendingIntent.FLAG_IMMUTABLE : 0));
        Notification.Builder b = Build.VERSION.SDK_INT >= 26
                ? new Notification.Builder(this, App.CHANNEL) : new Notification.Builder(this);
        return b.setSmallIcon(R.drawable.ic_stat).setContentTitle("Lindroids").setContentText(text)
                .setContentIntent(pi).setOngoing(true).build();
    }

    static void updateNotification() {
        if (instance != null) {
            NotificationManager nm = (NotificationManager) instance.getSystemService(NOTIFICATION_SERVICE);
            nm.notify(NOTE_RUNNING, instance.buildNotification());
        }
    }

    // ---------------------------------------------------------------- finding and pairing servers

    static SharedPreferences prefs() { return app.getSharedPreferences("lindroids", MODE_PRIVATE); }

    static boolean isAllowed(int uid) {
        return uid == android.os.Process.myUid() || prefs().getBoolean("allow." + uid, false);
    }

    static void setAllowed(int uid, boolean allowed) { prefs().edit().putBoolean("allow." + uid, allowed).apply(); }
    static String secretFor(int uid) { return prefs().getString("secret." + uid, null); }
    static String socketFor(int uid) { return "lindroids.server." + uid; }

    static String sandboxName(int uid, String hint) {
        if (hint != null && !hint.isEmpty()) return hint;
        String saved = prefs().getString("sandbox." + uid, null);
        if (saved != null) return saved;
        String[] pkgs = app.getPackageManager().getPackagesForUid(uid);
        if (pkgs != null && pkgs.length > 0) {
            try {
                return app.getPackageManager().getApplicationLabel(app.getPackageManager().getApplicationInfo(pkgs[0], 0)).toString();
            } catch (Exception ignored) {}
            return pkgs[0];
        }
        return "uid " + uid;
    }

    static boolean linked(int uid) {
        for (Link l : links.values()) if (l.uid == uid) return true;
        return false;
    }

    /** The CLI announced a server. */
    static void onHello(Context ctx, String socket, int uid, String sandbox, String secret) {
        ensureRunning(ctx);
        SharedPreferences.Editor e = prefs().edit();
        if (secret != null && !secret.isEmpty()) e.putString("secret." + uid, secret);
        if (sandbox != null) e.putString("sandbox." + uid, sandbox);
        e.apply();
        announcedPairing.remove(uid);
        connect(uid, socket, null, true);
    }

    /**
     * Try to link to uid's server. {@code code} is a pairing code the user typed, or null.
     * {@code interactive}: ask the user (approval / code) when needed, instead of staying quiet.
     */
    static void connect(final int uid, final String socket, final String code, final boolean interactive) {
        connector.execute(() -> {
            if (linked(uid)) return;
            final String sandbox = sandboxName(uid, null);
            String secret = secretFor(uid);
            // Already told the user this one needs pairing: wait for them.
            if (!interactive && code == null && announcedPairing.contains(uid) && (secret == null || !isAllowed(uid))) return;
            if (code == null && (secret == null || !isAllowed(uid))) {
                // Anything listening there? A handshake without credentials costs nothing.
                String probe = Native.linkConnect(-1, socket, uid, null, null);
                if (!probe.startsWith("denied")) return;
                if (interactive || !announcedPairing.contains(uid)) {
                    announcedPairing.add(uid);
                    final boolean needCode = secret == null;
                    main.post(() -> ApproveActivity.ask(app, uid, sandbox, needCode, interactive));
                }
                return;
            }
            int id;
            synchronized (links) { id = nextLink++; }
            String r = Native.linkConnect(id, socket, uid, code != null ? null : secret, code);
            Log.i(TAG, "link " + id + " to " + sandbox + " (uid " + uid + "): " + r.split(" ")[0]);
            if (r.startsWith("ok ") || r.startsWith("paired ")) {
                if (r.startsWith("paired ")) {
                    String[] parts = r.split(" ", 3);
                    prefs().edit().putString("secret." + uid, parts[2]).putBoolean("allow." + uid, true).apply();
                }
                final Link l = new Link(id, uid, socket, sandbox);
                main.post(() -> {
                    links.put(l.id, l);
                    ApproveActivity.dismissPairing(app, uid);
                    updateNotification();
                });
            } else if (r.startsWith("denied ")) {
                if (code != null) {
                    final boolean open = r.startsWith("denied true");
                    main.post(() -> ApproveActivity.wrongCode(app, uid, sandbox, open));
                } else {
                    needPairingAgain(uid, sandbox, interactive);
                }
            } else if (r.equals("wrong-server")) {
                // It does not know our secret: the sandbox was reset, or it is not the real server.
                // Either way, only a fresh pairing (with the code from the terminal) is safe.
                Log.w(TAG, "the server for uid " + uid + " could not prove it is ours");
                needPairingAgain(uid, sandbox, interactive);
            }
        });
    }

    static void needPairingAgain(int uid, String sandbox, boolean interactive) {
        prefs().edit().remove("secret." + uid).apply();
        if (!announcedPairing.contains(uid) || interactive) {
            announcedPairing.add(uid);
            main.post(() -> ApproveActivity.ask(app, uid, sandbox, true, interactive));
        }
    }

    /** Uids worth trying: approved sandboxes, plus every installed app (cheap: a failed connect). */
    static List<Integer> candidates() {
        long now = System.currentTimeMillis();
        if (now - installedAt > 30000) {
            ArrayList<Integer> uids = new ArrayList<>();
            for (ApplicationInfo ai : app.getPackageManager().getInstalledApplications(0)) {
                if ((ai.flags & ApplicationInfo.FLAG_SYSTEM) != 0) continue;
                if (ai.uid == android.os.Process.myUid() || uids.contains(ai.uid)) continue;
                uids.add(ai.uid);
            }
            installedUids = uids;
            installedAt = now;
        }
        return installedUids;
    }

    final Runnable scanner = new Runnable() {
        @Override public void run() {
            for (int uid : candidates()) {
                if (!linked(uid)) connect(uid, socketFor(uid), null, false);
            }
            main.postDelayed(this, 2500);
        }
    };

    // ---------------------------------------------------------------- calls from Rust (any thread)

    static void onLinked(int link, String sandbox) {
        // Called while linkConnect is still running; the Link entry is added right after.
        main.post(() -> {
            Link l = links.get(link);
            if (l != null && sandbox != null) l.sandbox = sandbox;
            updateNotification();
        });
    }

    static void onWindowNew(int link, int win, String title, String appId, byte[] icon) {
        main.post(() -> {
            Win w = new Win();
            w.link = link; w.win = win; w.title = title; w.appId = appId;
            if (icon != null && icon.length > 0) w.icon = BitmapFactory.decodeByteArray(icon, 0, icon.length);
            windows.put(key(link, win), w);
            Intent i = new Intent(app, WindowActivity.class);
            i.putExtra("link", link).putExtra("win", win);
            i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_NEW_DOCUMENT | Intent.FLAG_ACTIVITY_MULTIPLE_TASK);
            try {
                app.startActivity(i);
            } catch (Throwable t) {
                Log.e(TAG, "cannot open a window from the background; allow 'Display over other apps'", t);
            }
            LaunchActivity.windowOpened();
            updateNotification();
        });
    }

    static void onWindowTitle(int link, int win, String title) {
        main.post(() -> {
            Win w = windows.get(key(link, win));
            if (w == null) return;
            w.title = title;
            if (w.activity != null) w.activity.applyTaskDescription();
        });
    }

    static void onWindowClose(int link, int win) {
        main.post(() -> {
            Win w = windows.remove(key(link, win));
            if (w != null) {
                w.closedByServer = true;
                if (w.activity != null) w.activity.finishAndRemoveTask();
            }
            updateNotification();
        });
    }

    static void onKeyboard(int link, int win, boolean show) {
        main.post(() -> {
            Win w = windows.get(key(link, win));
            if (w != null && w.activity != null) w.activity.showKeyboard(show);
        });
    }

    static String lastClipboard;

    static void onClipboard(int link, String text) {
        main.post(() -> {
            lastClipboard = text;
            ClipboardManager cm = (ClipboardManager) app.getSystemService(CLIPBOARD_SERVICE);
            cm.setPrimaryClip(ClipData.newPlainText("Linux", text));
        });
    }

    static void onExport(int link, String json) { main.post(() -> ExportActivity.start(app, link, json)); }

    static void onPickFolder(int link, int request) { main.post(() -> FolderActivity.start(app, link, request)); }

    static void onUnexport(int link, String id) {
        main.post(() -> {
            boolean existed = new File(exportsDir(), ExportActivity.safe(id) + ".json").exists();
            removeExport(app, id);
            Native.linkExportResult(link, id, existed, existed ? "removed " + id : id + " was not exported");
        });
    }

    /** Forget an exported program: its bundle, its description and its shortcut. */
    static void removeExport(Context ctx, String id) {
        try {
            org.json.JSONObject launch = LaunchActivity.load(id).getJSONObject("launch");
            if (launch.getString("mode").equals("bundle")) {
                HomeActivity.deleteTree(new File(bundlesDir(), ExportActivity.safe(launch.getString("bundle"))));
            }
        } catch (Exception ignored) {}
        new File(exportsDir(), ExportActivity.safe(id) + ".json").delete();
        new File(exportsDir(), ExportActivity.safe(id) + ".png").delete();
        if (Build.VERSION.SDK_INT >= 26) {
            try {
                ctx.getSystemService(android.content.pm.ShortcutManager.class)
                        .disableShortcuts(java.util.Collections.singletonList("app-" + id), "Removed from Lindroids");
            } catch (Throwable ignored) {}
        }
    }

    static void onLinkClosed(int link) {
        main.post(() -> {
            links.remove(link);
            ArrayList<Long> gone = new ArrayList<>();
            for (Map.Entry<Long, Win> e : windows.entrySet()) if (e.getValue().link == link) gone.add(e.getKey());
            for (Long k : gone) {
                Win w = windows.remove(k);
                w.closedByServer = true;
                if (w.activity != null) w.activity.finishAndRemoveTask();
            }
            updateNotification();
        });
    }

    // ---------------------------------------------------------------- the in-app server (bundles)

    static File exportsDir() { File d = new File(app.getFilesDir(), "exports"); d.mkdirs(); return d; }
    static File bundlesDir() { File d = new File(app.getFilesDir(), "bundles"); d.mkdirs(); return d; }
    static File runtimeDir() { File d = new File(app.getFilesDir(), "run"); d.mkdirs(); return d; }

    private static Thread localServer;

    static String localSecret() {
        String s = prefs().getString("secret." + android.os.Process.myUid(), null);
        if (s == null) {
            byte[] b = new byte[32];
            new SecureRandom().nextBytes(b);
            s = android.util.Base64.encodeToString(b, android.util.Base64.NO_WRAP | android.util.Base64.URL_SAFE | android.util.Base64.NO_PADDING);
            prefs().edit().putString("secret." + android.os.Process.myUid(), s).apply();
        }
        return s;
    }

    /** Starts the server inside this app, for programs exported as bundles. */
    static synchronized void ensureLocalServer(Context ctx) {
        ensureRunning(ctx);
        if (localServer != null && localServer.isAlive()) return;
        final int me = android.os.Process.myUid();
        final String[] args = {
                "--runtime-dir", runtimeDir().getAbsolutePath(),
                "--sandbox", "Lindroids",
                "--secret", localSecret(),
                "--socket-name", socketFor(me),
        };
        localServer = new Thread(() -> {
            int code = Native.serverRun(args);
            Log.i(TAG, "in-app server stopped: " + code);
        }, "lindroids-local-server");
        localServer.start();
        main.postDelayed(() -> connect(me, socketFor(me), null, false), 300);
    }
}
