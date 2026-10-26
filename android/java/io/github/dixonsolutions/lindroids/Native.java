package io.github.dixonsolutions.lindroids;

import android.view.Surface;

/** Entry points into liblindroids.so (Rust). Everything that is not Android plumbing lives there. */
final class Native {
    private Native() {}

    /**
     * Called once at app start: where bundles live, the cache (files shared by OpenFile), and the
     * screen density for the server.
     */
    static native void appInit(String filesDir, String cacheDir, float density, int screenWidth, int screenHeight);

    /**
     * Connects link {@code link} to the server listening on abstract socket {@code socket}, which
     * must belong to {@code expectedUid}. Authenticates with {@code secret} (from an earlier pairing)
     * or {@code code} (typed by the user). Returns "ok", "paired &lt;secret&gt;", "denied &lt;open&gt;
     * &lt;why&gt;", "wrong-uid &lt;uid&gt;" or "error &lt;why&gt;".
     */
    static native String linkConnect(int link, String socket, int expectedUid, String secret, String code);
    static native void linkDisconnect(int link);

    /** A window's Surface appeared, or went away (null). */
    static native void windowSurface(int link, int win, Surface surface);

    static native void linkResize(int link, int win, int width, int height, float density);
    static native void linkTouch(int link, int win, int action, int id, float x, float y, int timeMs);
    static native void linkPointer(int link, int win, int action, float x, float y, int button, int timeMs);
    static native void linkScroll(int link, int win, float dx, float dy, int timeMs);
    static native void linkKey(int link, int win, int androidKeyCode, boolean down, int timeMs);
    static native void linkText(int link, int win, String text);
    static native void linkFocus(int link, int win, boolean focused);
    static native void linkClose(int link, int win);
    static native void linkClipboard(int link, String text);
    static native void linkFolderPicked(int link, int request, String path);
    static native void linkExportResult(int link, String id, boolean ok, String message);

    // Android integration: the server's portal (see portal.rs).

    /**
     * Android's state, read by {@link SystemMonitor}; Rust debounces it and sends what changed to
     * every link. {@code accent} is ARGB; {@code batteryLevel} -1 means no battery.
     */
    static native void systemState(boolean dark, boolean hasAccent, int accent, boolean highContrast,
                                   boolean reducedMotion, float fontScale, boolean powerSaver,
                                   boolean netAvailable, boolean netInternet, boolean netValidated,
                                   boolean netCaptive, boolean netMetered,
                                   long memTotal, long memAvailable, boolean memLow,
                                   int batteryLevel, boolean charging, String locale, String timezone);
    /** onTrimMemory's level, as Android gives it. */
    static native void memoryWarning(int level);
    static native void linkOpenResult(int link, int request, boolean ok, String message);
    /** A notification was tapped ("default") or a button pressed. Returns its link, or -1. */
    static native int notificationAction(int note, String action);
    static native void notificationDismissed(int note);
    /** The picker's answer: parallel arrays, one entry per document (nulls where unknown). */
    static native void linkFilesPicked(int link, int request, String[] uris, String[] names, long[] sizes,
                                       String[] mimes, String[] authorities, String[] docIds, String primaryRoot);

    /** Runs a server inside the app (exported bundles); blocks until it stops. */
    static native int serverRun(String[] args);

    /** Starts a bundled program under proot. Returns the pid, or -1. */
    static native int bundleSpawn(String nativeLibDir, String bundleDir, String runtimeDir,
                                  String[] argv, String[] env, String logPath);
}
