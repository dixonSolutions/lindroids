package io.github.dixonsolutions.lindroids;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

/**
 * Taps, buttons and dismissals of notifications from Linux programs ({@link Portal#notify}).
 * Not exported: only our own PendingIntents reach it. Rust knows which link and which of the
 * server's ids each Android notification is.
 */
public final class NoteReceiver extends BroadcastReceiver {
    static final String EXTRA_NOTE = "note";
    static final String EXTRA_ACTION = "action";

    @Override public void onReceive(Context ctx, Intent intent) {
        int note = intent.getIntExtra(EXTRA_NOTE, -1);
        if (note < 0) return;
        String action = intent.getStringExtra(EXTRA_ACTION);
        if (action == null) {
            Native.notificationDismissed(note);
            return;
        }
        int link = Native.notificationAction(note, action);
        // A button does not take the notification away by itself; it is done all the same.
        Portal.cancel(ctx, note);
        if (link >= 0 && "default".equals(action)) Portal.bringToFront(ctx, link);
    }
}
