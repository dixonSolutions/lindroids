package io.github.dixonsolutions.lindroids;

import android.app.ActivityManager;
import android.app.Notification;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.drawable.Icon;
import android.net.Uri;
import android.os.Build;
import android.os.ParcelFileDescriptor;
import android.provider.Settings;
import android.util.Log;
import android.webkit.MimeTypeMap;

import java.io.File;

/**
 * What Linux programs ask of Android through the server's portal: open a URI or a file with an
 * Android app, show notifications, open picked documents. Rust decides (see {@code portal.rs});
 * this is the Android end of each request.
 */
final class Portal {
    private Portal() {}

    static final String TAG = LinkService.TAG;
    /** Notifications from Linux programs are posted under this tag, apart from the app's own. */
    static final String NOTE_TAG = "linux";

    // ---------------------------------------------------------------- opening

    /** ACTION_VIEW on a URI (browser, mail, maps, dialer...). Main thread. */
    static void openUri(Context ctx, int link, int request, String uri) {
        Intent view = new Intent(Intent.ACTION_VIEW, Uri.parse(uri)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        try {
            backgroundStartCheck(ctx);
            ctx.startActivity(view);
            Native.linkOpenResult(link, request, true, null);
        } catch (ActivityNotFoundException e) {
            Native.linkOpenResult(link, request, false, "no Android app opens " + uri);
        } catch (RuntimeException e) {
            Native.linkOpenResult(link, request, false, String.valueOf(e.getMessage()));
        }
    }

    /**
     * Offer a file (already written to the cache by Rust) to the Android apps that open its type,
     * through {@link FilesProvider} with a read grant. Main thread.
     */
    static void openFile(Context ctx, int link, int request, String path, String mime) {
        File f = new File(path);
        Uri uri = FilesProvider.uriFor(ctx, f);
        if (uri == null) {
            Native.linkOpenResult(link, request, false, "the file is not where it can be shared");
            return;
        }
        if (mime == null || mime.isEmpty()) mime = guessMime(f.getName());
        FilesProvider.setType(uri, mime);
        Intent view = new Intent(Intent.ACTION_VIEW).setDataAndType(uri, mime)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        // The chooser shows "no apps" itself, which the Linux side would never hear about.
        if (ctx.getPackageManager().queryIntentActivities(view, 0).isEmpty()) {
            Native.linkOpenResult(link, request, false, "no Android app opens " + mime);
            return;
        }
        // The chooser carries the grant over to the app the user picks.
        Intent chooser = Intent.createChooser(view, "Open " + f.getName() + " with")
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        try {
            backgroundStartCheck(ctx);
            ctx.startActivity(chooser);
            Native.linkOpenResult(link, request, true, null);
        } catch (RuntimeException e) {
            Native.linkOpenResult(link, request, false, String.valueOf(e.getMessage()));
        }
    }

    static String guessMime(String name) {
        int dot = name.lastIndexOf('.');
        if (dot >= 0) {
            String m = MimeTypeMap.getSingleton().getMimeTypeFromExtension(name.substring(dot + 1).toLowerCase(java.util.Locale.ROOT));
            if (m != null) return m;
        }
        return "application/octet-stream";
    }

    /**
     * Activities started from the background (no window of ours in front) are allowed on Android
     * 10+ only because the app may draw over other apps, the same permission Linux windows open
     * with; without it Android drops the start silently. Say so in the log.
     */
    static void backgroundStartCheck(Context ctx) {
        if (Build.VERSION.SDK_INT >= 29 && !Settings.canDrawOverlays(ctx)) {
            Log.w(TAG, "starting an activity without 'Display over other apps': Android may block it");
        }
    }

    /**
     * A picked document (content URI) opened for Rust, which owns the returned fd. Returns the fd
     * as a string, or "!" and why it could not be opened. Any thread.
     */
    static String openDocument(Context ctx, String uri, String mode) {
        try {
            Uri u = Uri.parse(uri);
            if (!"content".equals(u.getScheme())) return "!not a content URI";
            ParcelFileDescriptor pfd;
            try {
                pfd = ctx.getContentResolver().openFileDescriptor(u, mode);
            } catch (IllegalArgumentException | UnsupportedOperationException e) {
                // Some providers know "w" but not "wt" (it truncates on all of them that matter).
                if (!mode.equals("wt")) throw e;
                pfd = ctx.getContentResolver().openFileDescriptor(u, "w");
            }
            if (pfd == null) return "!the provider gave nothing";
            return Integer.toString(pfd.detachFd());
        } catch (Exception e) {
            return "!" + e.getMessage();
        }
    }

    // ---------------------------------------------------------------- notifications

    /**
     * Show (or replace: same {@code note}) a notification. {@code actions} is key, label, key,
     * label... Taps, buttons and dismissals go to {@link NoteReceiver}. Main thread.
     */
    @SuppressWarnings("deprecation")
    static void notify(Context ctx, int note, String channel, String title, String body, String app,
                       byte[] icon, String[] actions, boolean urgent) {
        NotificationManager nm = ctx.getSystemService(NotificationManager.class);
        boolean allowed = nm != null && nm.areNotificationsEnabled()
                && (Build.VERSION.SDK_INT < 33
                    || ctx.checkSelfPermission("android.permission.POST_NOTIFICATIONS") == PackageManager.PERMISSION_GRANTED);
        if (!allowed) {
            Log.w(TAG, "notification from " + app + " dropped: notifications are not allowed for Lindroids");
            return;
        }
        Notification.Builder b = Build.VERSION.SDK_INT >= 26
                ? new Notification.Builder(ctx, channel) : new Notification.Builder(ctx);
        b.setSmallIcon(R.drawable.ic_stat).setContentTitle(title).setAutoCancel(true)
                .setContentIntent(pending(ctx, note, "default"))
                .setDeleteIntent(pending(ctx, note, null));
        if (body != null && !body.isEmpty()) b.setContentText(body).setStyle(new Notification.BigTextStyle().bigText(body));
        if (app != null && !app.isEmpty()) b.setSubText(app);
        if (icon != null && icon.length > 0) {
            Bitmap bm = BitmapFactory.decodeByteArray(icon, 0, icon.length);
            if (bm != null) b.setLargeIcon(bm);
        }
        if (actions != null) {
            Icon none = Icon.createWithResource(ctx, R.drawable.ic_stat);
            for (int i = 0; i + 1 < actions.length; i += 2) {
                b.addAction(new Notification.Action.Builder(none, actions[i + 1], pending(ctx, note, actions[i])).build());
            }
        }
        // Before channels (Android 7), importance is the notification's own priority.
        if (Build.VERSION.SDK_INT < 26) {
            b.setPriority(urgent ? Notification.PRIORITY_HIGH : Notification.PRIORITY_DEFAULT);
            if (urgent) b.setDefaults(Notification.DEFAULT_ALL);
        }
        nm.notify(NOTE_TAG, note, b.build());
    }

    static void cancel(Context ctx, int note) {
        NotificationManager nm = ctx.getSystemService(NotificationManager.class);
        if (nm != null) nm.cancel(NOTE_TAG, note);
    }

    /** {@code action} null: the deleteIntent (dismissed). Each (note, action) is its own intent. */
    static PendingIntent pending(Context ctx, int note, String action) {
        Intent i = new Intent(ctx, NoteReceiver.class)
                .setData(Uri.fromParts("lindroids-note", note + "/" + (action == null ? "" : "a/" + action), null))
                .putExtra(NoteReceiver.EXTRA_NOTE, note);
        if (action != null) i.putExtra(NoteReceiver.EXTRA_ACTION, action);
        return PendingIntent.getBroadcast(ctx, note, i,
                PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
    }

    /** Bring the link's most recently focused Linux window to the front, if it has one. Main thread. */
    @SuppressWarnings("deprecation")
    static void bringToFront(Context ctx, int link) {
        LinkService.Win best = null;
        for (LinkService.Win w : LinkService.windows.values()) {
            if (w.link != link || w.activity == null) continue;
            if (best == null || w.focusedAt > best.focusedAt) best = w;
        }
        if (best == null) return;
        int task = best.activity.getTaskId();
        ActivityManager am = ctx.getSystemService(ActivityManager.class);
        if (am == null) return;
        try {
            for (ActivityManager.AppTask t : am.getAppTasks()) {
                ActivityManager.RecentTaskInfo info = t.getTaskInfo();
                int id = Build.VERSION.SDK_INT >= 29 ? info.taskId : info.id;
                if (id == task) { t.moveToFront(); return; }
            }
        } catch (RuntimeException e) {
            Log.w(TAG, "cannot bring the window to the front", e);
        }
    }
}
