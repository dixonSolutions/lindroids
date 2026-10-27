package probe;
import android.content.Context;
import android.content.ContextWrapper;
import android.graphics.Color;
import android.os.Looper;
import android.view.View;
import android.view.WindowManager;
import android.view.WindowManager.LayoutParams;

public final class WinProbe {
    static final class As extends ContextWrapper {
        final String pkg;
        As(Context b, String p) { super(b); pkg = p; }
        @Override public String getPackageName() { return pkg; }
        @Override public String getOpPackageName() { return pkg; }
        public String getBasePackageName() { return pkg; }
        @Override public Context getApplicationContext() { return this; }
    }
    static Context sys() throws Exception {
        Class<?> at = Class.forName("android.app.ActivityThread");
        Object t = at.getMethod("systemMain").invoke(null);
        return (Context) at.getMethod("getSystemContext").invoke(t);
    }
    public static void main(String[] a) throws Exception {
        Looper.prepareMainLooper();
        Context c = new As(sys(), "com.termux");
        WindowManager wm = (WindowManager) c.getSystemService(Context.WINDOW_SERVICE);
        int[] types = {LayoutParams.TYPE_APPLICATION, LayoutParams.TYPE_TOAST,
                LayoutParams.TYPE_APPLICATION_OVERLAY, LayoutParams.TYPE_APPLICATION_PANEL};
        String[] names = {"TYPE_APPLICATION", "TYPE_TOAST", "TYPE_APPLICATION_OVERLAY", "TYPE_APPLICATION_PANEL"};
        int only = a.length > 0 ? Integer.parseInt(a[0]) : -1;
        for (int i = 0; i < types.length; i++) {
            if (only >= 0 && i != only) continue;
            View v = new View(c); v.setBackgroundColor(Color.RED);
            LayoutParams lp = new LayoutParams(300, 300, types[i], 0, -1);
            try { wm.addView(v, lp); System.out.println(names[i] + ": ADDED"); wm.removeViewImmediate(v); }
            catch (Throwable e) { System.out.println(names[i] + ": " + e.getClass().getSimpleName() + ": " + e.getMessage()); }
        }
        System.exit(0);
    }
}
