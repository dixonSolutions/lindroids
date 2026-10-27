package probe;

import android.app.ActivityManager;
import android.graphics.Bitmap;
import android.os.IBinder;

import java.io.FileOutputStream;
import java.lang.reflect.Method;
import java.util.List;

/**
 * What Recents draws for each task: the task's own activity (its app) and the TaskDescription
 * that activity set (label, icon). Run as adb's shell (it may list every task):
 *
 *   adb push taskprobe.dex /data/local/tmp/
 *   adb shell CLASSPATH=/data/local/tmp/taskprobe.dex app_process /system/bin probe.TaskProbe
 *
 * Each task's icon is written to /data/local/tmp/task-<id>.png.
 */
public final class TaskProbe {
    public static void main(String[] a) throws Exception {
        Class<?> sm = Class.forName("android.os.ServiceManager");
        IBinder b = (IBinder) sm.getMethod("getService", String.class).invoke(null, "activity_task");
        Object atm = Class.forName("android.app.IActivityTaskManager$Stub").getMethod("asInterface", IBinder.class).invoke(null, b);
        Method m = null;
        for (Method x : atm.getClass().getMethods()) if (x.getName().equals("getRecentTasks")) m = x;
        Object slice = m.invoke(atm, 20, 0, 0);
        List<?> tasks = (List<?>) slice.getClass().getMethod("getList").invoke(slice);
        for (Object o : tasks) {
            ActivityManager.RecentTaskInfo t = (ActivityManager.RecentTaskInfo) o;
            ActivityManager.TaskDescription d = t.taskDescription;
            String label = d == null ? null : d.getLabel();
            Bitmap icon = null;
            if (d != null) {
                try { icon = (Bitmap) d.getClass().getMethod("getInMemoryIcon").invoke(d); } catch (Exception ignored) { }
            }
            String file = "-";
            if (icon != null) {
                file = "/data/local/tmp/task-" + t.taskId + ".png";
                try (FileOutputStream f = new FileOutputStream(file)) { icon.compress(Bitmap.CompressFormat.PNG, 100, f); }
            }
            int res = -1;
            if (d != null) {
                try { res = (Integer) d.getClass().getMethod("getIconResource").invoke(d); } catch (Exception ignored) { }
            }
            String resPkg = null;
            if (d != null) {
                try { resPkg = (String) d.getClass().getMethod("getIconResourcePackage").invoke(d); } catch (Exception ignored) { }
            }
            System.out.println("task " + t.taskId
                    + "\n  window's activity (owner): " + (t.baseActivity == null ? t.baseIntent.getComponent() : t.baseActivity.flattenToShortString())
                    + "\n  started by intent to:      " + t.baseIntent.getComponent()
                    + "\n  data:                      " + t.baseIntent.getDataString()
                    + "\n  TaskDescription label:     " + label
                    + "\n  TaskDescription icon:      bitmap " + (icon == null ? "none" : icon.getWidth() + "x" + icon.getHeight() + " -> " + file)
                    + ", resource " + (res > 0 ? Integer.toHexString(res) + " of " + resPkg : "none"));
        }
    }
}
