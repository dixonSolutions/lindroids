package io.github.dixonsolutions.lindroids;

import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.Build;
import android.os.SystemClock;
import android.util.Log;

import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;

/**
 * Pairing with Termux without a code: the app asks Termux, through its RUN_COMMAND API, to run
 * `lindroids app-secret`, and Termux hands the output back through a PendingIntent to the
 * non-exported {@link PairResultReceiver}. Only Termux can answer there, so the secret is the real
 * one; the server then proves it knows it as usual (see linkConnect).
 *
 * Needs the "Run commands in Termux environment" permission (asked for once, by
 * {@link ApproveActivity}) and `allow-external-apps = true` in ~/.termux/termux.properties (set
 * by install.sh). Without either, pairing falls back to the code.
 */
final class TermuxPairing {
    static final String TERMUX = "com.termux";
    static final String PERMISSION = "com.termux.permission.RUN_COMMAND";
    /** A request per uid at most this often: the scanner calls connect() every few seconds. */
    static final long RETRY_MS = 20000;
    static final Map<Integer, Long> asked = new HashMap<>();
    /** Uids for which this did not work (refused, denied, or the secret did not fit the server). */
    static final Set<Integer> refused = new HashSet<>();
    /** Uids whose current secret came from Termux. */
    static final Set<Integer> gave = new HashSet<>();

    private TermuxPairing() {}

    /** Whether uid is Termux (or a plugin sharing its uid), whose server this can pair with. */
    static boolean isTermux(Context ctx, int uid) {
        String[] pkgs = ctx.getPackageManager().getPackagesForUid(uid);
        if (pkgs == null) return false;
        for (String p : pkgs) if (TERMUX.equals(p)) return true;
        return false;
    }

    static boolean permitted(Context ctx) {
        return Build.VERSION.SDK_INT < 23 || ctx.checkSelfPermission(PERMISSION) == PackageManager.PERMISSION_GRANTED;
    }

    /** Whether Termux defines the permission at all (an old Termux, or none installed). */
    static boolean available(Context ctx) {
        try {
            ctx.getPackageManager().getPermissionInfo(PERMISSION, 0);
            return true;
        } catch (PackageManager.NameNotFoundException e) {
            return false;
        }
    }

    /** Ask Termux for the secret, unless that was done moments ago. Returns false if it cannot. */
    static boolean request(Context ctx, int uid) {
        if (!permitted(ctx) || refused.contains(uid)) return false;
        Long last = asked.get(uid);
        long now = SystemClock.elapsedRealtime();
        if (last != null && now - last < RETRY_MS) return true;
        asked.put(uid, now);
        Intent result = new Intent(ctx, PairResultReceiver.class).putExtra("uid", uid);
        // Termux fills its result into this intent, so it must stay mutable.
        int flags = PendingIntent.FLAG_UPDATE_CURRENT | (Build.VERSION.SDK_INT >= 31 ? PendingIntent.FLAG_MUTABLE : 0);
        PendingIntent pi = PendingIntent.getBroadcast(ctx, uid, result, flags);
        Intent i = new Intent("com.termux.RUN_COMMAND")
                .setClassName(TERMUX, "com.termux.app.RunCommandService")
                .putExtra("com.termux.RUN_COMMAND_PATH", "$PREFIX/bin/lindroids")
                .putExtra("com.termux.RUN_COMMAND_ARGUMENTS", new String[] {"app-secret"})
                .putExtra("com.termux.RUN_COMMAND_BACKGROUND", true)
                .putExtra("com.termux.RUN_COMMAND_COMMAND_LABEL", "Lindroids pairing")
                .putExtra("com.termux.RUN_COMMAND_PENDING_INTENT", pi);
        try {
            if (Build.VERSION.SDK_INT >= 26) ctx.startForegroundService(i); else ctx.startService(i);
            Log.i(LinkService.TAG, "asked Termux for the pairing secret (uid " + uid + ")");
            return true;
        } catch (Throwable t) {
            Log.w(LinkService.TAG, "cannot ask Termux for the pairing secret", t);
            return false;
        }
    }

    /** Try again from scratch: the permission was just granted, or the CLI announced the server. */
    static void reset(int uid) {
        asked.remove(uid);
        refused.remove(uid);
        gave.remove(uid);
    }
}
