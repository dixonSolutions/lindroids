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
 *
 * <p>Or the server asks whether a run's window is still in Recents ({@code org.lindroids.ASK},
 * with the run, its ticket and the server's {@code host:port}): its page has been gone in the
 * background for long, and Android may have killed this app's process for memory while its card
 * stays, which brings the page back when tapped. The answer goes to the server over loopback:
 * {@code /run/<pid>/kept} when a task shows that run with that ticket, else
 * {@code /run/<pid>/closed}.</p>
 */
public final class End extends BroadcastReceiver {
    @Override
    public void onReceive(Context c, Intent i) {
        String run = i.getStringExtra("org.lindroids.RUN");
        String ticket = i.getStringExtra("org.lindroids.TICKET");
        if (run == null || ticket == null || ticket.isEmpty()) return;
        boolean ask = "org.lindroids.ASK".equals(i.getAction());
        String[] shown = null;
        try {
            ActivityManager am = (ActivityManager) c.getSystemService(Context.ACTIVITY_SERVICE);
            for (ActivityManager.AppTask t : am.getAppTasks()) {
                Intent base = t.getTaskInfo().baseIntent;
                String[] r = Runs.of(c, base);
                if (r != null && r[0].equals(run) && r[1].equals(ticket)) {
                    if (ask) {
                        shown = r;
                        break;
                    }
                    Runs.forget(c, Runs.key(base));
                    t.finishAndRemoveTask();
                }
            }
        } catch (Throwable ignored) {
            // No task of ours, or Android refused: the page closes its window when it can; asked,
            // no answer, and the server takes the window for gone.
            return;
        }
        if (!ask) return;
        String host = i.getStringExtra("org.lindroids.HOST");
        final String[] r = shown != null ? shown : host == null ? null : new String[]{run, ticket, host};
        if (r == null) return;
        final String answer = shown != null ? "kept" : "closed";
        final PendingResult pending = goAsync();
        new Thread(() -> {
            try {
                Runs.tell(r, answer);
            } finally {
                pending.finish();
            }
        }).start();
    }
}
