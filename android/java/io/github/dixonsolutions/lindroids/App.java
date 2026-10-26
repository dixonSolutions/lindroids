package io.github.dixonsolutions.lindroids;

import android.app.Application;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.content.res.Configuration;
import android.os.Build;

public final class App extends Application {
    static final String CHANNEL = "lindroids";
    static final String CHANNEL_PAIR = "pairing";
    /** Notifications from Linux programs; the ids are portal.rs's CHANNEL_LINUX / CHANNEL_URGENT. */
    static final String CHANNEL_LINUX = "linux";
    static final String CHANNEL_LINUX_URGENT = "linux_urgent";

    @Override
    public void onCreate() {
        super.onCreate();
        System.loadLibrary("lindroids");
        android.util.DisplayMetrics dm = getResources().getDisplayMetrics();
        Native.appInit(getFilesDir().getAbsolutePath(), getCacheDir().getAbsolutePath(), dm.density, dm.widthPixels, dm.heightPixels);
        LinkService.app = this;
        if (Build.VERSION.SDK_INT >= 26) {
            NotificationManager nm = getSystemService(NotificationManager.class);
            NotificationChannel ch = new NotificationChannel(CHANNEL, "Running apps", NotificationManager.IMPORTANCE_LOW);
            ch.setShowBadge(false);
            nm.createNotificationChannel(ch);
            NotificationChannel pair = new NotificationChannel(CHANNEL_PAIR, "Pairing requests", NotificationManager.IMPORTANCE_HIGH);
            nm.createNotificationChannel(pair);
            nm.createNotificationChannel(new NotificationChannel(CHANNEL_LINUX, "Linux apps", NotificationManager.IMPORTANCE_DEFAULT));
            nm.createNotificationChannel(new NotificationChannel(CHANNEL_LINUX_URGENT, "Linux apps (urgent)", NotificationManager.IMPORTANCE_HIGH));
        }
        SystemMonitor.start(this);
    }

    /** Theme (dark mode), font scale, locale... changed: Linux programs follow. */
    @Override
    public void onConfigurationChanged(Configuration c) {
        super.onConfigurationChanged(c);
        SystemMonitor.configurationChanged();
    }

    /** Android wants memory back: Linux programs hear it as the MemoryMonitor portal's warning. */
    @Override
    public void onTrimMemory(int level) {
        super.onTrimMemory(level);
        Native.memoryWarning(level);
    }
}
