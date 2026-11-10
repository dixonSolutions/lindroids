package org.lindroids;

import android.app.ActivityManager;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

/**
 * A run ended ({@code org.lindroids.END}, from Lindroids in the terminal app, with the run and its
 * ticket): its window goes, even when its page cannot close it (frozen in the background, or its
 * process gone while the task stays in Recents). Only the task that shows that run with that
 * ticket is removed, so no other app can close a window it did not open.
 */
public final class End extends BroadcastReceiver {
    @Override
    public void onReceive(Context c, Intent i) {
        String run = i.getStringExtra("org.lindroids.RUN");
        String ticket = i.getStringExtra("org.lindroids.TICKET");
        if (run == null || ticket == null || ticket.isEmpty()) return;
        try {
            ActivityManager am = (ActivityManager) c.getSystemService(Context.ACTIVITY_SERVICE);
            for (ActivityManager.AppTask t : am.getAppTasks()) {
                Intent base = t.getTaskInfo().baseIntent;
                String[] r = Runs.of(c, base);
                if (r != null && r[0].equals(run) && r[1].equals(ticket)) {
                    Runs.forget(c, Runs.key(base));
                    t.finishAndRemoveTask();
                }
            }
        } catch (Throwable ignored) {
            // No task of ours, or Android refused: the page closes its window when it can.
        }
    }
}
