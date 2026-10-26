package io.github.dixonsolutions.lindroids;

import android.app.Application;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.os.Build;

public final class App extends Application {
    static final String CHANNEL = "lindroids";

    @Override
    public void onCreate() {
        super.onCreate();
        System.loadLibrary("lindroids");
        Native.appInit(getFilesDir().getAbsolutePath(), getResources().getDisplayMetrics().density);
        LinkService.app = this;
        if (Build.VERSION.SDK_INT >= 26) {
            NotificationChannel ch = new NotificationChannel(CHANNEL, "Running apps", NotificationManager.IMPORTANCE_LOW);
            ch.setShowBadge(false);
            getSystemService(NotificationManager.class).createNotificationChannel(ch);
        }
    }
}
