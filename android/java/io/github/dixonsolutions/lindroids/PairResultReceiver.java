package io.github.dixonsolutions.lindroids;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.os.Bundle;
import android.util.Log;

/**
 * Termux's answer to {@link TermuxPairing#request}: the output of `lindroids app-secret`. Not
 * exported, so only the PendingIntent this app gave Termux reaches it.
 */
public final class PairResultReceiver extends BroadcastReceiver {
    @Override public void onReceive(Context ctx, Intent intent) {
        LinkService.app = ctx.getApplicationContext();
        int uid = intent.getIntExtra("uid", -1);
        Bundle r = intent.getBundleExtra("result");
        String out = r != null ? r.getString("stdout", "") : "";
        String secret = out == null ? "" : out.trim();
        int exit = r != null ? r.getInt("exitCode", -1) : -1;
        if (uid < 0) return;
        if (exit == 0 && secret.length() >= 32 && !secret.contains("\n")) {
            Log.i(LinkService.TAG, "Termux gave the pairing secret (uid " + uid + ")");
            TermuxPairing.gave.add(uid);
            LinkService.addSecret(uid, secret);
            LinkService.setAllowed(uid, true);
            LinkService.announcedPairing.remove(uid);
            LinkService.ensureRunning(ctx);
            LinkService.connect(uid, LinkService.socketFor(uid), null, true);
            return;
        }
        // Termux refused (allow-external-apps is off) or lindroids is missing: pair with the code.
        String why = r == null ? "no answer" : (r.getString("errmsg", "") + " " + r.getString("stderr", "")).trim();
        Log.w(LinkService.TAG, "Termux did not give the pairing secret: exit " + exit + " " + why);
        TermuxPairing.refused.add(uid);
        String sandbox = LinkService.sandboxName(uid, null);
        LinkService.announcedPairing.add(uid);
        ApproveActivity.ask(ctx, uid, sandbox, true, true);
    }
}
