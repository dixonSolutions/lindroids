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
import android.view.inputmethod.EditorInfo;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.TextView;

/**
 * Asks whether a sandbox app may open windows (and receive what you type in them), and, the first
 * time, for the pairing code that `lindroids` prints in the terminal.
 */
public final class ApproveActivity extends Activity {
    /** The dialog on screen: each new request (a retry, a wrong code) replaces it, never stacks. */
    private static ApproveActivity showing;
    private AlertDialog dialog;

    static void ask(Context ctx, int uid, String sandbox, boolean needCode, boolean interactive) {
        Intent i = new Intent(ctx, ApproveActivity.class).putExtra("uid", uid).putExtra("sandbox", sandbox)
                .putExtra("needCode", needCode);
        show(ctx, i, uid, sandbox, needCode ? "Tap to pair with the code shown by lindroids" : "Tap to allow", interactive);
    }

    /** Termux's server: only Android's "Run commands in Termux" permission is asked for. */
    static void askTermux(Context ctx, int uid, String sandbox, boolean interactive) {
        Intent i = new Intent(ctx, ApproveActivity.class).putExtra("uid", uid).putExtra("sandbox", sandbox)
                .putExtra("termux", true);
        show(ctx, i, uid, sandbox, "Tap to pair with Termux", interactive);
    }

    private static void show(Context ctx, Intent i, int uid, String sandbox, String text, boolean interactive) {
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
                .setContentText(text)
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

    @Override protected void onDestroy() {
        if (showing == this) showing = null;
        if (dialog != null) dialog.dismiss();
        super.onDestroy();
    }

    @Override protected void onCreate(Bundle b) {
        super.onCreate(b);
        if (showing != null && showing != this) showing.finish();
        showing = this;
        LinkService.app = getApplicationContext();
        if (getIntent().getBooleanExtra("termux", false) && Build.VERSION.SDK_INT >= 23) {
            if (TermuxPairing.permitted(this)) {
                termuxGranted();
            } else {
                // Android's own dialog: "Allow Lindroids to run commands in Termux environment?"
                requestPermissions(new String[] {TermuxPairing.PERMISSION}, 1);
            }
            return;
        }
        showDialog();
    }

    private void termuxGranted() {
        int uid = getIntent().getIntExtra("uid", -1);
        LinkService.announcedPairing.remove(uid);
        TermuxPairing.reset(uid);
        LinkService.ensureRunning(this);
        if (!TermuxPairing.request(this, uid)) TermuxPairing.refused.add(uid);
        finish();
    }

    @Override public void onRequestPermissionsResult(int request, String[] permissions, int[] results) {
        if (results.length > 0 && results[0] == android.content.pm.PackageManager.PERMISSION_GRANTED) {
            termuxGranted();
        } else {
            // Denied: pair the old way, with the code.
            TermuxPairing.refused.add(getIntent().getIntExtra("uid", -1));
            getIntent().putExtra("needCode", true).putExtra("termux", false);
            showDialog();
        }
    }

    private void showDialog() {
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
        final Runnable accept = () -> {
            LinkService.setAllowed(uid, true);
            LinkService.announcedPairing.remove(uid);
            LinkService.ensureRunning(this);
            String c = needCode ? code.getText().toString().trim() : null;
            LinkService.connect(uid, LinkService.socketFor(uid), c, true);
            finish();
        };
        dialog = new AlertDialog.Builder(this)
                .setTitle("Allow Linux apps from " + sandbox + "?")
                .setView(col)
                .setPositiveButton(needCode ? "Pair" : "Allow", (d, w) -> accept.run())
                .setNegativeButton("Not now", (d, w) -> finish())
                .setOnCancelListener(d -> finish())
                .show();
        if (needCode) {
            // The keyboard's Done (or Enter) pairs, as the Pair button does.
            code.setImeOptions(EditorInfo.IME_ACTION_DONE);
            code.setSingleLine(true);
            code.setOnEditorActionListener((v, action, ev) -> {
                accept.run();
                return true;
            });
            code.requestFocus();
        }
    }
}
