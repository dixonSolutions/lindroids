package probe;

import android.content.ComponentName;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;
import android.os.IBinder;
import android.os.Process;
import java.lang.reflect.Method;

/**
 * Start an activity as the terminal app (app_process with its uid), with extras `am` cannot
 * express (a null Binder, as Custom Tabs need).
 *
 *   Open <pkg-of-caller> <uri> [-p pkg] [-n pkg/cls] [-a action] [-f hexflags]
 *        [--es k v] [--ez k true|false] [--ei k n] [--null-binder k] [--cct]
 */
public final class Open {
    public static void main(String[] a) throws Exception {
        String caller = a[0];
        Intent i = new Intent(Intent.ACTION_VIEW, Uri.parse(a[1]));
        Bundle extras = new Bundle();
        for (int k = 2; k < a.length; k++) {
            switch (a[k]) {
                case "-p": i.setPackage(a[++k]); break;
                case "-n": i.setComponent(ComponentName.unflattenFromString(a[++k])); break;
                case "-a": i.setAction(a[++k]); break;
                case "-f": i.addFlags((int) Long.parseLong(a[++k].replace("0x", ""), 16)); break;
                case "--es": extras.putString(a[++k], a[++k]); break;
                case "--ez": extras.putBoolean(a[++k], Boolean.parseBoolean(a[++k])); break;
                case "--ei": extras.putInt(a[++k], Integer.parseInt(a[++k])); break;
                case "--null-binder": extras.putBinder(a[++k], null); break;
                case "--cct": extras.putBinder("android.support.customtabs.extra.SESSION", null); break;
                default: throw new IllegalArgumentException(a[k]);
            }
        }
        i.putExtras(extras);
        if ((i.getFlags() & Intent.FLAG_ACTIVITY_NEW_TASK) == 0) i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);

        Class<?> sm = Class.forName("android.os.ServiceManager");
        IBinder b = (IBinder) sm.getMethod("getService", String.class).invoke(null, "activity_task");
        String stubName = "android.app.IActivityTaskManager$Stub";
        if (b == null) {
            b = (IBinder) sm.getMethod("getService", String.class).invoke(null, "activity");
            stubName = "android.app.IActivityManager$Stub";
        }
        Object svc = Class.forName(stubName).getMethod("asInterface", IBinder.class).invoke(null, b);
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
        Object r = m.invoke(svc, args);
        System.out.println("result " + r + " (uid " + Process.myUid() + ")");
        System.exit(0);
    }
}
