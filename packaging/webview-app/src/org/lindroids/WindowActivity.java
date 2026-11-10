package org.lindroids;

import android.app.Activity;
import android.app.ActivityManager;
import android.content.ActivityNotFoundException;
import android.content.BroadcastReceiver;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.ContentResolver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.PackageManager;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Color;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.provider.DocumentsContract;
import android.provider.OpenableColumns;
import android.util.Base64;
import android.view.Display;
import android.view.InputDevice;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowManager;
import android.view.inputmethod.InputMethodManager;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.webkit.DownloadListener;
import android.webkit.JavascriptInterface;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.FrameLayout;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.Collections;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;

/**
 * A program's window, shown without a browser: Android's own WebView, a page from the Lindroids
 * server in Termux ({@code http://<label>.localhost:<port>/}).
 *
 * <p>The activity is one document task per run. It is immersive from the start, hands Back to the
 * page while the program runs, and finishes its task when the page says the program ended. It
 * exposes {@code window.lindroids} only to pages on {@code *.localhost}: Android's own file
 * pickers (open, save, folder) and downloads, the clipboard both ways, and sharing. Any other
 * address is handed to the user's browser, so the app can never be pointed at a website.</p>
 *
 * <p>A run's window and its run go together: the page tells the app which run it shows
 * ({@code attached}); the app tells the server when the window is gone for good, even with the page
 * gone ({@link CloseWatch}, {@link #onDestroy}); and the server has the app close the window when
 * the run ends ({@link End}). A card left in Recents by a run that is over goes when another run's
 * window opens.</p>
 */
public class WindowActivity extends Activity {
    /**
     * This code's number (lindroids_proto::APP_TEMPLATE): bumped with every change to the app,
     * whose code is in each APK built from it, so the page can tell an app that is out of date.
     */
    static final int TEMPLATE = 1;

    static final String EXTRA_URL = "org.lindroids.URL";
    static final String EXTRA_NAME = "org.lindroids.NAME";
    static final String EXTRA_ICON = "org.lindroids.ICON";
    static final String EXTRA_LAUNCH = "org.lindroids.LAUNCH";
    static final String EXTRA_TERMUX = "org.lindroids.TERMUX";
    static final String EXTRA_DEBUG = "org.lindroids.DEBUG";

    private static final int REQ_PICK = 4101;
    private static final int REQ_DOWNLOAD = 4103;
    /** Most entries a picked folder is listed with; more is an error, never a part of it. */
    private static final int MAX_ENTRIES = 20000;
    private static final int MAX_DEPTH = 32;
    private static final int CHUNK = 512 * 1024;

    private WebView webView;
    private String originalUrl;
    private String programName = "Lindroids";
    private Bitmap taskIcon;
    private String launchUrl = "";
    private String termuxPackage = "com.termux";
    private boolean offline = false;
    /** A program's own app: its start page (from the manifest), else null (the shared app). */
    private String startUrl;
    /** The program was started through Termux once already, when its page could not be reached. */
    private boolean autoStarted = false;
    private boolean resumed = false;
    private boolean pickerOpen = false;
    /** A program's own app: run windows handed over by broadcast (see {@link #takeWindows}). */
    private BroadcastReceiver windows;
    /** This task's key ({@link Runs#key}), and the run its page shows: {run, ticket, host:port}. */
    private String taskKey = "";
    private String[] attached;
    /** Runs whose window this process told the server is closed. */
    private static final Set<String> told = Collections.synchronizedSet(new HashSet<>());

    private final Bridge bridge = new Bridge();
    private String pickRequest;
    private String pickKind;
    // Used from the bridge's thread and the folder lister's as well as the main thread.
    private final Map<String, Uri> pickedUris = new ConcurrentHashMap<>();
    private final Map<String, OutputStream> pickedWrites = new ConcurrentHashMap<>();
    /** A picked document being read: its stream, open, and where in it the next chunk starts. */
    private final Map<String, Object[]> pickedReads = new ConcurrentHashMap<>();
    private final AtomicLong nextToken = new AtomicLong(1);
    private final Map<String, OutputStream> downloads = new ConcurrentHashMap<>();
    private final Map<String, String> downloadNames = new ConcurrentHashMap<>();
    private final Map<String, File> downloadFiles = new ConcurrentHashMap<>();
    private String downloadToken;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        Intent intent = getIntent();
        if (intent != null && intent.getBooleanExtra(EXTRA_DEBUG, false)) {
            WebView.setWebContentsDebuggingEnabled(true);
        }
        String start = programDefaults();
        startUrl = start;
        String url = intent == null ? null : urlOf(intent);
        if (url == null) url = start;
        if (url == null && this instanceof Launch) {
            // The windows app's own icon: it has no page; Termux is where programs start.
            startTermuxNow();
            finish();
            return;
        }
        if (url == null || !local(Uri.parse(url))) {
            if (url != null) openExternal(Uri.parse(url));
            finish();
            return;
        }
        originalUrl = url;
        taskKey = Runs.key(intent);
        readIntent(intent);
        setTaskFromIntent(intent);
        if (isRun(url)) {
            closeLauncherTasks();
            closeEndedTasks();
        }
        // A program's own app opened at its start page: the run it showed before is not this one's.
        if (this instanceof Launch && !isRun(url)) Runs.forget(this, taskKey);

        FrameLayout root = new FrameLayout(this);
        root.setBackgroundColor(Color.BLACK);
        webView = new WebView(this);
        webView.setBackgroundColor(Color.BLACK);
        webView.setLayoutParams(new FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
        root.addView(webView);
        setContentView(root);
        makeImmersive();
        keepClearOfKeyboard(root);
        // Taps and drags are the program's: no focus highlight (the blue flash), no text selection
        // or menu on a long press, no overscroll glow, and the renderer kept at full priority.
        webView.setLongClickable(false);
        webView.setOnLongClickListener(v -> true);
        webView.setHapticFeedbackEnabled(false);
        webView.setOverScrollMode(View.OVER_SCROLL_NEVER);
        webView.setVerticalScrollBarEnabled(false);
        webView.setHorizontalScrollBarEnabled(false);
        if (Build.VERSION.SDK_INT >= 26) {
            webView.setDefaultFocusHighlightEnabled(false);
            webView.setRendererPriorityPolicy(WebView.RENDERER_PRIORITY_IMPORTANT, false);
        }
        // Touches, a mouse and a pen reach the page as they come, not batched to the screen's next
        // frame: what a finger does gets to the program up to a frame sooner. Asked again at each
        // gesture's start, since the WebView taking focus drops the standing request.
        webView.setOnTouchListener((v, e) -> {
            if (e.getActionMasked() == android.view.MotionEvent.ACTION_DOWN) v.requestUnbufferedDispatch(e);
            return false;
        });
        unbuffered();
        fastestRefreshRate();

        WebSettings s = webView.getSettings();
        s.setJavaScriptEnabled(true);
        s.setDomStorageEnabled(true);
        s.setDatabaseEnabled(true);
        s.setAllowFileAccess(true);
        s.setAllowContentAccess(false);
        s.setAllowFileAccessFromFileURLs(false);
        s.setAllowUniversalAccessFromFileURLs(false);
        s.setMediaPlaybackRequiresUserGesture(false);
        s.setMixedContentMode(WebSettings.MIXED_CONTENT_NEVER_ALLOW);
        s.setSupportZoom(false);
        s.setBuiltInZoomControls(false);
        s.setDisplayZoomControls(false);
        s.setJavaScriptCanOpenWindowsAutomatically(false);

        webView.addJavascriptInterface(bridge, "lindroids");
        webView.setWebViewClient(new Client());
        webView.setWebChromeClient(new WebChromeClient());
        webView.setDownloadListener(new Downloads());
        load(url);
        if (startUrl != null && this instanceof Launch) takeWindows();
        webView.setFocusable(true);
        webView.setFocusableInTouchMode(true);
        takeKeys();

        if (Build.VERSION.SDK_INT >= 33) {
            getOnBackInvokedDispatcher().registerOnBackInvokedCallback(
                    android.window.OnBackInvokedDispatcher.PRIORITY_DEFAULT, this::handleBack);
        }
    }

    private static String urlOf(Intent i) {
        String u = i.getDataString();
        if (u == null) u = i.getStringExtra(EXTRA_URL);
        return u;
    }

    /**
     * A program's own app (built on the phone by {@code lindroids export}) carries its page, its
     * launch URL and its terminal app in the manifest's meta-data, and its name as the app's label,
     * so its icon opens it with no intent data. Returns the page; null for the shared app.
     */
    private String programDefaults() {
        try {
            Bundle meta = getPackageManager().getApplicationInfo(getPackageName(), PackageManager.GET_META_DATA).metaData;
            if (meta == null) return null;
            String start = meta.getString("org.lindroids.START");
            if (start == null || start.isEmpty()) return null;
            String l = meta.getString("org.lindroids.LAUNCH");
            if (l != null) launchUrl = l;
            String t = meta.getString("org.lindroids.TERMUX");
            if (t != null && !t.isEmpty()) termuxPackage = t;
            CharSequence label = getApplicationInfo().loadLabel(getPackageManager());
            if (label != null && label.length() > 0) programName = label.toString();
            return start;
        } catch (Throwable t) {
            return null;
        }
    }

    /**
     * A run's window opened: the app's own tasks started from its icon (no URL in their intent:
     * the offline page, or "Starting..." while Termux started the program) have done their part.
     */
    private void closeLauncherTasks() {
        try {
            ActivityManager am = (ActivityManager) getSystemService(Context.ACTIVITY_SERVICE);
            for (ActivityManager.AppTask t : am.getAppTasks()) {
                Intent base = t.getTaskInfo().baseIntent;
                // Not one that shows a run (a program's own app shows its runs in that task).
                if (base != null && base.getData() == null && base.getStringExtra(EXTRA_URL) == null && t.getTaskInfo().taskId != getTaskId()
                        && Runs.of(this, base) == null) {
                    t.finishAndRemoveTask();
                }
            }
        } catch (Throwable ignored) { }
    }

    /**
     * Cards in Recents whose run is over (it ended while their page was frozen or gone, or the
     * server stopped): tapping one would only show that it closed. They go when a new run's
     * window opens; the server says which runs still run, by their tickets.
     */
    private void closeEndedTasks() {
        final ActivityManager am = (ActivityManager) getSystemService(Context.ACTIVITY_SERVICE);
        final int me = getTaskId();
        new Thread(() -> {
            try {
                for (ActivityManager.AppTask t : am.getAppTasks()) {
                    if (t.getTaskInfo().taskId == me) continue;
                    Intent base = t.getTaskInfo().baseIntent;
                    String[] r = Runs.of(this, base);
                    if (r != null && Runs.tell(r, "alive") == 410) {
                        Runs.forget(this, Runs.key(base));
                        t.finishAndRemoveTask();
                    }
                }
            } catch (Throwable ignored) { }
        }).start();
    }

    /** The page says which run it shows (null: none, or it ended). */
    private void attach(String run, String ticket) {
        if (run == null || run.isEmpty() || ticket == null || ticket.isEmpty()) {
            attached = null;
            Runs.forget(this, taskKey);
            return;
        }
        Uri u = Uri.parse(originalUrl);
        attached = new String[]{run, ticket, u.getHost() + ":" + (u.getPort() > 0 ? u.getPort() : 80)};
        Runs.remember(this, taskKey, run, ticket, attached[2]);
    }

    /** The server is told that run {@code r}'s window is gone for good (once). Blocks. */
    static void tellClosed(String[] r) {
        if (told.add(r[0] + " " + r[1])) Runs.tell(r, "closed");
    }

    /** The watcher of removed tasks runs while a window is in front ({@link CloseWatch}). */
    private void watchCloses() {
        try {
            startService(new Intent(this, CloseWatch.class));
        } catch (Throwable ignored) {
            // In the background (not allowed to start a service): the next onResume does.
        }
    }

    private void readIntent(Intent intent) {
        String n = intent.getStringExtra(EXTRA_NAME);
        if (n != null && !n.isEmpty()) programName = n;
        String l = intent.getStringExtra(EXTRA_LAUNCH);
        if (l != null) launchUrl = l;
        String t = intent.getStringExtra(EXTRA_TERMUX);
        if (t != null && !t.isEmpty()) termuxPackage = t;
    }

    private void setTaskFromIntent(Intent intent) {
        byte[] bytes = intent == null ? null : intent.getByteArrayExtra(EXTRA_ICON);
        if (bytes != null && bytes.length > 0) {
            Bitmap decoded = BitmapFactory.decodeByteArray(bytes, 0, bytes.length);
            if (decoded != null) taskIcon = decoded;
        }
        if (taskIcon == null) {
            try { taskIcon = BitmapFactory.decodeResource(getResources(), R.mipmap.ic_launcher); } catch (Throwable ignored) { }
        }
        setTitle(programName);
        try {
            setTaskDescription(new ActivityManager.TaskDescription(programName, taskIcon, 0xFF0F1720));
        } catch (Throwable ignored) {
            // Very old releases: the task keeps the app's own name and icon.
        }
    }

    private void load(String url) {
        offline = false;
        if (webView != null) webView.loadUrl(url);
    }

    private void handleBack() {
        if (offline) {
            finishAndRemoveTask();
            return;
        }
        // Back belongs to the program's window: the page shows the same hint it does for
        // CloseWatcher in a browser. The window is closed by its ✕, Recents, or the program ending.
        if (webView != null) {
            webView.evaluateJavascript("window.dispatchEvent(new Event('lindroids:back'))", null);
        }
    }

    @Override
    @SuppressWarnings("deprecation")
    public void onBackPressed() {
        handleBack();
    }

    /**
     * The screen's fastest refresh rate at its resolution: tablets and phones with 90 or 120 Hz
     * screens often run an app at 60 unless it asks. Frames and input then come that much faster.
     */
    private void fastestRefreshRate() {
        try {
            Display d = getWindowManager().getDefaultDisplay();
            Display.Mode cur = d.getMode(), best = cur;
            for (Display.Mode m : d.getSupportedModes()) {
                if (m.getPhysicalWidth() == cur.getPhysicalWidth() && m.getPhysicalHeight() == cur.getPhysicalHeight()
                        && m.getRefreshRate() > best.getRefreshRate()) best = m;
            }
            WindowManager.LayoutParams lp = getWindow().getAttributes();
            lp.preferredDisplayModeId = best.getModeId();
            getWindow().setAttributes(lp);
        } catch (Throwable t) {
            // The default mode, then.
        }
    }

    private void makeImmersive() {
        if (Build.VERSION.SDK_INT >= 30) {
            getWindow().setDecorFitsSystemWindows(false);
            WindowInsetsController c = getWindow().getInsetsController();
            if (c != null) {
                c.hide(WindowInsets.Type.statusBars() | WindowInsets.Type.navigationBars());
                c.setSystemBarsBehavior(WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
            }
        } else {
            getWindow().getDecorView().setSystemUiVisibility(
                    View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                            | View.SYSTEM_UI_FLAG_FULLSCREEN
                            | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                            | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                            | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                            | View.SYSTEM_UI_FLAG_LAYOUT_STABLE);
        }
    }

    /**
     * The soft keyboard takes the bottom of the window, and the program's window shrinks above it.
     * The window is drawn under the system bars (immersive), where Android does not resize it for
     * the keyboard (adjustResize: nothing on Android 15+ edge to edge, nothing with the decor not
     * fitting system windows): the keyboard's height comes off the bottom here instead.
     */
    private void keepClearOfKeyboard(View root) {
        if (Build.VERSION.SDK_INT < 30) return;
        getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_NOTHING);
        root.setOnApplyWindowInsetsListener((v, insets) -> {
            int bottom = insets.getInsets(WindowInsets.Type.ime()).bottom;
            if (v.getPaddingBottom() != bottom) v.setPadding(0, 0, 0, bottom);
            return insets;
        });
    }

    private void softKeyboard(boolean show) {
        if (webView == null) return;
        InputMethodManager imm = (InputMethodManager) getSystemService(Context.INPUT_METHOD_SERVICE);
        if (imm == null) return;
        if (show) {
            webView.requestFocus();
            imm.showSoftInput(webView, 0);
        } else {
            imm.hideSoftInputFromWindow(webView.getWindowToken(), 0);
        }
    }

    @Override
    public void onWindowFocusChanged(boolean focused) {
        super.onWindowFocusChanged(focused);
        if (focused) {
            makeImmersive();
            takeKeys();
        }
    }

    /**
     * Keys go to the page from the start: a WebView without Android's focus takes the first D-pad
     * or arrow key to gain it, and moves the page's focus with it (to a button or the hidden text
     * area) instead of passing the key on to the program.
     */
    private void takeKeys() {
        if (webView != null && !webView.hasFocus()) webView.requestFocus(View.FOCUS_DOWN);
        unbuffered();
    }

    /** Every pointer event unbuffered (Android 11+), mouse and pen moves without a button too. */
    private void unbuffered() {
        if (webView != null && Build.VERSION.SDK_INT >= 30) webView.requestUnbufferedDispatch(InputDevice.SOURCE_CLASS_POINTER);
    }

    @Override
    protected void onResume() {
        super.onResume();
        resumed = true;
        if (webView != null) webView.onResume();
        makeImmersive();
        watchCloses();
    }

    @Override
    protected void onPause() {
        resumed = false;
        if (webView != null) webView.onPause();
        super.onPause();
    }

    /**
     * A program's own app whose page had not paired asks the server for the program; the run's
     * window (its URL carries the ticket that pairs the page) comes back as a broadcast to this
     * package, since Termux, in the background then, may not open the app. Only a page of this
     * program on this app's own server is loaded.
     */
    private void takeWindows() {
        windows = new BroadcastReceiver() {
            @Override
            public void onReceive(Context c, Intent i) {
                String url = i.getStringExtra(EXTRA_URL);
                if (url == null || !ownPage(url) || webView == null) return;
                if (url.equals(originalUrl)) return;
                if (elsewhere(url, i)) return;
                readIntent(i);
                setTaskFromIntent(i);
                originalUrl = url;
                load(url);
            }
        };
        IntentFilter f = new IntentFilter("org.lindroids.WINDOW");
        try {
            if (Build.VERSION.SDK_INT >= 33) {
                registerReceiver(windows, f, Context.RECEIVER_EXPORTED);
            } else {
                registerReceiver(windows, f);
            }
        } catch (Throwable t) {
            windows = null;
        }
    }

    /** A page under this program's start page, on the same server. */
    private boolean ownPage(String url) {
        Uri u = Uri.parse(url), s = Uri.parse(startUrl);
        return local(u) && u.getHost().equalsIgnoreCase(s.getHost()) && u.getPort() == s.getPort()
                && u.getPath() != null && s.getPath() != null && u.getPath().startsWith(s.getPath());
    }

    /**
     * A program's own app shows its runs in one task: a window of another run while the run it
     * shows still runs (another instance: "New window") gets a task of its own, since loading it
     * in place would close the running one's window, and with it that run. True if so.
     */
    private boolean elsewhere(String url, Intent from) {
        if (!(this instanceof Launch) || attached == null || !isRun(url)) return false;
        String[] r = Runs.ofUrl(url);
        if (r == null || r[0].equals(attached[0])) return false;
        Intent i = new Intent(this, WindowActivity.class);
        if (from != null && from.getExtras() != null) i.putExtras(from.getExtras());
        i.setData(Uri.parse(url));
        i.removeExtra(EXTRA_URL);
        i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_NEW_DOCUMENT | Intent.FLAG_ACTIVITY_MULTIPLE_TASK);
        try {
            startActivity(i);
            return true;
        } catch (Throwable t) {
            return false;
        }
    }

    @Override
    protected void onDestroy() {
        // Closed for good, not rebuilt (a rotation is not handled by a rebuild here anyway): the
        // run it showed ends after the server's grace, unless a window links for it again. Also
        // said by CloseWatch for a task swiped away; whichever comes first.
        if (isFinishing() && !isChangingConfigurations() && attached != null) {
            final String[] r = attached;
            new Thread(() -> tellClosed(r)).start();
        }
        for (Object[] read : pickedReads.values()) {
            try { ((InputStream) read[0]).close(); } catch (Throwable ignored) { }
        }
        pickedReads.clear();
        if (windows != null) {
            try { unregisterReceiver(windows); } catch (Throwable ignored) { }
            windows = null;
        }
        if (webView != null) {
            webView.removeJavascriptInterface("lindroids");
            webView.stopLoading();
            webView.destroy();
            webView = null;
        }
        super.onDestroy();
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        if (intent == null) return;
        setIntent(intent);
        readIntent(intent);
        setTaskFromIntent(intent);
        String url = urlOf(intent);
        if (url != null && local(Uri.parse(url)) && !url.equals(originalUrl) && !elsewhere(url, intent)) {
            originalUrl = url;
            load(url);
        }
    }

    /** Whether a URL is the Lindroids server: {@code http://<label>.localhost[:port]/...}. */
    static boolean local(Uri u) {
        if (u == null || !"http".equals(u.getScheme())) return false;
        String h = u.getHost();
        return h != null && (h.equals("localhost") || h.endsWith(".localhost"));
    }

    private void openExternal(Uri uri) {
        try {
            Intent i = new Intent(Intent.ACTION_VIEW, uri);
            i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            startActivity(i);
        } catch (ActivityNotFoundException ignored) { }
    }

    /** A run's window (`?window&run=<pid>`), not a program's start page. */
    private static boolean isRun(String url) {
        return url != null && url.contains("run=");
    }

    private void loadOffline(WebResourceError error) {
        if (offline) return;
        // A run's window in the shared app is a temporary window: the run is gone with the
        // server (Termux closed), so is its window.
        if (startUrl == null && isRun(originalUrl)) {
            finishAndRemoveTask();
            return;
        }
        offline = true;
        // Opened from the program's icon while Termux is not running: start it through Termux at
        // once (this app is in front, so Android lets it), as the Start button would.
        boolean auto = startUrl != null && !isRun(originalUrl) && !autoStarted;
        if (auto) {
            autoStarted = true;
            startTermuxNow();
        }
        String q = "?name=" + Uri.encode(programName)
                + (auto ? "&auto=1" : "")
                + "&launch=" + Uri.encode(launchUrl)
                + "&termux=" + Uri.encode(termuxPackage)
                + "&url=" + Uri.encode(originalUrl);
        if (webView != null) webView.loadUrl("file:///android_asset/offline.html" + q);
    }

    /**
     * Hands the program's launch URL to Termux (its share receiver runs `lindroids launch <id>`,
     * as a Termux home screen icon does); without one, opens Termux.
     */
    void startTermuxNow() {
        if (launchUrl == null || launchUrl.isEmpty()) {
            Intent launch = getPackageManager().getLaunchIntentForPackage(termuxPackage);
            if (launch != null) {
                launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                try { startActivity(launch); } catch (Throwable ignored) { }
            }
            return;
        }
        Intent send = new Intent(Intent.ACTION_SEND);
        send.setType("text/plain");
        send.putExtra(Intent.EXTRA_TEXT, launchUrl);
        send.setPackage(termuxPackage);
        send.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        try {
            startActivity(send);
        } catch (Throwable t) {
            Intent launch = getPackageManager().getLaunchIntentForPackage(termuxPackage);
            if (launch != null) {
                launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                try { startActivity(launch); } catch (Throwable ignored) { }
            }
        }
    }

    private final class Client extends WebViewClient {
        @Override
        public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
            Uri u = request.getUrl();
            if (u == null) return true;
            String scheme = u.getScheme() == null ? "" : u.getScheme();
            if (local(u)) return false;
            if (scheme.equals("about") || scheme.equals("data") || scheme.equals("blob")) return false;
            if (scheme.equals("file")) return !u.toString().startsWith("file:///android_asset/");
            if (scheme.equals("intent")) {
                try {
                    Intent i = Intent.parseUri(u.toString(), Intent.URI_INTENT_SCHEME);
                    i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                    startActivity(i);
                } catch (Exception ignored) { }
                return true;
            }
            openExternal(u);
            return true;
        }

        @Override
        public void onPageFinished(WebView view, String url) {
            super.onPageFinished(view, url);
            if (!url.startsWith("file:///android_asset/")) offline = false;
        }

        @Override
        public void onReceivedError(WebView view, WebResourceRequest request, WebResourceError error) {
            super.onReceivedError(view, request, error);
            if (!request.isForMainFrame()) return;
            int c = error == null ? -1 : error.getErrorCode();
            if (c == WebViewClient.ERROR_HOST_LOOKUP || c == WebViewClient.ERROR_CONNECT
                    || c == WebViewClient.ERROR_TIMEOUT || c == WebViewClient.ERROR_IO
                    || c == WebViewClient.ERROR_UNKNOWN) {
                loadOffline(error);
            }
        }
    }

    private final class Downloads implements DownloadListener {
        @Override
        public void onDownloadStart(String url, String userAgent, String contentDisposition, String mimeType, long contentLength) {
            // Downloads the page could not hand over as a blob go through the bridge; an ordinary
            // link here is a file the user asked for.
            openExternal(Uri.parse(url));
        }
    }

    private static int grantFlags() {
        return Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION
                | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION;
    }

    private void deliver(String request, String json) {
        final String js = "window.__lindroidsPicked && window.__lindroidsPicked(" + JSONObject.quote(request) + "," + json + ")";
        runOnUiThread(() -> {
            if (webView != null) webView.evaluateJavascript(js, null);
        });
    }

    private void deliverSaved(String token, String json) {
        final String js = "window.__lindroidsSaved && window.__lindroidsSaved(" + JSONObject.quote(token) + "," + json + ")";
        runOnUiThread(() -> {
            if (webView != null) webView.evaluateJavascript(js, null);
        });
    }

    private String newToken() {
        return "n" + nextToken.getAndIncrement();
    }

    private void startPicker(String request, String kind, boolean multiple, JSONArray mimes, String suggested) {
        pickRequest = request;
        pickKind = kind;
        pickerOpen = true;
        String[] types = new String[mimes == null ? 0 : mimes.length()];
        for (int i = 0; i < types.length; i++) types[i] = mimes.optString(i, "*/*");
        runOnUiThread(() -> {
            Intent i;
            if ("dir".equals(kind)) {
                i = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
            } else if ("save".equals(kind)) {
                i = new Intent(Intent.ACTION_CREATE_DOCUMENT);
                i.addCategory(Intent.CATEGORY_OPENABLE);
                i.setType(types.length > 0 ? types[0] : "*/*");
                if (suggested != null && !suggested.isEmpty()) i.putExtra(Intent.EXTRA_TITLE, suggested);
            } else {
                i = new Intent(Intent.ACTION_OPEN_DOCUMENT);
                i.addCategory(Intent.CATEGORY_OPENABLE);
                i.setType(types.length == 1 ? types[0] : "*/*");
                if (types.length > 1) i.putExtra(Intent.EXTRA_MIME_TYPES, types);
                if (multiple) i.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true);
            }
            i.addFlags(grantFlags());
            try {
                startActivityForResult(i, REQ_PICK);
            } catch (ActivityNotFoundException e) {
                pickerOpen = false;
                pickRequest = null;
                deliver(request, "{\"error\":\"Android has no file picker\"}");
            }
        });
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode == REQ_DOWNLOAD) {
            finishDownload(resultCode == RESULT_OK ? data : null);
            return;
        }
        if (requestCode != REQ_PICK) return;
        pickerOpen = false;
        String request = pickRequest;
        pickRequest = null;
        if (request == null) return;
        if (resultCode != RESULT_OK || data == null) {
            deliver(request, "{\"cancel\":true}");
            return;
        }
        final String kind = pickKind;
        // Off the main thread: listing a folder through its provider takes a query per folder in
        // it, which on a big one is seconds (Android would say the app is not responding).
        new Thread(() -> {
            try {
                JSONObject out = new JSONObject();
                JSONArray entries = new JSONArray();
                if ("dir".equals(kind)) {
                    Uri tree = data.getData();
                    if (tree == null) throw new IllegalArgumentException("no folder");
                    take(tree, data.getFlags());
                    String root = DocumentsContract.getTreeDocumentId(tree);
                    Uri rootDoc = DocumentsContract.buildDocumentUriUsingTree(tree, root);
                    String path = sharedPath(tree.getAuthority(), root);
                    JSONObject top = entry(rootDoc, "", true);
                    if (path != null) top.put("path", path);
                    entries.put(top);
                    if (!listTree(tree, root, "", entries, 0)) {
                        // Too big to list: the folder itself, which the Linux side reads where it
                        // is when it can (Termux with storage access), and nothing else.
                        if (path == null) {
                            throw new IllegalStateException("The folder holds more than " + MAX_ENTRIES + " files and folders (or is nested deeper than "
                                    + MAX_DEPTH + "): pick a smaller one");
                        }
                        JSONArray only = new JSONArray();
                        only.put(top);
                        entries = only;
                    }
                } else if ("save".equals(kind)) {
                    Uri uri = data.getData();
                    if (uri == null) throw new IllegalArgumentException("no document");
                    take(uri, data.getFlags());
                    entries.put(entry(uri, "this", false));
                } else {
                    for (Uri uri : urisOf(data)) {
                        take(uri, data.getFlags());
                        entries.put(entry(uri, "", false));
                    }
                }
                out.put("entries", entries);
                deliver(request, out.toString());
            } catch (Throwable t) {
                deliver(request, "{\"error\":" + JSONObject.quote(String.valueOf(t.getMessage())) + "}");
            }
        }).start();
    }

    /**
     * Where a document of Android's shared storage is as a path, from its provider's id
     * ("primary:Download/a.pdf" is /storage/emulated/0/Download/a.pdf; the Downloads provider's
     * "raw:/storage/..."): the Linux side then uses the document itself when it can read it,
     * rather than a copy. Null for any other provider.
     */
    static String sharedPath(String authority, String docId) {
        if (docId == null) return null;
        if ("com.android.externalstorage.documents".equals(authority) && docId.startsWith("primary:")) {
            String rel = docId.substring("primary:".length());
            if (rel.contains("..")) return null;
            return rel.isEmpty() ? "/storage/emulated/0" : "/storage/emulated/0/" + rel;
        }
        if ("com.android.providers.downloads.documents".equals(authority) && docId.startsWith("raw:/storage/")) {
            String p = docId.substring("raw:".length());
            return p.contains("..") ? null : p;
        }
        return null;
    }

    private Uri[] urisOf(Intent data) {
        if (data == null) return new Uri[0];
        ClipData clip = data.getClipData();
        if (clip != null) {
            Uri[] all = new Uri[clip.getItemCount()];
            for (int i = 0; i < all.length; i++) all[i] = clip.getItemAt(i).getUri();
            return all;
        }
        Uri one = data.getData();
        return one == null ? new Uri[0] : new Uri[]{one};
    }

    private void take(Uri uri, int flags) {
        if (uri == null) return;
        try {
            int mode = 0;
            if ((flags & Intent.FLAG_GRANT_READ_URI_PERMISSION) != 0) mode |= Intent.FLAG_GRANT_READ_URI_PERMISSION;
            if ((flags & Intent.FLAG_GRANT_WRITE_URI_PERMISSION) != 0) mode |= Intent.FLAG_GRANT_WRITE_URI_PERMISSION;
            if (mode != 0) getContentResolver().takePersistableUriPermission(uri, mode);
        } catch (Throwable ignored) {
            // Some providers do not persist; the grant still lasts for this window.
        }
    }

    private JSONObject entry(Uri uri, String rel, boolean directory) throws Exception {
        String token = newToken();
        pickedUris.put(token, uri);
        String[] nameSize = nameSize(uri);
        JSONObject o = new JSONObject();
        o.put("uri", "native:" + token);
        o.put("key", token);
        o.put("name", nameSize[0]);
        o.put("size", Long.parseLong(nameSize[1]));
        o.put("mime", directory ? "inode/directory" : nameSize[2]);
        o.put("writable", true);
        if (!rel.isEmpty() || directory) o.put("rel", rel);
        if (!directory) {
            try {
                String path = sharedPath(uri.getAuthority(), DocumentsContract.getDocumentId(uri));
                if (path != null) o.put("path", path);
            } catch (Throwable ignored) { }
        }
        return o;
    }

    private String[] nameSize(Uri uri) {
        String name = "file";
        long size = 0;
        String mime = "*/*";
        try (android.database.Cursor c = getContentResolver().query(uri,
                new String[]{OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE}, null, null, null)) {
            if (c != null && c.moveToFirst()) {
                int ni = c.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                int si = c.getColumnIndex(OpenableColumns.SIZE);
                if (ni >= 0 && !c.isNull(ni)) name = c.getString(ni);
                if (si >= 0 && !c.isNull(si)) size = c.getLong(si);
            }
        } catch (Throwable ignored) { }
        try {
            String t = getContentResolver().getType(uri);
            if (t != null) mime = t;
        } catch (Throwable ignored) { }
        return new String[]{name, Long.toString(size), mime};
    }

    /** Everything in a picked folder, into {@code out}; false when it is too big to list. */
    private boolean listTree(Uri tree, String docId, String prefix, JSONArray out, int depth) throws Exception {
        if (depth > MAX_DEPTH) return false;
        Uri children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, docId);
        try (android.database.Cursor c = getContentResolver().query(children, new String[]{
                DocumentsContract.Document.COLUMN_DOCUMENT_ID,
                DocumentsContract.Document.COLUMN_DISPLAY_NAME,
                DocumentsContract.Document.COLUMN_MIME_TYPE,
                DocumentsContract.Document.COLUMN_SIZE}, null, null, null)) {
            if (c == null) return true;
            while (c.moveToNext()) {
                if (out.length() > MAX_ENTRIES) return false;
                String id = c.getString(0);
                String name = c.getString(1);
                String mime = c.getString(2);
                long size = c.isNull(3) ? 0 : c.getLong(3);
                Uri uri = DocumentsContract.buildDocumentUriUsingTree(tree, id);
                String rel = prefix.isEmpty() ? name : prefix + "/" + name;
                if (DocumentsContract.Document.MIME_TYPE_DIR.equals(mime)) {
                    String token = newToken();
                    pickedUris.put(token, uri);
                    JSONObject d = new JSONObject();
                    d.put("uri", "native:" + token);
                    d.put("key", token);
                    d.put("name", name);
                    d.put("size", 0);
                    d.put("mime", "inode/directory");
                    d.put("rel", rel);
                    d.put("writable", true);
                    out.put(d);
                    if (!listTree(tree, id, rel, out, depth + 1)) return false;
                } else {
                    String token = newToken();
                    pickedUris.put(token, uri);
                    JSONObject f = new JSONObject();
                    f.put("uri", "native:" + token);
                    f.put("key", token);
                    f.put("name", name);
                    f.put("size", size);
                    f.put("mime", mime == null ? "application/octet-stream" : mime);
                    f.put("rel", rel);
                    f.put("writable", true);
                    out.put(f);
                }
            }
        }
        return true;
    }

    private void finishDownload(Intent data) {
        String token = downloadToken;
        downloadToken = null;
        File tmp = token == null ? null : downloadFiles.remove(token);
        String name = token == null ? null : downloadNames.remove(token);
        if (tmp == null) {
            deliverSaved(token == null ? "" : token, "{\"ok\":false}");
            return;
        }
        Uri dest = data == null ? null : data.getData();
        if (dest == null) {
            tmp.delete();
            deliverSaved(token, "{\"ok\":false}");
            return;
        }
        try (InputStream in = new FileInputStream(tmp); OutputStream out = getContentResolver().openOutputStream(dest, "wt")) {
            if (out == null) throw new IllegalStateException("no " + dest);
            byte[] buf = new byte[64 * 1024];
            int n;
            while ((n = in.read(buf)) > 0) out.write(buf, 0, n);
            deliverSaved(token, "{\"ok\":true,\"name\":" + JSONObject.quote(name == null ? "" : name) + "}");
        } catch (Throwable t) {
            deliverSaved(token, "{\"ok\":false,\"error\":" + JSONObject.quote(String.valueOf(t.getMessage())) + "}");
        } finally {
            tmp.delete();
        }
    }

    /** The page's bridge. Only {@code *.localhost} is ever loaded in this WebView. */
    public final class Bridge {
        @JavascriptInterface
        public void close() {
            runOnUiThread(WindowActivity.this::finishAndRemoveTask);
        }

        /** This app's code ({@link #TEMPLATE}): the page says when it is older than its server's. */
        @JavascriptInterface
        public int template() {
            return TEMPLATE;
        }

        /** The page linked to its run ({@code run}, by its ticket). */
        @JavascriptInterface
        public void attached(String run, String ticket) {
            runOnUiThread(() -> attach(run, ticket));
        }

        /** The run the page showed is over. */
        @JavascriptInterface
        public void detached() {
            runOnUiThread(() -> attach(null, null));
        }

        @JavascriptInterface
        public void share(String text, String title) {
            runOnUiThread(() -> {
                Intent i = new Intent(Intent.ACTION_SEND);
                i.setType("text/plain");
                i.putExtra(Intent.EXTRA_TEXT, text);
                if (title != null && !title.isEmpty()) i.putExtra(Intent.EXTRA_SUBJECT, title);
                try {
                    startActivity(Intent.createChooser(i, title == null || title.isEmpty() ? "Share" : title));
                } catch (ActivityNotFoundException ignored) { }
            });
        }

        @JavascriptInterface
        public String readClipboard() {
            try {
                ClipboardManager cm = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
                if (cm == null || !cm.hasPrimaryClip() || cm.getPrimaryClip() == null) return "";
                ClipData.Item item = cm.getPrimaryClip().getItemAt(0);
                CharSequence text = item == null ? null : item.getText();
                return text == null ? "" : text.toString();
            } catch (Throwable t) {
                return "";
            }
        }

        @JavascriptInterface
        public void writeClipboard(String text) {
            try {
                ClipboardManager cm = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
                if (cm != null) cm.setPrimaryClip(ClipData.newPlainText("lindroids", text == null ? "" : text));
            } catch (Throwable ignored) { }
        }

        @JavascriptInterface
        public void pickFiles(String request, String kind, boolean multiple, String mimeJson, String suggested) {
            if (request == null) return;
            if (pickerOpen) {
                // Android shows one picker at a time; the program hears at once, not after an hour.
                deliver(request, "{\"error\":\"Android's file picker is open already: finish with it first\"}");
                return;
            }
            JSONArray mimes = new JSONArray();
            try { if (mimeJson != null && !mimeJson.isEmpty()) mimes = new JSONArray(mimeJson); } catch (Throwable ignored) { }
            startPicker(request, kind == null ? "open" : kind, multiple, mimes, suggested);
        }

        @JavascriptInterface
        public String readPicked(String key, int offset) {
            Uri uri = key == null ? null : pickedUris.get(key);
            if (uri == null) return null;
            // One stream per document, read on from where the last chunk ended: opening it again
            // and skipping to each chunk made a big file's reading quadratic.
            Object[] read = pickedReads.remove(key);
            try {
                if (read != null && (Long) read[1] != offset) {
                    try { ((InputStream) read[0]).close(); } catch (Throwable ignored) { }
                    read = null;
                }
                if (read == null) {
                    InputStream in = getContentResolver().openInputStream(uri);
                    if (in == null) return null;
                    skip(in, offset);
                    read = new Object[]{in, (long) offset};
                }
                InputStream in = (InputStream) read[0];
                // A whole chunk unless at the end: a shorter one is the last (the page stops there).
                byte[] buf = new byte[CHUNK];
                int n = 0;
                while (n < CHUNK) {
                    int k = in.read(buf, n, CHUNK - n);
                    if (k < 0) break;
                    n += k;
                }
                if (n < CHUNK) {
                    in.close();
                } else {
                    read[1] = (long) offset + n;
                    pickedReads.put(key, read);
                }
                return n == 0 ? "" : Base64.encodeToString(buf, 0, n, Base64.NO_WRAP);
            } catch (Throwable t) {
                if (read != null) try { ((InputStream) read[0]).close(); } catch (Throwable ignored) { }
                return null;
            }
        }

        @JavascriptInterface
        public String writePicked(String key, String b64, int offset, boolean last) {
            Uri uri = key == null ? null : pickedUris.get(key);
            if (uri == null) return "error: not a picked file";
            try {
                byte[] data = Base64.decode(b64 == null ? "" : b64, Base64.NO_WRAP);
                OutputStream out = pickedWrites.get(key);
                if (out == null || offset == 0) {
                    if (out != null) try { out.close(); } catch (Throwable ignored) { }
                    out = getContentResolver().openOutputStream(uri, "wt");
                    if (out == null) return "error: the document cannot be written";
                    pickedWrites.put(key, out);
                }
                out.write(data);
                if (last) {
                    out.close();
                    pickedWrites.remove(key);
                }
                return "ok";
            } catch (Throwable t) {
                OutputStream out = pickedWrites.remove(key);
                if (out != null) try { out.close(); } catch (Throwable ignored) { }
                return "error: " + t.getMessage();
            }
        }

        @JavascriptInterface
        public String downloadStart(String name) {
            try {
                String token = newToken();
                File f = File.createTempFile("lindroids-download-", ".tmp", getCacheDir());
                FileOutputStream out = new FileOutputStream(f);
                downloads.put(token, out);
                downloadFiles.put(token, f);
                downloadNames.put(token, name == null ? "file" : name);
                return token;
            } catch (Throwable t) {
                return null;
            }
        }

        @JavascriptInterface
        public String downloadChunk(String token, String b64, boolean last) {
            try {
                OutputStream out = downloads.get(token);
                if (out == null) return "error: no download";
                out.write(Base64.decode(b64 == null ? "" : b64, Base64.NO_WRAP));
                if (last) {
                    out.close();
                    downloads.remove(token);
                    String name = downloadNames.get(token);
                    runOnUiThread(() -> {
                        downloadToken = token;
                        Intent i = new Intent(Intent.ACTION_CREATE_DOCUMENT);
                        i.addCategory(Intent.CATEGORY_OPENABLE);
                        i.setType("*/*");
                        i.putExtra(Intent.EXTRA_TITLE, name == null ? "file" : name);
                        i.addFlags(grantFlags());
                        try {
                            startActivityForResult(i, REQ_DOWNLOAD);
                        } catch (ActivityNotFoundException e) {
                            finishDownload(null);
                        }
                    });
                }
                return "ok";
            } catch (Throwable t) {
                return "error: " + t.getMessage();
            }
        }

        /**
         * The page focused (or left) its text field for the program's text field: Android shows
         * the soft keyboard only for a tap, and this came from the program.
         */
        @JavascriptInterface
        public void keyboard(boolean show) {
            runOnUiThread(() -> softKeyboard(show));
        }

        @JavascriptInterface
        public void startTermux() {
            runOnUiThread(WindowActivity.this::startTermuxNow);
        }

        @JavascriptInterface
        public void retry() {
            // Reload only once the server answers: reloading while it is down shows this page
            // again, and a tap on Start during that reload is lost.
            // A program's own app goes back to its start page, which finds the program running or
            // starts it: the run this window showed may be gone with the server. Only while the
            // app is in front, so a server started later for something else does not start it.
            if (!resumed) return;
            final String url = startUrl != null ? startUrl : originalUrl;
            if (startUrl != null) originalUrl = startUrl;
            new Thread(() -> {
                Uri u = Uri.parse(url);
                int port = u.getPort() > 0 ? u.getPort() : 80;
                try (java.net.Socket sock = new java.net.Socket()) {
                    sock.connect(new java.net.InetSocketAddress("127.0.0.1", port), 800);
                } catch (Throwable t) {
                    return;
                }
                runOnUiThread(() -> load(url));
            }).start();
        }

        @JavascriptInterface
        public String packageName() {
            return getPackageName();
        }
    }

    private static void skip(InputStream in, long n) throws Exception {
        long left = n;
        while (left > 0) {
            long s = in.skip(left);
            if (s <= 0) {
                if (in.read() < 0) return;
                s = 1;
            }
            left -= s;
        }
    }
}
