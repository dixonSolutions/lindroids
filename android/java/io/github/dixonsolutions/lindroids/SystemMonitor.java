package io.github.dixonsolutions.lindroids;

import android.animation.ValueAnimator;
import android.app.ActivityManager;
import android.app.UiModeManager;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.res.Configuration;
import android.content.res.TypedArray;
import android.database.ContentObserver;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.os.BatteryManager;
import android.os.Build;
import android.os.PowerManager;
import android.provider.Settings;
import android.util.Log;

import java.util.TimeZone;

/**
 * Reads what Linux programs want to know about Android (dark theme, accent colour, contrast,
 * animations, font scale, power saver, network, memory, battery, locale, time zone) and hands it
 * to Rust whenever any of it may have changed. Rust decides what is worth sending, debounces it
 * and sends it to every link ({@code portal.rs}); this side only listens and reads.
 *
 * Everything runs on the main thread.
 */
final class SystemMonitor {
    private SystemMonitor() {}

    private static Context ctx;
    /** The default network as the last callback described it; null when there is none. */
    private static NetworkCapabilities netCaps;
    private static Network net;

    /** While a link is up, memory is read this often (Rust only sends a real change). */
    private static final long MEMORY_POLL_MS = 30_000;

    static void start(Context app) {
        if (ctx != null) return;
        ctx = app.getApplicationContext();

        BroadcastReceiver changed = new BroadcastReceiver() {
            @Override public void onReceive(Context c, Intent i) {
                // The framework resets the default zone itself, but maybe after this receiver.
                if (Intent.ACTION_TIMEZONE_CHANGED.equals(i.getAction())) TimeZone.setDefault(null);
                refresh();
            }
        };
        IntentFilter f = new IntentFilter();
        f.addAction(PowerManager.ACTION_POWER_SAVE_MODE_CHANGED);
        f.addAction(Intent.ACTION_BATTERY_CHANGED);
        f.addAction(Intent.ACTION_TIMEZONE_CHANGED);
        f.addAction(Intent.ACTION_LOCALE_CHANGED);
        ctx.registerReceiver(changed, f);

        ctx.getContentResolver().registerContentObserver(
                Settings.Global.getUriFor(Settings.Global.ANIMATOR_DURATION_SCALE), false,
                new ContentObserver(LinkService.main) {
                    @Override public void onChange(boolean self) { refresh(); }
                });

        if (Build.VERSION.SDK_INT >= 34) {
            UiModeManager um = ctx.getSystemService(UiModeManager.class);
            if (um != null) um.addContrastChangeListener(ctx.getMainExecutor(), contrast -> refresh());
        }

        try {
            ConnectivityManager cm = ctx.getSystemService(ConnectivityManager.class);
            cm.registerDefaultNetworkCallback(new ConnectivityManager.NetworkCallback() {
                @Override public void onAvailable(Network n) {
                    // Android 7 does not follow onAvailable with onCapabilitiesChanged: ask.
                    NetworkCapabilities caps = cm.getNetworkCapabilities(n);
                    LinkService.main.post(() -> { net = n; netCaps = caps; refresh(); });
                }
                @Override public void onCapabilitiesChanged(Network n, NetworkCapabilities caps) {
                    LinkService.main.post(() -> { net = n; netCaps = caps; refresh(); });
                }
                @Override public void onLost(Network n) {
                    // Only the default going away counts, not an old one after a switch.
                    LinkService.main.post(() -> { if (n.equals(net)) { net = null; netCaps = null; } refresh(); });
                }
            });
        } catch (RuntimeException e) {
            Log.w(LinkService.TAG, "no network callback", e);
        }

        LinkService.main.postDelayed(new Runnable() {
            @Override public void run() {
                if (!LinkService.links.isEmpty()) refresh();
                LinkService.main.postDelayed(this, MEMORY_POLL_MS);
            }
        }, MEMORY_POLL_MS);

        refresh();
    }

    /** A configuration change (App.onConfigurationChanged): theme, font scale, locale. */
    static void configurationChanged() { refresh(); }

    /** Read everything and hand it to Rust. Cheap; Rust drops what did not change. */
    static void refresh() {
        if (ctx == null) return;
        try {
            Configuration conf = ctx.getResources().getConfiguration();
            boolean dark = (conf.uiMode & Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES;

            boolean hasAccent = false;
            int accent = 0;
            if (Build.VERSION.SDK_INT >= 31) {
                accent = ctx.getColor(android.R.color.system_accent1_500);
                hasAccent = true;
            } else {
                TypedArray a = ctx.obtainStyledAttributes(new int[]{android.R.attr.colorAccent});
                if (a.hasValue(0)) { accent = a.getColor(0, 0); hasAccent = true; }
                a.recycle();
            }

            boolean highContrast = false;
            if (Build.VERSION.SDK_INT >= 34) {
                UiModeManager um = ctx.getSystemService(UiModeManager.class);
                highContrast = um != null && um.getContrast() >= 0.5f;
            }

            boolean reducedMotion = Settings.Global.getFloat(ctx.getContentResolver(),
                    Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f;
            if (Build.VERSION.SDK_INT >= 26) reducedMotion |= !ValueAnimator.areAnimatorsEnabled();

            PowerManager pm = ctx.getSystemService(PowerManager.class);
            boolean powerSaver = pm != null && pm.isPowerSaveMode();

            ConnectivityManager cm = ctx.getSystemService(ConnectivityManager.class);
            NetworkCapabilities caps = netCaps;
            boolean internet = caps != null && caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET);
            boolean validated = caps != null && caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED);
            boolean captive = caps != null && caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_CAPTIVE_PORTAL);
            boolean metered = cm != null && cm.isActiveNetworkMetered();

            ActivityManager am = ctx.getSystemService(ActivityManager.class);
            ActivityManager.MemoryInfo mem = new ActivityManager.MemoryInfo();
            if (am != null) am.getMemoryInfo(mem);

            // The sticky broadcast: the battery as it is now, without waiting for a change.
            int battery = -1;
            boolean charging = false;
            Intent b = ctx.registerReceiver(null, new IntentFilter(Intent.ACTION_BATTERY_CHANGED));
            if (b != null && b.getBooleanExtra(BatteryManager.EXTRA_PRESENT, true)) {
                int level = b.getIntExtra(BatteryManager.EXTRA_LEVEL, -1);
                int scale = b.getIntExtra(BatteryManager.EXTRA_SCALE, -1);
                if (level >= 0 && scale > 0) battery = Math.round(level * 100f / scale);
                int status = b.getIntExtra(BatteryManager.EXTRA_STATUS, -1);
                charging = status == BatteryManager.BATTERY_STATUS_CHARGING || status == BatteryManager.BATTERY_STATUS_FULL;
            }

            String locale = conf.getLocales().isEmpty() ? "" : conf.getLocales().get(0).toLanguageTag();
            String zone = TimeZone.getDefault().getID();

            Native.systemState(dark, hasAccent, accent, highContrast, reducedMotion, conf.fontScale, powerSaver,
                    net != null, internet, validated, captive, metered,
                    mem.totalMem, mem.availMem, mem.lowMemory, battery, charging, locale, zone);
        } catch (RuntimeException e) {
            Log.w(LinkService.TAG, "reading the system state", e);
        }
    }
}
