package io.github.dixonsolutions.lindroids;

import android.app.Activity;
import android.app.AlertDialog;
import android.app.Notification;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.os.Build;
import android.os.Bundle;
import android.text.InputType;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.TextView;

/**
 * Asks whether a sandbox app may open windows (and receive what you type in them), and, the first
 * time, for the pairing code that `lindroids` prints in the terminal.
 */
public final class ApproveActivity extends Activity {
    static void ask(Context ctx, int uid, String sandbox, boolean needCode, boolean interactive) {
        Intent i = new Intent(ctx, ApproveActivity.class).putExtra("uid", uid).putExtra("sandbox", sandbox)
                .putExtra("needCode", needCode);
        i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_SINGLE_TOP);
        if (interactive) {
            try { ctx.startActivity(i); return; } catch (Throwable ignored) {}
        }
        // Found by scanning: a notification, not a dialog out of nowhere.
        PendingIntent pi = PendingIntent.getActivity(ctx, uid, i,
                PendingIntent.FLAG_UPDATE_CURRENT | (Build.VERSION.SDK_INT >= 23 ? PendingIntent.FLAG_IMMUTABLE : 0));
        Notification.Builder b = Build.VERSION.SDK_INT >= 26
                ? new Notification.Builder(ctx, App.CHANNEL_PAIR) : new Notification.Builder(ctx);
        Notification n = b.setSmallIcon(R.drawable.ic_stat)
                .setGroup("pairing")
                .setPriority(Notification.PRIORITY_HIGH)
                .setContentTitle(sandbox + " wants to show Linux apps")
                .setContentText(needCode ? "Tap to pair with the code shown by lindroids" : "Tap to allow")
                .setContentIntent(pi).setAutoCancel(true).build();
        ((NotificationManager) ctx.getSystemService(NOTIFICATION_SERVICE)).notify(LinkService.NOTE_PAIR + uid, n);
    }

    static void dismissPairing(Context ctx, int uid) {
        ((NotificationManager) ctx.getSystemService(NOTIFICATION_SERVICE)).cancel(LinkService.NOTE_PAIR + uid);
    }

    static void wrongCode(Context ctx, int uid, String sandbox, boolean open) {
        Intent i = new Intent(ctx, ApproveActivity.class).putExtra("uid", uid).putExtra("sandbox", sandbox)
                .putExtra("needCode", true).putExtra("error", open
                        ? "That code did not match. Check the code shown by lindroids and try again."
                        : "Too many wrong codes. Restart the server (lindroids stop) to pair again.");
        i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_SINGLE_TOP);
        try { ctx.startActivity(i); } catch (Throwable ignored) {}
    }

    @Override protected void onCreate(Bundle b) {
        super.onCreate(b);
        LinkService.app = getApplicationContext();
        final int uid = getIntent().getIntExtra("uid", -1);
        final String sandbox = getIntent().getStringExtra("sandbox");
        // A code is needed unless this is only about allowing a server we already share a secret
        // with (a second server in the same app has a secret of its own: the caller says so).
        final boolean needCode = getIntent().getBooleanExtra("needCode", true);
        String error = getIntent().getStringExtra("error");
        String[] pkgs = getPackageManager().getPackagesForUid(uid);
        String pkg = pkgs != null && pkgs.length > 0 ? pkgs[0] : "uid " + uid;

        int pad = (int) (20 * getResources().getDisplayMetrics().density);
        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        col.setPadding(pad, pad / 2, pad, 0);
        TextView msg = new TextView(this);
        msg.setText("Programs started from " + sandbox + " (" + pkg + ") will be able to open windows, play sound, "
                + "and receive what you type and touch in those windows."
                + (needCode ? "\n\nEnter the pairing code that lindroids shows in the terminal:" : "")
                + (error != null ? "\n\n" + error : ""));
        col.addView(msg);
        final EditText code = new EditText(this);
        if (needCode) {
            code.setInputType(InputType.TYPE_CLASS_NUMBER);
            code.setHint("6-digit code");
            col.addView(code);
        }
        new AlertDialog.Builder(this)
                .setTitle("Allow Linux apps from " + sandbox + "?")
                .setView(col)
                .setPositiveButton(needCode ? "Pair" : "Allow", (d, w) -> {
                    LinkService.setAllowed(uid, true);
                    LinkService.announcedPairing.remove(uid);
                    LinkService.ensureRunning(this);
                    String c = needCode ? code.getText().toString().trim() : null;
                    LinkService.connect(uid, LinkService.socketFor(uid), c, true);
                    finish();
                })
                .setNegativeButton("Not now", (d, w) -> finish())
                .setOnCancelListener(d -> finish())
                .show();
    }
}
