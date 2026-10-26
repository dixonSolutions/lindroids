package io.github.dixonsolutions.lindroids;

import android.view.Surface;

/** Entry points into liblindroids.so (Rust). Everything that is not Android plumbing lives there. */
final class Native {
    private Native() {}

    /** Called once at app start: where bundles live, and the screen density for the server. */
    static native void appInit(String filesDir, float density);

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

    /** Runs a server inside the app (exported bundles); blocks until it stops. */
    static native int serverRun(String[] args);

    /** Starts a bundled program under proot. Returns the pid, or -1. */
    static native int bundleSpawn(String nativeLibDir, String bundleDir, String runtimeDir,
                                  String[] argv, String[] env, String logPath);
}
