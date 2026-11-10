package org.lindroids;

import android.app.Service;
import android.content.Intent;
import android.os.IBinder;

/**
 * Tells the server when the user removes a run's window from Recents: Android says so only to a
 * service that is running ({@code onTaskRemoved}), and the page may be gone already (its process
 * frozen or killed in the background), so the page cannot say it. The server then ends the run
 * after the same 5 seconds as a window closed on screen.
 *
 * <p>A plain started service: no permission, no notification. Android stops it about a minute
 * after the app leaves the screen; each window starts it again when it comes back. A window
 * swiped away after that is lost without a word, and the server ends its run after its limit for
 * windows lost in the background (docs/DISPLAY.md, "The grace period").</p>
 */
public final class CloseWatch extends Service {
    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        return START_NOT_STICKY;
    }

    @Override
    public void onTaskRemoved(Intent root) {
        final String[] r = Runs.of(this, root);
        Runs.forget(this, Runs.key(root));
        if (r == null) return;
        // The process stays while this service runs: the call goes out before it stops.
        new Thread(() -> {
            WindowActivity.tellClosed(r);
            stopSelf();
        }).start();
    }
}
