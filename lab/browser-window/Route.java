package probe;

import android.app.ActivityManager;
import android.content.ComponentName;
import android.content.Intent;
import android.graphics.Bitmap;
import android.graphics.Color;
import android.net.Uri;
import android.os.Binder;
import android.os.Bundle;
import android.os.IBinder;
import android.os.Process;

import java.lang.reflect.Method;
import java.util.List;

/**
 * Routes by which the terminal app (app_process with its uid; no permission, no root) could put a
 * program's icon on a window's card in Recents. The terminal app's package is found from this
 * process' uid, so it works for Termux, its forks or any other terminal app.
 *
 *   app_process -Djava.class.path=route.dex /system/bin probe.Route <command>
 *     pkg                              the package this process runs as
 *     apptasks                         that package's own tasks (IActivityTaskManager.getAppTasks)
 *     doc <pkg/cls> <hexflags> [url]   starts one of the terminal app's activities (waits)
 *     intask <task> <browser> <url>    a Custom Tab of browser started INTO that task (IAppTask.startActivity)
 *     launchtask <task> <browser> <url> a Custom Tab started into that task (ActivityOptions.setLaunchTaskId), and startActivityFromRecents
 *     settd <label> <hexcolor>         TaskDescription without an activity token (IActivityClientController),
 *                                      then whether any of the package's tasks took the label
 *     addtask <pkg/cls> <label>        a Recents entry with its own TaskDescription (addAppTask, no token)
 *     remove <task>                    IAppTask.finishAndRemoveTask
 */
public final class Route {
    public static void main(String[] a) throws Exception {
        quietAppOps();
        String pkg = packageOf();
        Object atm = atm();
        switch (a[0]) {
            case "pkg":
                System.out.println(pkg + " uid " + Process.myUid());
                break;
            case "apptasks":
                for (Object t : appTasks(atm, pkg)) describe(t);
                break;
            case "doc": {
                Intent i = new Intent(Intent.ACTION_MAIN);
                if (a.length > 3) i = new Intent(Intent.ACTION_VIEW, Uri.parse(a[3]));
                i.setComponent(ComponentName.unflattenFromString(a[1]));
                i.addFlags((int) Long.parseLong(a[2], 16));
                Object r = call(atm, "startActivityAndWait", pkg, i);
                print("doc", r);
                break;
            }
            case "intask": {
                int id = Integer.parseInt(a[1]);
                Object task = null;
                for (Object t : appTasks(atm, pkg)) if (info(t).taskId == id) task = t;
                if (task == null) throw new IllegalStateException("no task " + id + " of " + pkg);
                Intent i = new Intent(Intent.ACTION_VIEW, Uri.parse(a[3]));
                i.setPackage(a[2]);
                Bundle e = new Bundle();
                e.putBinder("android.support.customtabs.extra.SESSION", null);
                e.putInt("android.support.customtabs.extra.TITLE_VISIBILITY", 1);
                i.putExtras(e);
                // startActivity(IBinder whoThread, String callingPackage, String featureId, Intent, String resolvedType, Bundle)
                Method m = method(task, "startActivity");
                Object[] args = new Object[m.getParameterTypes().length];
                boolean pkgSet = false;
                for (int k = 0; k < args.length; k++) {
                    Class<?> t = m.getParameterTypes()[k];
                    if (t == Intent.class) args[k] = i;
                    else if (t == String.class && !pkgSet) { args[k] = pkg; pkgSet = true; }
                }
                try {
                    print("intask", invoke(m, task, args));
                } catch (Exception x) {
                    System.out.println("intask: " + cause(x));
                }
                break;
            }
            case "launchtask": {
                // ActivityOptions.setLaunchTaskId: the start goes into that task (as Recents does).
                Intent i = new Intent(Intent.ACTION_VIEW, Uri.parse(a[3]));
                i.setPackage(a[2]);
                Bundle e = new Bundle();
                e.putBinder("android.support.customtabs.extra.SESSION", null);
                i.putExtras(e);
                i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                android.app.ActivityOptions o = android.app.ActivityOptions.makeBasic();
                o.getClass().getMethod("setLaunchTaskId", int.class).invoke(o, Integer.parseInt(a[1]));
                try {
                    print("launchtask", call(atm, "startActivityAndWait", pkg, i, o.toBundle()));
                } catch (Exception x) {
                    System.out.println("launchtask: " + cause(x));
                }
                try {
                    print("startActivityFromRecents", call(atm, "startActivityFromRecents", Integer.parseInt(a[1]), (Bundle) null));
                } catch (Exception x) {
                    System.out.println("startActivityFromRecents: " + cause(x));
                }
                break;
            }
            case "settd": {
                ActivityManager.TaskDescription td = new ActivityManager.TaskDescription(a[1], icon((int) Long.parseLong(a[2], 16)));
                Object acc = atm.getClass().getMethod("getActivityClientController").invoke(atm);
                for (IBinder token : new IBinder[] {null, new Binder()}) {
                    try {
                        call(acc, "setTaskDescription", token, td);
                        System.out.println("settd token " + token + ": returned");
                    } catch (Exception e) {
                        System.out.println("settd token " + token + ": " + cause(e));
                    }
                }
                Thread.sleep(500);
                for (Object t : appTasks(atm, pkg)) {
                    ActivityManager.TaskDescription d = info(t).taskDescription;
                    System.out.println("settd: task " + info(t).taskId + " label now " + (d == null ? null : d.getLabel()));
                }
                try {
                    System.out.println("getActivityTokenBelow(new Binder()): " + call(acc, "getActivityTokenBelow", new Binder()));
                } catch (Exception e) {
                    System.out.println("getActivityTokenBelow: " + cause(e));
                }
                break;
            }
            case "addtask": {
                Intent i = new Intent(Intent.ACTION_MAIN);
                i.setComponent(ComponentName.unflattenFromString(a[1]));
                ActivityManager.TaskDescription td = new ActivityManager.TaskDescription(a[2], icon(0xFF2E7D32));
                Object size = call(atm, "getAppTaskThumbnailSize");
                android.graphics.Point p = (android.graphics.Point) size;
                Bitmap thumb = Bitmap.createBitmap(Math.max(1, p.x), Math.max(1, p.y), Bitmap.Config.ARGB_8888);
                for (IBinder token : new IBinder[] {null, new Binder()}) {
                    try {
                        System.out.println("addAppTask token " + token + ": " + call(atm, "addAppTask", token, i, td, thumb));
                    } catch (Exception e) {
                        System.out.println("addAppTask token " + token + ": " + cause(e));
                    }
                }
                break;
            }
            case "remove": {
                int id = Integer.parseInt(a[1]);
                for (Object t : appTasks(atm, pkg)) if (info(t).taskId == id) {
                    invoke(method(t, "finishAndRemoveTask"), t);
                    System.out.println("removed " + id);
                }
                break;
            }
            default:
                throw new IllegalArgumentException(a[0]);
        }
        System.exit(0);
    }

    static Bitmap icon(int color) {
        Bitmap b = Bitmap.createBitmap(192, 192, Bitmap.Config.ARGB_8888);
        b.eraseColor(color);
        return b;
    }

    static List<?> appTasks(Object atm, String pkg) throws Exception {
        List<?> r = (List<?>) call(atm, "getAppTasks", pkg);
        java.util.ArrayList<Object> out = new java.util.ArrayList<>();
        Method as = Class.forName("android.app.IAppTask$Stub").getMethod("asInterface", IBinder.class);
        for (Object b : r) out.add(as.invoke(null, b));
        return out;
    }

    static ActivityManager.RecentTaskInfo info(Object appTask) throws Exception {
        return (ActivityManager.RecentTaskInfo) invoke(method(appTask, "getTaskInfo"), appTask);
    }

    static void describe(Object appTask) throws Exception {
        ActivityManager.RecentTaskInfo t = info(appTask);
        ActivityManager.TaskDescription d = t.taskDescription;
        System.out.println("app task " + t.taskId + " base " + t.baseIntent.getComponent() + " baseActivity " + t.baseActivity
                + " top " + t.topActivity + " activities " + t.numActivities
                + " td-label " + (d == null ? null : d.getLabel()));
    }

    static void print(String what, Object r) throws Exception {
        if (r != null && r.getClass().getName().equals("android.app.WaitResult")) {
            Object who = r.getClass().getField("who").get(r);
            Object res = r.getClass().getField("result").get(r);
            System.out.println(what + ": result " + res + " who " + who);
        } else {
            System.out.println(what + ": " + r);
        }
    }

    static Object atm() throws Exception {
        IBinder b = (IBinder) Class.forName("android.os.ServiceManager").getMethod("getService", String.class).invoke(null, "activity_task");
        return Class.forName("android.app.IActivityTaskManager$Stub").getMethod("asInterface", IBinder.class).invoke(null, b);
    }

    static String packageOf() throws Exception {
        Object pm = Class.forName("android.app.AppGlobals").getMethod("getPackageManager").invoke(null);
        String[] p = (String[]) pm.getClass().getMethod("getPackagesForUid", int.class).invoke(pm, Process.myUid());
        return p[0];
    }

    static Method method(Object o, String name) {
        for (Method m : o.getClass().getMethods()) if (m.getName().equals(name)) return m;
        throw new IllegalStateException("no " + name + " on " + o.getClass());
    }

    static Object invoke(Method m, Object o, Object... args) throws Exception {
        Class<?>[] t = m.getParameterTypes();
        Object[] full = new Object[t.length];
        for (int k = 0; k < t.length; k++) {
            full[k] = k < args.length ? args[k] : null;
            if (full[k] == null && t[k] == int.class) full[k] = 0;
            if (full[k] == null && t[k] == boolean.class) full[k] = false;
        }
        try {
            return m.invoke(o, full);
        } catch (java.lang.reflect.InvocationTargetException e) {
            if (String.valueOf(e.getCause()).contains("SyncNotedAppOp")) return "(reply lost: SyncNotedAppOp)";
            throw e;
        }
    }

    /** Fills a binder method's parameters by type from values, in order (null where none fits). */
    static Object call(Object svc, String name, Object... values) throws Exception {
        Method m = null;
        for (Method x : svc.getClass().getMethods()) if (x.getName().equals(name)) m = x;
        if (m == null) throw new IllegalStateException("no " + name);
        Class<?>[] types = m.getParameterTypes();
        Object[] args = new Object[types.length];
        boolean[] used = new boolean[values.length];
        for (int i = 0; i < types.length; i++) {
            for (int k = 0; k < values.length; k++) {
                Object v = values[k];
                if (!used[k] && (v == null ? !types[i].isPrimitive() && k == i : types[i].isInstance(v) || (types[i] == int.class && v instanceof Integer))) {
                    args[i] = v;
                    used[k] = true;
                    break;
                }
            }
            if (args[i] == null && types[i] == int.class) args[i] = 0;
            if (args[i] == null && types[i] == boolean.class) args[i] = false;
        }
        try {
            return m.invoke(svc, args);
        } catch (java.lang.reflect.InvocationTargetException e) {
            if (String.valueOf(e.getCause()).contains("SyncNotedAppOp")) return "(reply lost: SyncNotedAppOp)";
            throw e;
        }
    }

    static String cause(Throwable e) {
        // The exception the system threw (its cause is only the remote stack trace).
        while (e instanceof java.lang.reflect.InvocationTargetException && e.getCause() != null) e = e.getCause();
        return e.getClass().getName() + ": " + String.valueOf(e.getMessage()).split("\n")[0];
    }

    static void quietAppOps() {
        try {
            Object never = Class.forName("com.android.internal.app.MessageSamplingConfig")
                    .getConstructor(int.class, int.class, long.class).newInstance(-1, 0, Long.MAX_VALUE);
            java.lang.reflect.Field f = android.app.AppOpsManager.class.getDeclaredField("sConfig");
            f.setAccessible(true);
            f.set(null, never);
        } catch (Throwable ignored) { }
    }
}
