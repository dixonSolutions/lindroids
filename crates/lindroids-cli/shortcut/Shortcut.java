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
 *   app_process ... lindroids.Shortcut clip &lt;package&gt; &lt;text file&gt;
 *   app_process ... lindroids.Shortcut device
 *   app_process ... lindroids.Shortcut app-browsers
 *   app_process ... lindroids.Shortcut apps-for &lt;url&gt;
 *   app_process ... lindroids.Shortcut view &lt;package&gt; &lt;target package&gt; &lt;url&gt;
 *   app_process ... lindroids.Shortcut app-settings &lt;package&gt; &lt;app package&gt;
 *   app_process ... lindroids.Shortcut app-icons &lt;icon.png&gt; &lt;maskable.png&gt; &lt;plain.png&gt;
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
            quietAppOps();
            if (a.length >= 9 && a[0].equals("pin")) {
                pin(a[1], a[2], a[3], a[4], a[5], a[6], a[7], a[8]);
            } else if (a.length >= 3 && a[0].equals("remove")) {
                remove(a[1], a[2]);
            } else if (a.length >= 5 && a[0].equals("open")) {
                System.out.println("browser " + open(a[1], a[2], a[3], Arrays.copyOfRange(a, 4, a.length)));
            } else if (a.length >= 9 && (a[0].equals("notify") || a[0].equals("running"))) {
                post(a[1], Integer.parseInt(a[2]), a[3], a[4], a[5], a[6], a[7], a[8], Arrays.copyOfRange(a, 9, a.length), a[0].equals("running"));
            } else if (a.length >= 3 && a[0].equals("clip")) {
                clip(a[1], a[2]);
            } else if (a.length >= 1 && a[0].equals("device")) {
                device();
            } else if (a.length >= 3 && a[0].equals("pinned")) {
                System.out.println("pinned " + pinned(service(), a[1], a[2]));
            } else if (a.length >= 3 && a[0].equals("cancel")) {
                cancel(a[1], Integer.parseInt(a[2]));
            } else if (a.length >= 1 && a[0].equals("app-browsers")) {
                appBrowsers();
            } else if (a.length >= 1 && a[0].equals("system-browsers")) {
                systemBrowsers();
            } else if (a.length >= 1 && a[0].equals("diag")) {
                diag();
            } else if (a.length >= 1 && a[0].equals("default-browser")) {
                String d = defaultBrowser();
                System.out.println("default " + (d == null ? "none" : d));
            } else if (a.length >= 2 && a[0].equals("apps-for")) {
                for (String p : appsFor(a[1])) System.out.println("app " + p);
            } else if (a.length >= 4 && a[0].equals("view")) {
                view(a[1], a[2], a[3]);
            } else if (a.length >= 3 && a[0].equals("app-settings")) {
                appSettings(a[1], a[2]);
            } else if (a.length >= 4 && a[0].equals("app-icons")) {
                appIcons(a[1], a[2], a[3]);
            } else {
                throw new IllegalArgumentException("usage: pin <package> <id> <label> <icon> <action> <component> <type> <text> | remove <package> <id>"
                        + " | open <package> new|raise <url> <browser>... | notify|running <package> <id> <program> <title> <text> <icon> <small-icon> <url> <browser>..."
                        + " | cancel <package> <id> | pinned <package> <id> | clip <package> <file> | device"
                        + " | app-browsers | system-browsers | diag | default-browser | apps-for <url> | view <package> <target> <url> | app-settings <package> <app> | app-icons <icon> <maskable> <plain>");
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

    /** Puts the text in {@code path} on Android's clipboard, as the terminal app (writing is allowed in the background). */
    static void clip(String pkg, String path) throws Exception {
        String text = new String(java.nio.file.Files.readAllBytes(java.nio.file.Paths.get(path)), java.nio.charset.StandardCharsets.UTF_8);
        IBinder b = (IBinder) Class.forName("android.os.ServiceManager").getMethod("getService", String.class).invoke(null, "clipboard");
        if (b == null) throw new IllegalStateException("no clipboard service");
        Object svc = Class.forName("android.content.IClipboard$Stub").getMethod("asInterface", IBinder.class).invoke(null, b);
        call(svc, "setPrimaryClip", android.content.ClipData.newPlainText("lindroids", text), pkg, userId());
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

    /**
     * Stops this process asking for the app ops a call notes. Android 11+ starts every process
     * sampling them; the system then puts them in front of the reply, and reading them throws in a
     * process that is not an app (SyncNotedAppOp's checks), losing the reply (see call): what
     * startActivityAndWait opened, for one. A sampling config that never expires turns it off.
     */
    static void quietAppOps() {
        try {
            Object never = Class.forName("com.android.internal.app.MessageSamplingConfig")
                    .getConstructor(int.class, int.class, long.class).newInstance(-1, 0, Long.MAX_VALUE);
            java.lang.reflect.Field f = android.app.AppOpsManager.class.getDeclaredField("sConfig");
            f.setAccessible(true);
            f.set(null, never);
        } catch (Throwable e) {
            // Before Android 11 nothing is sampled.
        }
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
        if (!type.isEmpty()) i.setType(type);
        String[] cn = receiver(component, pkg, i).split("/", 2);
        i.setClassName(cn[0], cn[1].startsWith(".") ? cn[0] + cn[1] : cn[1]);
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

    /**
     * The activity a shortcut sends {@code probe} to: the first of {@code candidates}
     * (comma-separated {@code package/class}) that is installed and may be started from outside,
     * else whichever activity of {@code pkg} takes the intent. Termux renamed its share receiver
     * (0.119: {@code .app.api.file.FileShareReceiverActivity}); a shortcut to a class that is not
     * there is one the launcher refuses ("Can't open this app"). With no match the first
     * candidate is used as it is.
     */
    static String receiver(String candidates, String pkg, Intent probe) {
        String[] list = candidates.split(",");
        try {
            Object pm = Class.forName("android.app.ActivityThread").getMethod("getPackageManager").invoke(null);
            for (String c : list) {
                String[] cn = c.split("/", 2);
                if (cn.length < 2) continue;
                String cls = cn[1].startsWith(".") ? cn[0] + cn[1] : cn[1];
                for (Method m : pm.getClass().getMethods()) {
                    Class<?>[] t = m.getParameterTypes();
                    if (!m.getName().equals("getActivityInfo") || t.length != 3) continue;
                    android.content.pm.ActivityInfo ai = (android.content.pm.ActivityInfo) m.invoke(pm,
                            new android.content.ComponentName(cn[0], cls), t[1] == long.class ? (Object) 0L : (Object) 0, userId());
                    if (ai != null && ai.exported) return cn[0] + "/" + cls;
                }
            }
            Intent q = new Intent(probe);
            q.setPackage(pkg);
            for (Method m : pm.getClass().getMethods()) {
                Class<?>[] t = m.getParameterTypes();
                if (!m.getName().equals("resolveIntent") || t.length != 4) continue;
                Object flags = t[2] == long.class ? (Object) (long) android.content.pm.PackageManager.MATCH_DEFAULT_ONLY
                        : (Object) android.content.pm.PackageManager.MATCH_DEFAULT_ONLY;
                android.content.pm.ResolveInfo r = (android.content.pm.ResolveInfo) m.invoke(pm, q, probe.getType(), flags, userId());
                if (r != null && r.activityInfo != null) return r.activityInfo.packageName + "/" + r.activityInfo.name;
            }
        } catch (Exception e) {
            System.err.println("receiver: " + e);
        }
        return list[0];
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
     * {@code new} always makes a new window; {@code raise} brings back the window that has this URL;
     * {@code first} opens this URL's window for the first time. With {@code new} and {@code first},
     * a browser that opens an ordinary tab instead is passed over (the first such one is kept if
     * none opens a window). A Custom Tab needs a session extra holding a Binder, which `am` cannot send;
     * a null one asks for a tab without a session. Returns the browser used.
     */
    static String open(String pkg, String mode, String url, String[] browsers) throws Exception {
        Object atm = taskManager();
        int flags = Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_NEW_DOCUMENT;
        if (mode.equals("new")) flags |= Intent.FLAG_ACTIVITY_MULTIPLE_TASK;
        String last = "no browser given", tab = null;
        boolean strict = !mode.equals("raise");
        boolean wait = hasMethod(atm, "startActivityAndWait") && userId() == 0;
        for (String browser : withDefault(browsers)) {
            // An installed web app (`export --webapk`) is a window of its own whatever it opens.
            boolean app = !isBrowser(browser);
            Intent i = windowIntent(url, browser, flags);
            Object r = wait && !app ? call(atm, "startActivityAndWait", pkg, i) : call(atm, "startActivity", pkg, i);
            String opened = null;
            if (r != null && r.getClass().getName().equals("android.app.WaitResult")) {
                opened = window(r, strict);
                r = r.getClass().getField("result").get(r);
            }
            // ActivityManager.START_SUCCESS and the other non-negative codes (delivered to the top,
            // task brought to the front) all mean the browser took it; no answer (see call) means
            // the start went through too.
            if (r != null && !(r instanceof Integer && (Integer) r >= 0)) {
                last = browser + ": start result " + r;
                continue;
            }
            if (opened == null) return browser;
            // An ordinary tab: the browser has no Custom Tabs, or has them turned off (Cromite
            // without "Allow custom tab intents"). On to the next; the page in that tab closes
            // itself when the window in the next one links (the server tells it to stand down).
            System.err.println(browser + ": " + opened + " is not a window of its own");
            last = browser + ": an ordinary tab (" + opened + ")";
            if (tab == null) tab = browser;
            // The answer can come before the tab is up (it names the browser's trampoline): let
            // it come up now, under the window the next browser opens, not over it later. (Sending
            // the window's intent again afterwards is no way out: Firefox loads the page again.)
            Thread.sleep(1500);
        }
        if (tab != null) return tab;
        throw new IllegalStateException(last);
    }

    /**
     * What a start that waited ({@code startActivityAndWait}) opened, if it is not a window of its
     * own: null for a Custom Tab, else the activity. When a window was there already
     * ({@code strict} false), also null when nothing new was launched: the window brought to the
     * front through the browser's trampoline activity, which is all the answer names then. A
     * browser that adds an ordinary tab to its running activity answers the same way (Cromite:
     * its ChromeLauncherActivity), so a first window must name a Custom Tab.
     */
    static String window(Object waitResult, boolean strict) throws Exception {
        android.content.ComponentName who = (android.content.ComponentName) waitResult.getClass().getField("who").get(waitResult);
        if (who == null || who.getClassName().toLowerCase().contains("customtab")) return null;
        if (strict) return who.flattenToShortString();
        try {
            // WaitResult.LAUNCH_STATE_UNKNOWN (Android 10+): delivered to what was there.
            if (waitResult.getClass().getField("launchState").getInt(waitResult) == 0) return null;
        } catch (NoSuchFieldException e) {
            // Before Android 10: START_TASK_TO_FRONT, START_DELIVERED_TO_TOP.
            int result = waitResult.getClass().getField("result").getInt(waitResult);
            if (result == 2 || result == 3) return null;
        }
        return who.flattenToShortString();
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
        return systemContext().createPackageContext(pkg, Context.CONTEXT_IGNORE_SECURITY);
    }

    /**
     * The system Context app_process can make. Package lookups go through its PackageManager;
     * Android may still hide a browser from queries (Chrome on a Samsung tablet), which is why
     * known browsers are opened by explicit intent and looked for on the system image too.
     */
    static Context systemContext() throws Exception {
        Class<?> at = Class.forName("android.app.ActivityThread");
        Object thread = at.getMethod("systemMain").invoke(null);
        return (Context) at.getMethod("getSystemContext").invoke(thread);
    }

    static android.content.pm.PackageManager packageManager() throws Exception {
        return systemContext().getPackageManager();
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
        String[] pkgs = packageManager().getPackagesForUid(Process.myUid());
        return pkgs != null && pkgs.length > 0 ? pkgs[0] : "com.termux";
    }

    /**
     * {@code browsers} with {@code default} replaced by the phone's default browser when it is one
     * of the others and not Firefox (its windows cannot show the program in Recents or close
     * themselves, so a Chromium browser further down the list goes before it), and left out
     * otherwise.
     */
    static String[] withDefault(String[] browsers) {
        List<String> all = Arrays.asList(browsers);
        String d = all.contains("default") ? defaultBrowser() : null;
        java.util.LinkedHashSet<String> out = new java.util.LinkedHashSet<>();
        for (String b : browsers) {
            if (!b.equals("default")) out.add(b);
            else if (d != null && all.contains(d) && !d.startsWith("org.mozilla.")) out.add(d);
        }
        return out.toArray(new String[0]);
    }

    /**
     * {@code url} naming the browser it is sent to ({@code &via=<package>}, before the fragment):
     * the server keeps the page in the window over a copy in the same browser, and over an
     * ordinary tab another browser opened before. The same browser always gets the same URL, so
     * sending it again brings that window back.
     */
    static String withVia(String url, String browser) {
        int hash = url.indexOf('#');
        String base = hash < 0 ? url : url.substring(0, hash), fragment = hash < 0 ? "" : url.substring(hash);
        if (!base.contains("?")) return url;
        return base + "&via=" + browser + fragment;
    }

    /** The package that opens web links, if one is chosen. */
    static String defaultBrowser() {
        try {
            Intent i = new Intent(Intent.ACTION_VIEW, android.net.Uri.parse("http://example.com/"));
            i.addCategory(Intent.CATEGORY_BROWSABLE);
            android.content.pm.ResolveInfo r = packageManager().resolveActivity(i, android.content.pm.PackageManager.MATCH_DEFAULT_ONLY);
            return r != null && r.activityInfo != null ? r.activityInfo.packageName : null;
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
        Intent i = new Intent(Intent.ACTION_VIEW, android.net.Uri.parse(withVia(url, browser)));
        i.setPackage(browser);
        if (!isBrowser(browser)) {
            // An installed web app: its own task already, which takes its URLs.
            i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            return i;
        }
        i.addFlags(flags);
        android.os.Bundle extras = new android.os.Bundle();
        extras.putBinder("android.support.customtabs.extra.SESSION", null);
        extras.putInt("android.support.customtabs.extra.TITLE_VISIBILITY", 1);
        extras.putInt("android.support.customtabs.extra.TOOLBAR_COLOR", 0xFF0F1720);
        extras.putInt("androidx.browser.customtabs.extra.SHARE_STATE", 2);
        i.putExtras(extras);
        return i;
    }

    /** Android's activity task manager (the activity manager before Android 10). */
    static Object taskManager() throws Exception {
        Class<?> sm = Class.forName("android.os.ServiceManager");
        IBinder b = (IBinder) sm.getMethod("getService", String.class).invoke(null, "activity_task");
        String stub = "android.app.IActivityTaskManager$Stub";
        if (b == null) { // before Android 10
            b = (IBinder) sm.getMethod("getService", String.class).invoke(null, "activity");
            stub = "android.app.IActivityManager$Stub";
        }
        return Class.forName(stub).getMethod("asInterface", IBinder.class).invoke(null, b);
    }

    /** The packages with an activity that opens {@code url} from a link (BROWSABLE, any of them: MATCH_ALL). */
    static java.util.Set<String> viewers(String url) throws Exception {
        Intent i = new Intent(Intent.ACTION_VIEW, android.net.Uri.parse(url));
        i.addCategory(Intent.CATEGORY_BROWSABLE);
        List<android.content.pm.ResolveInfo> list = packageManager().queryIntentActivities(i, android.content.pm.PackageManager.MATCH_ALL);
        java.util.Set<String> out = new java.util.LinkedHashSet<>();
        for (android.content.pm.ResolveInfo ri : list) {
            if (ri.activityInfo != null) out.add(ri.activityInfo.packageName);
        }
        return out;
    }

    static java.util.Set<String> browserPackages;

    /** The browsers windows are opened in (the CLI's list). Known even when Android hides them from queries. */
    static final String[] KNOWN_BROWSERS = {
        "com.android.chrome", "com.microsoft.emmx", "com.brave.browser", "com.vivaldi.browser",
        "com.sec.android.app.sbrowser", "com.chrome.beta", "org.chromium.chrome", "org.cromite.cromite",
        "org.mozilla.firefox", "org.mozilla.firefox_beta", "org.mozilla.fenix",
    };

    /** Whether {@code pkg} opens any web link (a browser), not only some site's (an installed web app). */
    static boolean isBrowser(String pkg) {
        for (String b : KNOWN_BROWSERS) {
            if (b.equals(pkg)) return true;
        }
        try {
            if (browserPackages == null) browserPackages = viewers("http://example.com/");
            return browserPackages.contains(pkg);
        } catch (Exception e) {
            System.err.println("browsers: " + e);
            return true;
        }
    }

    /**
     * The installed apps, other than browsers, that open {@code url}: an installed web app (a
     * WebAPK, which Chrome or Samsung Internet had made) whose scope holds it.
     */
    static List<String> appsFor(String url) throws Exception {
        java.util.Set<String> all = viewers(url);
        all.removeAll(viewers("http://example.com/"));
        all.removeAll(viewers("https://example.com/"));
        return new java.util.ArrayList<>(all);
    }

    static android.content.pm.PackageInfo packageInfo(String pkg, int flags) {
        try {
            return packageManager().getPackageInfo(pkg, flags);
        } catch (Exception e) {
            return null;
        }
    }

    /** Installed and not turned off. */
    static boolean usable(String pkg) {
        android.content.pm.PackageInfo p = packageInfo(pkg, 0);
        return p != null && p.applicationInfo != null && p.applicationInfo.enabled;
    }

    /**
     * Google's own Play services, not microG's stand-in (same package name, its own activities
     * under org.microg): Chrome makes installed web apps through them.
     */
    static String playServices() {
        boolean gms = usable("com.google.android.gms");
        boolean store = usable("com.android.vending") || hasViewer("market://details?id=x", "com.android.vending");
        if (!gms && !store) return "no Google Play services";
        if (!store) return "no Play Store";
        android.content.pm.PackageInfo p = packageInfo("com.google.android.gms", 1 /* GET_ACTIVITIES */);
        if (p != null && p.activities != null) {
            for (android.content.pm.ActivityInfo a : p.activities) {
                if (a.name.startsWith("org.microg.")) return "microG instead of Google Play services";
            }
        }
        return null;
    }

    /** Whether {@code pkg} opens {@code url} (for when getPackageInfo cannot see it). */
    static boolean hasViewer(String url, String pkg) {
        try {
            return viewers(url).contains(pkg);
        } catch (Exception e) {
            return false;
        }
    }

    /** The packages that open web links, by intent query: getPackageInfo may be refused. */
    static java.util.Set<String> webViewers() {
        try {
            return viewers("http://example.com/");
        } catch (Exception e) {
            return java.util.Collections.emptySet();
        }
    }

    /** The Galaxy Store, by package or by the links it opens. */
    static boolean galaxyStore() {
        return usable("com.sec.android.app.samsungapps")
                || hasViewer("samsungapps://ProductDetail/com.sec.android.app.sbrowser", "com.sec.android.app.samsungapps");
    }

    static final String[] CHROMES = {"com.android.chrome", "com.chrome.beta", "com.chrome.dev", "com.chrome.canary"};
    static final String[] SAMSUNG_BROWSERS = {"com.sec.android.app.sbrowser", "com.sec.android.app.sbrowser.beta"};

    /**
     * The installed browsers that can install a page as an app of its own (a WebAPK), a line
     * each: {@code can <package>}, or {@code cannot <package> <why>}. Only Chrome can, through
     * Google Play services, and Samsung Internet, on Samsung's phones through the Galaxy Store;
     * every other browser makes only a home screen shortcut.
     */
    static void appBrowsers() {
        String play = playServices();
        java.util.Set<String> web = webViewers();
        for (String c : CHROMES) {
            android.content.pm.PackageInfo pi = packageInfo(c, 0);
            if (pi == null && !web.contains(c)) continue;
            String why = pi != null && pi.applicationInfo != null && !pi.applicationInfo.enabled ? "turned off" : play;
            System.out.println(why == null ? "can " + c : "cannot " + c + " " + why);
        }
        boolean samsung = "samsung".equalsIgnoreCase(android.os.Build.MANUFACTURER);
        for (String c : SAMSUNG_BROWSERS) {
            android.content.pm.PackageInfo pi = packageInfo(c, 0);
            if (pi == null && !web.contains(c)) continue;
            String why = pi != null && pi.applicationInfo != null && !pi.applicationInfo.enabled ? "turned off"
                    : !samsung ? "not a Samsung phone"
                    : !galaxyStore() ? "no Galaxy Store" : null;
            System.out.println(why == null ? "can " + c : "cannot " + c + " " + why);
        }
    }

    /**
     * Known WebAPK browsers whose APK sits on the system image but which are not installed for
     * this user (a preload uninstalled for the user). Android cannot start them until Play Store
     * or the installer puts them back, but the export can say which one and how.
     * A line each: {@code system <package> <apk path>}.
     */
    static void systemBrowsers() {
        java.util.Set<String> wanted = new java.util.LinkedHashSet<>();
        wanted.addAll(Arrays.asList(CHROMES));
        wanted.addAll(Arrays.asList(SAMSUNG_BROWSERS));
        for (String d : new String[]{"/product/app", "/product/priv-app", "/system/app", "/system/priv-app", "/system_ext/app", "/system_ext/priv-app", "/vendor/app"}) {
            findSystemApks(new java.io.File(d), wanted, 0);
        }
    }

    static void findSystemApks(java.io.File d, java.util.Set<String> wanted, int depth) {
        if (d == null || depth > 3) return;
        java.io.File[] fs = d.listFiles();
        if (fs == null) return;
        for (java.io.File f : fs) {
            if (f.isDirectory()) {
                findSystemApks(f, wanted, depth + 1);
            } else if (f.getName().endsWith(".apk")) {
                try {
                    android.content.pm.PackageInfo pi = packageManager().getPackageArchiveInfo(f.getPath(), 0);
                    if (pi != null && wanted.contains(pi.packageName)) {
                        System.out.println("system " + pi.packageName + " " + f.getPath());
                    }
                } catch (Throwable t) {
                }
            }
        }
    }

    /**
     * What package lookups see on this Android: the hidden IPackageManager calls the export used
     * to make, and the public PackageManager calls it makes now. For `lindroids doctor` reports
     * (a failed lookup says why here).
     */
    static void out(String s) {
        System.out.println(s);
        System.out.flush();
    }

    static void diag() {
        String[] pkgs = {"com.termux", "com.android.chrome", "com.sec.android.app.sbrowser", "com.google.android.gms", "com.android.vending", "com.sec.android.app.samsungapps"};
        out("uid " + Process.myUid() + " user " + userId() + " " + android.os.Build.MANUFACTURER + " sdk " + android.os.Build.VERSION.SDK_INT);
        try {
            android.content.pm.PackageManager pm = packageManager();
            out("public pm " + pm.getClass().getName());
            for (String p : pkgs) {
                try {
                    out("public info " + p + " " + (pm.getPackageInfo(p, 0) == null ? "null" : "ok"));
                } catch (Throwable t) {
                    out("public info " + p + " " + cause(t));
                }
            }
            for (android.content.pm.PackageInfo pi : pm.getInstalledPackages(0)) {
                String n = pi.packageName.toLowerCase(java.util.Locale.ROOT);
                if (n.contains("chrome") || n.contains("browser") || n.contains("samsung") || n.contains("vending") || n.contains("gms")) {
                    out("public pkg " + pi.packageName + (pi.applicationInfo != null && !pi.applicationInfo.enabled ? " disabled" : ""));
                }
            }
        } catch (Throwable t) {
            out("public pm " + cause(t));
        }
        for (String url : new String[]{"http://example.com/", "market://details?id=x", "samsungapps://ProductDetail/com.sec.android.app.sbrowser"}) {
            try {
                out("public viewers " + url + " " + viewers(url));
            } catch (Throwable t) {
                out("public viewers " + url + " " + cause(t));
            }
        }
        try {
            Object pm = Class.forName("android.app.ActivityThread").getMethod("getPackageManager").invoke(null);
            out("hidden pm " + (pm == null ? "null" : pm.getClass().getName()));
            for (String p : pkgs) {
                try {
                    Object r = pm.getClass().getMethod("getPackageInfo", String.class, long.class, int.class).invoke(pm, p, 0L, userId());
                    out("hidden info " + p + " " + (r == null ? "null" : "ok"));
                } catch (Throwable t) {
                    out("hidden info " + p + " " + cause(t));
                }
            }
        } catch (Throwable t) {
            out("hidden pm " + cause(t));
        }
        for (String url : new String[]{"http://example.com/", "market://details?id=x", "samsungapps://ProductDetail/com.sec.android.app.sbrowser"}) {
            try {
                out("hidden viewers " + url + " " + viewersHidden(url));
            } catch (Throwable t) {
                out("hidden viewers " + url + " " + cause(t));
            }
        }
    }

    static String cause(Throwable t) {
        Throwable c = t;
        while (c.getCause() != null) c = c.getCause();
        return c.getClass().getSimpleName() + ": " + c.getMessage();
    }

    static java.util.Set<String> viewersHidden(String url) throws Exception {
        Object pm = Class.forName("android.app.ActivityThread").getMethod("getPackageManager").invoke(null);
        Intent i = new Intent(Intent.ACTION_VIEW, android.net.Uri.parse(url));
        i.addCategory(Intent.CATEGORY_BROWSABLE);
        Object r = pm.getClass().getMethod("queryIntentActivities", Intent.class, String.class, long.class, int.class).invoke(pm, i, null, (long) 0x20000, userId());
        java.util.Set<String> out = new java.util.LinkedHashSet<>();
        List<?> list = r instanceof List ? (List<?>) r : (List<?>) r.getClass().getMethod("getList").invoke(r);
        for (Object o : list) {
            android.content.pm.ResolveInfo ri = (android.content.pm.ResolveInfo) o;
            if (ri.activityInfo != null) out.add(ri.activityInfo.packageName);
        }
        return out;
    }

    /** Opens {@code url} in {@code target} (a browser's ordinary tab, or an installed web app), as {@code pkg}. */
    static void view(String pkg, String target, String url) throws Exception {
        Intent i = new Intent(Intent.ACTION_VIEW, android.net.Uri.parse(url));
        i.setPackage(target);
        i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        Object r = call(taskManager(), "startActivity", pkg, i);
        if (r instanceof Integer && (Integer) r < 0) throw new IllegalStateException(target + ": start result " + r);
    }

    /**
     * Android's page for {@code app} in Settings, where it is uninstalled: asking for the uninstall
     * itself (ACTION_DELETE) needs a permission the terminal app does not have.
     */
    static void appSettings(String pkg, String app) throws Exception {
        Intent i = new Intent("android.settings.APPLICATION_DETAILS_SETTINGS", android.net.Uri.parse("package:" + app));
        i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        Object r = call(taskManager(), "startActivity", pkg, i);
        if (r instanceof Integer && (Integer) r < 0) throw new IllegalStateException("settings: start result " + r);
    }

    /**
     * A web app manifest's icons, as PNGs: {@code maskable}, the icon as {@code adaptive} draws it
     * (what Android's launchers cut into their shape), and {@code plain}, the icon 512 pixels
     * square (browsers install no app whose icon is under 144).
     */
    static void appIcons(String in, String maskable, String plain) throws Exception {
        Bitmap icon = BitmapFactory.decodeFile(in);
        if (icon == null) throw new IllegalArgumentException(in + ": not an image");
        try (java.io.FileOutputStream f = new java.io.FileOutputStream(maskable)) {
            adaptive(icon).compress(Bitmap.CompressFormat.PNG, 100, f);
        }
        int size = 512;
        Bitmap out = Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888);
        float k = Math.min((float) size / icon.getWidth(), (float) size / icon.getHeight());
        int w = Math.round(icon.getWidth() * k), h = Math.round(icon.getHeight() * k);
        new Canvas(out).drawBitmap(icon, null, new Rect((size - w) / 2, (size - h) / 2, (size - w) / 2 + w, (size - h) / 2 + h),
                new Paint(Paint.FILTER_BITMAP_FLAG | Paint.ANTI_ALIAS_FLAG));
        try (java.io.FileOutputStream f = new java.io.FileOutputStream(plain)) {
            out.compress(Bitmap.CompressFormat.PNG, 100, f);
        }
    }

    /** A pinned shortcut cannot be taken off the home screen by its app, only disabled. */
    static void remove(String pkg, String id) throws Exception {
        call(service(), "disableShortcuts", pkg, Arrays.asList(id), "Removed with lindroids remove", 0, userId());
    }
}
