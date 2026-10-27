package lindroids;

import android.content.ContextWrapper;
import android.content.Intent;
import android.content.pm.ShortcutInfo;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
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
            } else {
                throw new IllegalArgumentException("usage: pin <package> <id> <label> <icon> <action> <component> <type> <text> | remove <package> <id>"
                        + " | open <package> new|raise <url> <browser>...");
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
        Object r = m.invoke(svc, args);
        if (future != null) {
            return ((Future<?>) future).get(10, TimeUnit.SECONDS);
        }
        return r;
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
        if (icon != null) b.setIcon(Icon.createWithBitmap(icon));
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
        Object r = call(svc, "requestPinShortcut", pkg, s, userId());
        if (Boolean.FALSE.equals(r)) throw new IllegalStateException("the launcher does not take pinned shortcuts");
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
        for (String browser : browsers) {
            Intent i = new Intent(Intent.ACTION_VIEW, android.net.Uri.parse(url));
            i.setPackage(browser);
            i.addFlags(flags);
            android.os.Bundle extras = new android.os.Bundle();
            extras.putBinder("android.support.customtabs.extra.SESSION", null);
            i.putExtras(extras);
            Object r = call(atm, "startActivity", pkg, i);
            // ActivityManager.START_SUCCESS and the other non-negative codes (delivered to the top,
            // task brought to the front) all mean the window is there.
            if (r instanceof Integer && (Integer) r >= 0) return browser;
            last = browser + ": start result " + r;
        }
        throw new IllegalStateException(last);
    }

    /** A pinned shortcut cannot be taken off the home screen by its app, only disabled. */
    static void remove(String pkg, String id) throws Exception {
        call(service(), "disableShortcuts", pkg, Arrays.asList(id), "Removed with lindroids remove", 0, userId());
    }
}
