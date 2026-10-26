package io.github.dixonsolutions.lindroids;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

/**
 * `lindroids` (the CLI) announcing a server: which abstract socket, which uid, and the pairing
 * secret. Nothing here is trusted as is: the app checks the socket's owner with SO_PEERCRED, and
 * asks the user before a new sandbox may open windows.
 */
public final class HelloReceiver extends BroadcastReceiver {
    @Override public void onReceive(Context ctx, Intent intent) {
        String socket = intent.getStringExtra("socket");
        // The CLI sends the uid as a string (`am --es`).
        int uid = -1;
        try { uid = Integer.parseInt(intent.getStringExtra("uid")); } catch (Exception ignored) {}
        if (socket == null || uid < 0) return;
        LinkService.onHello(ctx, socket, uid, intent.getStringExtra("sandbox"), intent.getStringExtra("secret"));
    }
}
