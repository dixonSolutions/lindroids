package io.github.dixonsolutions.lindroids;

import android.app.Application;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.os.Build;

public final class App extends Application {
    static final String CHANNEL = "lindroids";
    static final String CHANNEL_PAIR = "pairing";

    @Override
    public void onCreate() {
        super.onCreate();
        System.loadLibrary("lindroids");
        android.util.DisplayMetrics dm = getResources().getDisplayMetrics();
        Native.appInit(getFilesDir().getAbsolutePath(), dm.density, dm.widthPixels, dm.heightPixels);
        LinkService.app = this;
        if (Build.VERSION.SDK_INT >= 26) {
            NotificationChannel ch = new NotificationChannel(CHANNEL, "Running apps", NotificationManager.IMPORTANCE_LOW);
            ch.setShowBadge(false);
            getSystemService(NotificationManager.class).createNotificationChannel(ch);
            NotificationChannel pair = new NotificationChannel(CHANNEL_PAIR, "Pairing requests", NotificationManager.IMPORTANCE_HIGH);
            getSystemService(NotificationManager.class).createNotificationChannel(pair);
        }
    }
}
