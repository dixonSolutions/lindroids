package lindroids;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.ContextWrapper;
import android.content.Intent;
import android.content.pm.ShortcutInfo;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.graphics.Rect;
import android.graphics.drawable.Icon;
import android.os.IBinder;
import android.os.Process;

import java.lang.reflect.Method;
import java.util.Arrays;
import java.util.List;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;

/**
 * Pins a home screen shortcut for the terminal app this runs as, without any app of Lindroids'
 * own. `lindroids export` runs it with Android's {@code app_process} from inside the terminal app
 * (Termux), so the process has the terminal app's uid and Android's shortcut service accepts it as
 * that package — the same way Termux's own {@code am} command talks to Android.
 *
 * <pre>
 *   app_process -Djava.class.path=shortcut.dex /system/bin lindroids.Shortcut \
 *       pin &lt;package&gt; &lt;id&gt; &lt;label&gt; &lt;icon.png&gt; &lt;intent action&gt; &lt;component&gt; &lt;mime type&gt; &lt;extra text&gt;
 *   app_process ... lindroids.Shortcut remove &lt;package&gt; &lt;id&gt;
 *   app_process ... lindroids.Shortcut open &lt;package&gt; new|raise &lt;url&gt; &lt;browser&gt;...
 *   app_process ... lindroids.Shortcut notify &lt;package&gt; &lt;id&gt; &lt;program&gt; &lt;title&gt; &lt;text&gt; &lt;icon.png&gt; &lt;small-icon.png&gt; &lt;url&gt; &lt;browser&gt;...
 *   app_process ... lindroids.Shortcut running &lt;package&gt; &lt;id&gt; &lt;program&gt; &lt;title&gt; &lt;text&gt; &lt;icon.png&gt; &lt;small-icon.png&gt; &lt;url&gt; &lt;browser&gt;...
 *   app_process ... lindroids.Shortcut cancel &lt;package&gt; &lt;id&gt;
 *   app_process ... lindroids.Shortcut device
 * </pre>
 *
 * Prints "ok", or "error: ..." and exits 1. Android's binder interfaces are hidden API, reached by
 * reflection: their signatures differ between releases (Android 12 made the calls asynchronous).
 */
public final class Shortcut {
    private Shortcut() {}

    /** Just enough of a Context for ShortcutInfo.Builder: it asks for the package and the user. */
    static final class As extends ContextWrapper {
        private final String pkg;

        As(String pkg) {
            super(null);
            this.pkg = pkg;
        }

        @Override public String getPackageName() { return pkg; }
        @Override public String getOpPackageName() { return pkg; }
        // Hidden in the SDK, overridden all the same.
        public int getUserId() { return userId(); }
    }

    static int userId() {
        return Process.myUid() / 100000;
    }

    public static void main(String[] a) {
        try {
            // AndroidFuture (Android 12+) posts to the main looper, which app_process has not made.
            if (android.os.Looper.getMainLooper() == null) android.os.Looper.prepareMainLooper();
            if (a.length >= 9 && a[0].equals("pin")) {
                pin(a[1], a[2], a[3], a[4], a[5], a[6], a[7], a[8]);
            } else if (a.length >= 3 && a[0].equals("remove")) {
                remove(a[1], a[2]);
            } else if (a.length >= 5 && a[0].equals("open")) {
                System.out.println("browser " + open(a[1], a[2], a[3], Arrays.copyOfRange(a, 4, a.length)));
            } else if (a.length >= 9 && (a[0].equals("notify") || a[0].equals("running"))) {
                post(a[1], Integer.parseInt(a[2]), a[3], a[4], a[5], a[6], a[7], a[8], Arrays.copyOfRange(a, 9, a.length), a[0].equals("running"));
            } else if (a.length >= 1 && a[0].equals("device")) {
                device();
            } else if (a.length >= 3 && a[0].equals("pinned")) {
                System.out.println("pinned " + pinned(service(), a[1], a[2]));
            } else if (a.length >= 3 && a[0].equals("cancel")) {
                cancel(a[1], Integer.parseInt(a[2]));
            } else {
                throw new IllegalArgumentException("usage: pin <package> <id> <label> <icon> <action> <component> <type> <text> | remove <package> <id>"
                        + " | open <package> new|raise <url> <browser>... | notify|running <package> <id> <program> <title> <text> <icon> <small-icon> <url> <browser>..."
                        + " | cancel <package> <id> | pinned <package> <id> | device");
            }
            System.out.println("ok");
            System.exit(0);
        } catch (Throwable t) {
            Throwable c = t;
            while (c.getCause() != null) c = c.getCause();
            System.out.println("error: " + c);
            System.exit(1);
        }
    }

    static Object service() throws Exception {
        Class<?> sm = Class.forName("android.os.ServiceManager");
        IBinder b = (IBinder) sm.getMethod("getService", String.class).invoke(null, "shortcut");
        if (b == null) throw new IllegalStateException("no shortcut service");
        Class<?> stub = Class.forName("android.content.pm.IShortcutService$Stub");
        return stub.getMethod("asInterface", IBinder.class).invoke(null, b);
    }

    static Method method(Object svc, String name) {
        for (Method m : svc.getClass().getMethods()) {
            if (m.getName().equals(name)) return m;
        }
        throw new IllegalStateException("the shortcut service has no " + name);
    }

    /**
     * Calls a binder method, filling its parameters from {@code values}: each parameter takes the
     * next unused value that fits its type, in order; parameters nothing fits get null (an
     * IntentSender for a result nobody waits for). Waits on the future the call takes, if any.
     */
    static Object call(Object svc, String name, Object... values) throws Exception {
        Method m = method(svc, name);
        Class<?>[] types = m.getParameterTypes();
        Object[] args = new Object[types.length];
        boolean[] used = new boolean[values.length];
        Object future = null;
        for (int i = 0; i < types.length; i++) {
            Class<?> t = types[i];
            if (t.getName().equals("com.android.internal.infra.AndroidFuture")) {
                future = t.getConstructor().newInstance();
                args[i] = future;
                continue;
            }
            for (int k = 0; k < values.length; k++) {
                Object v = values[k];
                if (!used[k] && v != null && (t.isInstance(v) || (t == int.class && v instanceof Integer))) {
                    args[i] = v;
                    used[k] = true;
                    break;
                }
            }
            if (args[i] == null && t == int.class) args[i] = 0;
            if (args[i] == null && t == boolean.class) args[i] = false;
        }
        Object r;
        try {
            r = m.invoke(svc, args);
        } catch (java.lang.reflect.InvocationTargetException e) {
            // Since Android 11 a reply can carry the app ops the call noted; reading them fails
            // in a process that is not an app (SyncNotedAppOp's checks), after the call itself
            // went through. Its answer is lost: null.
            if (!String.valueOf(e.getCause()).contains("SyncNotedAppOp") && !stackMentions(e.getCause(), "SyncNotedAppOp")) throw e;
            r = null;
        }
        if (future != null) {
            return ((Future<?>) future).get(10, TimeUnit.SECONDS);
        }
        return r;
    }

    static boolean hasMethod(Object o, String name) {
        for (Method m : o.getClass().getMethods()) {
            if (m.getName().equals(name)) return true;
        }
        return false;
    }

    static boolean stackMentions(Throwable t, String what) {
        for (; t != null; t = t.getCause()) {
            if (String.valueOf(t.getMessage()).contains(what)) return true;
        }
        return false;
    }

    static void pin(String pkg, String id, String label, String iconPath, String action, String component, String type, String text) throws Exception {
        Intent i = new Intent(action);
        String[] cn = component.split("/", 2);
        i.setClassName(cn[0], cn[1].startsWith(".") ? cn[0] + cn[1] : cn[1]);
        if (!type.isEmpty()) i.setType(type);
        if (!text.isEmpty()) i.putExtra(Intent.EXTRA_TEXT, text);
        i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);

        ShortcutInfo.Builder b = new ShortcutInfo.Builder(new As(pkg), id)
                .setShortLabel(label)
                .setLongLabel(label)
                .setIntent(i);
        Bitmap icon = iconPath.isEmpty() ? null : BitmapFactory.decodeFile(iconPath);
        if (icon != null) b.setIcon(Icon.createWithAdaptiveBitmap(adaptive(icon)));
        ShortcutInfo s = b.build();

        Object svc = service();
        // Removed before (`lindroids remove` disables it: an app cannot unpin): a disabled
        // shortcut can be neither updated nor pinned again until it is enabled.
        try {
            call(svc, "enableShortcuts", pkg, Arrays.asList(id), userId());
        } catch (Exception ignored) {
            // Never pinned: nothing to enable.
        }
        // Already pinned (exported again): update it in place; the launcher keeps it where it is.
        try {
            Object list = Class.forName("android.content.pm.ParceledListSlice").getConstructor(List.class).newInstance(Arrays.asList(s));
            call(svc, "updateShortcuts", pkg, list, userId());
        } catch (Exception ignored) {
            // Not pinned yet, or a release where this takes other arguments: pin it below.
        }
        // On the home screen already: updated above. Asking again would add a second icon.
        if (pinned(svc, pkg, id)) {
            System.out.println("updated");
            return;
        }
        Object r = call(svc, "requestPinShortcut", pkg, s, userId());
        if (Boolean.FALSE.equals(r)) throw new IllegalStateException("the launcher does not take pinned shortcuts");
    }

    /** Whether shortcut {@code id} is pinned and enabled (FLAG_MATCH_PINNED = 4; before Android 11, getPinnedShortcuts). */
    static boolean pinned(Object svc, String pkg, String id) {
        Object r;
        try {
            r = call(svc, "getShortcuts", pkg, 4, userId());
        } catch (Exception e) {
            try {
                r = call(svc, "getPinnedShortcuts", pkg, userId());
            } catch (Exception e2) {
                return false;
            }
        }
        try {
            for (Object o : (List<?>) r.getClass().getMethod("getList").invoke(r)) {
                ShortcutInfo si = (ShortcutInfo) o;
                if (si.getId().equals(id) && si.isEnabled()) return true;
            }
        } catch (Exception ignored) {
            // An answer of another shape: ask the launcher.
        }
        return false;
    }

    /**
     * Opens {@code url} as a window of its own: a Custom Tab of the first browser in
     * {@code browsers} that is installed, in a task of its own (one window in recents per URL).
     * {@code new} always makes a new window; {@code raise} brings back the window that has this URL,
     * if there is one. A Custom Tab needs a session extra holding a Binder, which `am` cannot send;
     * a null one asks for a tab without a session. Returns the browser used.
     */
    static String open(String pkg, String mode, String url, String[] browsers) throws Exception {
        Class<?> sm = Class.forName("android.os.ServiceManager");
        IBinder b = (IBinder) sm.getMethod("getService", String.class).invoke(null, "activity_task");
        String stub = "android.app.IActivityTaskManager$Stub";
        if (b == null) { // before Android 10
            b = (IBinder) sm.getMethod("getService", String.class).invoke(null, "activity");
            stub = "android.app.IActivityManager$Stub";
        }
        Object atm = Class.forName(stub).getMethod("asInterface", IBinder.class).invoke(null, b);
        int flags = Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_NEW_DOCUMENT;
        if (mode.equals("new")) flags |= Intent.FLAG_ACTIVITY_MULTIPLE_TASK;
        String last = "no browser given";
        for (String browser : withDefault(browsers)) {
            Object r = call(atm, "startActivity", pkg, windowIntent(url, browser, flags));
            // ActivityManager.START_SUCCESS and the other non-negative codes (delivered to the top,
            // task brought to the front) all mean the window is there; no answer (see call) means
            // the start went through too.
            if (r == null || (r instanceof Integer && (Integer) r >= 0)) return browser;
            last = browser + ": start result " + r;
        }
        throw new IllegalStateException(last);
    }

    /**
     * A program's icon as an adaptive icon, so the launcher fills its shape with it as it does for
     * apps, instead of shrinking a plain bitmap into a white disc. Linux icons are drawn with their
     * own margins, on transparency: what they draw (margins cut off) goes on a pale backdrop of
     * its own colour, 50 units across in the middle of the 108-unit canvas, of which launcher
     * shapes show the middle 72.
     */
    static Bitmap adaptive(Bitmap src) {
        Rect drawn = opaqueBounds(src);
        int size = 432;
        Bitmap out = Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888);
        Canvas c = new Canvas(out);
        c.drawColor(backdrop(src));
        int box = size * 50 / 108;
        float k = Math.min((float) box / drawn.width(), (float) box / drawn.height());
        int w = Math.round(drawn.width() * k), h = Math.round(drawn.height() * k);
        Rect dst = new Rect((size - w) / 2, (size - h) / 2, (size - w) / 2 + w, (size - h) / 2 + h);
        c.drawBitmap(src, drawn, dst, new Paint(Paint.FILTER_BITMAP_FLAG | Paint.ANTI_ALIAS_FLAG));
        return out;
    }

    /** Where the icon draws anything (alpha above a trace); the whole bitmap if nowhere. */
    static Rect opaqueBounds(Bitmap b) {
        int l = b.getWidth(), t = b.getHeight(), r = -1, bt = -1;
        for (int y = 0; y < b.getHeight(); y++) {
            for (int x = 0; x < b.getWidth(); x++) {
                if (Color.alpha(b.getPixel(x, y)) <= 16) continue;
                l = Math.min(l, x); t = Math.min(t, y); r = Math.max(r, x); bt = Math.max(bt, y);
            }
        }
        return r < 0 ? new Rect(0, 0, b.getWidth(), b.getHeight()) : new Rect(l, t, r + 1, bt + 1);
    }

    /** The icon's average opaque colour, mixed mostly with white. */
    static int backdrop(Bitmap b) {
        long r = 0, g = 0, bl = 0, n = 0;
        int step = Math.max(1, Math.min(b.getWidth(), b.getHeight()) / 32);
        for (int y = 0; y < b.getHeight(); y += step) {
            for (int x = 0; x < b.getWidth(); x += step) {
                int p = b.getPixel(x, y);
                if (Color.alpha(p) < 200) continue;
                r += Color.red(p); g += Color.green(p); bl += Color.blue(p); n++;
            }
        }
        if (n == 0) return Color.WHITE;
        float keep = 0.3f;
        return Color.rgb(Math.round(255 - (255 - r / n) * keep), Math.round(255 - (255 - g / n) * keep), Math.round(255 - (255 - bl / n) * keep));
    }

    /**
     * The terminal app's own Context (resources, application info), which Notification.Builder and
     * PendingIntent need: made from the system context app_process can create, and accepted by
     * Android's services because this process has the terminal app's uid.
     */
    static Context appContext(String pkg) throws Exception {
        Class<?> at = Class.forName("android.app.ActivityThread");
        Object thread = at.getMethod("systemMain").invoke(null);
        Context sys = (Context) at.getMethod("getSystemContext").invoke(thread);
        return sys.createPackageContext(pkg, Context.CONTEXT_IGNORE_SECURITY);
    }

    static Object notificationService() throws Exception {
        Class<?> sm = Class.forName("android.os.ServiceManager");
        IBinder b = (IBinder) sm.getMethod("getService", String.class).invoke(null, "notification");
        return Class.forName("android.app.INotificationManager$Stub").getMethod("asInterface", IBinder.class).invoke(null, b);
    }

    static final String CHANNEL = "lindroids";
    /** One per running program, its icon in the status bar: default importance, no sound. */
    static final String RUNNING = "lindroids-programs";

    /**
     * Posts a program's notification as the terminal app's (Android shows the terminal app as its
     * source, with the program's name and icon). Tapping it brings back the program's window:
     * {@code url} in the first of {@code browsers} installed, as {@code open raise} does.
     * {@code running}: the quiet one that stands for a running program (it stays when tapped; the
     * user can still swipe it away, so it cannot outlive a terminal app that was killed).
     */
    static void post(String pkg, int id, String program, String title, String text, String iconPath, String smallIconPath, String url, String[] browsers, boolean running) throws Exception {
        Context ctx = appContext(pkg);
        Object nm = notificationService();
        NotificationChannel ch = running
                ? new NotificationChannel(RUNNING, "Running Linux programs", NotificationManager.IMPORTANCE_DEFAULT)
                : new NotificationChannel(CHANNEL, "Linux programs", NotificationManager.IMPORTANCE_DEFAULT);
        if (running) {
            // Shown in the status bar (silent ones are not), without a sound.
            ch.setSound(null, null);
            ch.enableVibration(false);
        }
        Object list = Class.forName("android.content.pm.ParceledListSlice").getConstructor(List.class).newInstance(Arrays.asList(ch));
        call(nm, "createNotificationChannels", pkg, list);
        Notification.Builder b = new Notification.Builder(ctx, ch.getId())
                .setContentTitle(title)
                .setContentText(text)
                .setAutoCancel(!running)
                .setOnlyAlertOnce(running)
                .setShowWhen(!running);
        if (!running) b.setStyle(new Notification.BigTextStyle().bigText(text));
        // A group of its own: Android does not bundle it with the terminal app's notifications,
        // so it keeps its own icon in the status bar.
        b.setGroup("lindroids-" + id);
        if (!program.isEmpty() && !program.equals(title)) b.setSubText(program);
        Bitmap icon = iconPath.isEmpty() ? null : BitmapFactory.decodeFile(iconPath);
        Bitmap small = smallIconPath.isEmpty() ? null : BitmapFactory.decodeFile(smallIconPath);
        if (icon != null) b.setLargeIcon(icon);
        // The status bar draws only the small icon's shape: the one-colour icon if there is one.
        if (small != null) {
            b.setSmallIcon(Icon.createWithBitmap(small));
        } else if (icon != null) {
            b.setSmallIcon(Icon.createWithBitmap(icon));
        } else {
            b.setSmallIcon(Icon.createWithResource("android", android.R.drawable.stat_notify_chat));
        }
        if (!url.isEmpty()) {
            for (String browser : withDefault(browsers)) {
                Intent i = windowIntent(url, browser, Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_NEW_DOCUMENT);
                if (ctx.getPackageManager().resolveActivity(i, 0) == null) continue;
                b.setContentIntent(PendingIntent.getActivity(ctx, id, i, PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT));
                break;
            }
        }
        call(nm, "enqueueNotificationWithTag", pkg, pkg, CHANNEL, id, b.build(), userId());
    }

    static void cancel(String pkg, int id) throws Exception {
        call(notificationService(), "cancelNotificationWithTag", pkg, pkg, CHANNEL, id, userId());
    }

    /**
     * What Android says about the device, for programs' power and battery questions:
     * {@code battery <percent> <charging 0|1>} and {@code powersave <0|1>}, a line each, for what
     * it could read.
     */
    static void device() throws Exception {
        Context ctx = appContext(packageOf());
        // The battery service's last broadcast (what apps read), else the battery HAL.
        int level = -1, status = -1;
        try {
            // As the activity manager's own call, with no caller and no receiver: the sticky
            // intent alone (a Context's registerReceiver wants a real app process).
            Class<?> sm = Class.forName("android.os.ServiceManager");
            IBinder ab = (IBinder) sm.getMethod("getService", String.class).invoke(null, "activity");
            Object am = Class.forName("android.app.IActivityManager$Stub").getMethod("asInterface", IBinder.class).invoke(null, ab);
            android.content.IntentFilter f = new android.content.IntentFilter(Intent.ACTION_BATTERY_CHANGED);
            String name = hasMethod(am, "registerReceiverWithFeature") ? "registerReceiverWithFeature" : "registerReceiver";
            Intent b = (Intent) call(am, name, ctx.getPackageName(), f, userId(), 0);
            if (b != null && b.getBooleanExtra(android.os.BatteryManager.EXTRA_PRESENT, true)) {
                int scale = b.getIntExtra(android.os.BatteryManager.EXTRA_SCALE, 100);
                level = scale > 0 ? b.getIntExtra(android.os.BatteryManager.EXTRA_LEVEL, -1) * 100 / scale : -1;
                status = b.getIntExtra(android.os.BatteryManager.EXTRA_STATUS, -1);
            }
        } catch (Exception e) {
            android.os.BatteryManager bm = ctx.getSystemService(android.os.BatteryManager.class);
            if (bm != null) {
                level = bm.getIntProperty(android.os.BatteryManager.BATTERY_PROPERTY_CAPACITY);
                status = bm.getIntProperty(android.os.BatteryManager.BATTERY_PROPERTY_STATUS);
            }
        }
        boolean charging = status == android.os.BatteryManager.BATTERY_STATUS_CHARGING || status == android.os.BatteryManager.BATTERY_STATUS_FULL;
        if (level >= 0 && level <= 100) System.out.println("battery " + level + " " + (charging ? 1 : 0));
        android.os.PowerManager pm = ctx.getSystemService(android.os.PowerManager.class);
        if (pm != null) System.out.println("powersave " + (pm.isPowerSaveMode() ? 1 : 0));
    }

    /** The package this process runs as (its uid's first package). */
    static String packageOf() throws Exception {
        Class<?> ag = Class.forName("android.app.AppGlobals");
        Object pmgr = ag.getMethod("getPackageManager").invoke(null);
        String[] pkgs = (String[]) pmgr.getClass().getMethod("getPackagesForUid", int.class).invoke(pmgr, Process.myUid());
        return pkgs != null && pkgs.length > 0 ? pkgs[0] : "com.termux";
    }

    /** Cromite opens Custom Tabs only as the default browser, with its "Allow custom tab intents". */
    static final String CROMITE = "org.cromite.cromite";

    /**
     * {@code browsers} with {@code default} replaced by the phone's default browser, when it is one
     * of the others or Cromite, and left out otherwise.
     */
    static String[] withDefault(String[] browsers) {
        List<String> all = Arrays.asList(browsers);
        String d = all.contains("default") ? defaultBrowser() : null;
        java.util.ArrayList<String> out = new java.util.ArrayList<>();
        for (String b : browsers) {
            if (!b.equals("default")) out.add(b);
            else if (d != null && (all.contains(d) || d.equals(CROMITE))) out.add(d);
        }
        return out.toArray(new String[0]);
    }

    /** The package that opens web links, if one is chosen. */
    static String defaultBrowser() {
        try {
            Object pm = Class.forName("android.app.ActivityThread").getMethod("getPackageManager").invoke(null);
            Intent i = new Intent(Intent.ACTION_VIEW, android.net.Uri.parse("http://example.com/"));
            i.addCategory(Intent.CATEGORY_BROWSABLE);
            for (Method m : pm.getClass().getMethods()) {
                Class<?>[] t = m.getParameterTypes();
                if (!m.getName().equals("resolveIntent") || t.length != 4) continue;
                // The flags are an int before Android 13, a long since.
                Object flags = t[2] == long.class ? (Object) (long) android.content.pm.PackageManager.MATCH_DEFAULT_ONLY
                        : (Object) android.content.pm.PackageManager.MATCH_DEFAULT_ONLY;
                android.content.pm.ResolveInfo r = (android.content.pm.ResolveInfo) m.invoke(pm, i, null, flags, userId());
                return r != null && r.activityInfo != null ? r.activityInfo.packageName : null;
            }
        } catch (Exception e) {
            System.err.println("default browser: " + e);
        }
        return null;
    }

    /**
     * A Custom Tab of {@code browser} showing {@code url}, without a session (see open). Its bar,
     * until the first tap takes the whole screen: the page's title (the program's name) rather
     * than the URL, in the page's colour, without a share button.
     */
    static Intent windowIntent(String url, String browser, int flags) {
        Intent i = new Intent(Intent.ACTION_VIEW, android.net.Uri.parse(url));
        i.setPackage(browser);
        i.addFlags(flags);
        android.os.Bundle extras = new android.os.Bundle();
        extras.putBinder("android.support.customtabs.extra.SESSION", null);
        extras.putInt("android.support.customtabs.extra.TITLE_VISIBILITY", 1);
        extras.putInt("android.support.customtabs.extra.TOOLBAR_COLOR", 0xFF0F1720);
        extras.putInt("androidx.browser.customtabs.extra.SHARE_STATE", 2);
        i.putExtras(extras);
        return i;
    }

    /** A pinned shortcut cannot be taken off the home screen by its app, only disabled. */
    static void remove(String pkg, String id) throws Exception {
        call(service(), "disableShortcuts", pkg, Arrays.asList(id), "Removed with lindroids remove", 0, userId());
    }
}
