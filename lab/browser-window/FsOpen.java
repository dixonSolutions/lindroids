package probe;

import android.app.PendingIntent;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.net.Uri;
import android.os.Bundle;
import android.os.IBinder;
import java.lang.reflect.Method;

/**
 * Full screen without a tap, from the terminal app's side: a Custom Tab with the extras a browser
 * might honour, sent with the terminal app's uid from app_process (see fsprobe.py).
 *
 *   FsOpen <caller-pkg> <url> <browser-pkg> [options]
 *     --session-id      a PendingIntent of the caller as androidx's EXTRA_SESSION_ID, so the
 *                       browser sees a session without a bound service
 *     --twa             EXTRA_LAUNCH_AS_TRUSTED_WEB_ACTIVITY
 *     --immersive       TrustedWebActivityDisplayMode.ImmersiveMode (sticky, cutout: short edges)
 *     --ez k v / --ei k n / --es k v   any other extra
 *     --bind            try to bind the browser's CustomTabsService (what a TWA client does first)
 */
public final class FsOpen {
    public static void main(String[] a) throws Exception {
        android.os.Looper.prepareMainLooper();
        String caller = a[0], browser = a[2];
        Context ctx = appContext(caller);
        Intent i = new Intent(Intent.ACTION_VIEW, Uri.parse(a[1]));
        i.setPackage(browser);
        i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_NEW_DOCUMENT);
        Bundle extras = new Bundle();
        extras.putBinder("android.support.customtabs.extra.SESSION", null);
        for (int k = 3; k < a.length; k++) {
            switch (a[k]) {
                case "--session-id":
                    Intent self = new Intent("probe.fs.SESSION").setPackage(caller);
                    extras.putParcelable("androidx.browser.customtabs.extra.SESSION_ID",
                            PendingIntent.getActivity(ctx, 0, self, PendingIntent.FLAG_IMMUTABLE));
                    break;
                case "--twa":
                    extras.putBoolean("android.support.customtabs.extra.LAUNCH_AS_TRUSTED_WEB_ACTIVITY", true);
                    break;
                case "--immersive": {
                    Bundle m = new Bundle();
                    m.putInt("androidx.browser.trusted.displaymode.KEY_ID", 1);
                    m.putBoolean("androidx.browser.trusted.displaymode.KEY_STICKY", true);
                    m.putInt("androidx.browser.trusted.displaymode.KEY_CUTOUT_MODE", 1);
                    extras.putBundle("androidx.browser.trusted.extra.DISPLAY_MODE", m);
                    break;
                }
                case "--ez": extras.putBoolean(a[++k], Boolean.parseBoolean(a[++k])); break;
                case "--ei": extras.putInt(a[++k], Integer.parseInt(a[++k])); break;
                case "--es": extras.putString(a[++k], a[++k]); break;
                case "--bind": bind(ctx, browser); break;
                default: throw new IllegalArgumentException(a[k]);
            }
        }
        i.putExtras(extras);
        try {
            System.out.println("started " + start(caller, i) + " " + extras.keySet());
        } catch (Exception e) {
            System.out.println("start: " + (e.getCause() != null ? e.getCause() : e));
        }
        System.exit(0);
    }

    static void bind(Context ctx, String browser) {
        Intent s = new Intent("android.support.customtabs.action.CustomTabsService").setPackage(browser);
        try {
            boolean ok = ctx.bindService(s, new ServiceConnection() {
                public void onServiceConnected(ComponentName n, IBinder b) { System.out.println("bind: connected " + n); }
                public void onServiceDisconnected(ComponentName n) {}
            }, Context.BIND_AUTO_CREATE);
            System.out.println("bind: returned " + ok);
        } catch (Throwable e) {
            System.out.println("bind: " + e);
        }
    }

    /** startActivity as the caller package (Context.startActivity would name "android"). */
    static Object start(String caller, Intent i) throws Exception {
        Class<?> sm = Class.forName("android.os.ServiceManager");
        IBinder b = (IBinder) sm.getMethod("getService", String.class).invoke(null, "activity_task");
        Object svc = Class.forName("android.app.IActivityTaskManager$Stub").getMethod("asInterface", IBinder.class).invoke(null, b);
        Method m = null;
        for (Method c : svc.getClass().getMethods()) if (c.getName().equals("startActivity")) m = c;
        Class<?>[] t = m.getParameterTypes();
        Object[] args = new Object[t.length];
        boolean beforeIntent = true, pkgSet = false;
        for (int k = 0; k < t.length; k++) {
            if (t[k] == Intent.class) { args[k] = i; beforeIntent = false; }
            else if (t[k] == String.class && beforeIntent && !pkgSet) { args[k] = caller; pkgSet = true; }
            else if (t[k] == int.class) args[k] = 0;
            else args[k] = null;
        }
        return m.invoke(svc, args);
    }

    static Context appContext(String pkg) throws Exception {
        Class<?> at = Class.forName("android.app.ActivityThread");
        Object thread = at.getMethod("systemMain").invoke(null);
        Context sys = (Context) at.getMethod("getSystemContext").invoke(thread);
        return sys.createPackageContext(pkg, Context.CONTEXT_IGNORE_SECURITY);
    }
}
