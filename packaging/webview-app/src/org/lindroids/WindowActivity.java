package org.lindroids;

import android.app.Activity;
import android.app.ActivityManager;
import android.content.ActivityNotFoundException;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.ContentResolver;
import android.content.Context;
import android.content.Intent;
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
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.webkit.DownloadListener;
import android.webkit.JavascriptInterface;
import android.webkit.ValueCallback;
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
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * A program's window, shown without a browser: Android's own WebView, a page from the Lindroids
 * server in Termux ({@code http://<label>.localhost:<port>/}).
 *
 * <p>The activity is one document task per run. It is immersive from the start, hands Back to the
 * page while the program runs, and finishes its task when the page says the program ended. It
 * exposes {@code window.lindroids} only to pages on {@code *.localhost}: Android's own file
 * pickers (open, save, folder) and downloads, the clipboard both ways, and sharing. Any other
 * address is handed to the user's browser, so the app can never be pointed at a website.</p>
 */
public final class WindowActivity extends Activity {
    static final String EXTRA_URL = "org.lindroids.URL";
    static final String EXTRA_NAME = "org.lindroids.NAME";
    static final String EXTRA_ICON = "org.lindroids.ICON";
    static final String EXTRA_LAUNCH = "org.lindroids.LAUNCH";
    static final String EXTRA_TERMUX = "org.lindroids.TERMUX";
    static final String EXTRA_DEBUG = "org.lindroids.DEBUG";

    private static final int REQ_PICK = 4101;
    private static final int REQ_FILE_CHOOSER = 4102;
    private static final int REQ_DOWNLOAD = 4103;

    private WebView webView;
    private String originalUrl;
    private String programName = "Lindroids";
    private Bitmap taskIcon;
    private String launchUrl = "";
    private String termuxPackage = "com.termux";
    private boolean offline = false;
    private boolean pickerOpen = false;

    private final Bridge bridge = new Bridge();
    private String pickRequest;
    private String pickKind;
    private ValueCallback<Uri[]> fileChooserCallback;
    private final Map<String, Uri> pickedUris = new HashMap<>();
    private final Map<String, OutputStream> pickedWrites = new HashMap<>();
    private long nextToken = 1;
    private final Map<String, OutputStream> downloads = new HashMap<>();
    private final Map<String, String> downloadNames = new HashMap<>();
    private final Map<String, File> downloadFiles = new HashMap<>();
    private String downloadToken;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        Intent intent = getIntent();
        if (intent != null && intent.getBooleanExtra(EXTRA_DEBUG, false)) {
            WebView.setWebContentsDebuggingEnabled(true);
        }
        String url = intent == null ? null : urlOf(intent);
        if (url == null || !local(Uri.parse(url))) {
            if (url != null) openExternal(Uri.parse(url));
            finish();
            return;
        }
        originalUrl = url;
        readIntent(intent);
        setTaskFromIntent(intent);

        FrameLayout root = new FrameLayout(this);
        root.setBackgroundColor(Color.BLACK);
        webView = new WebView(this);
        webView.setBackgroundColor(Color.BLACK);
        webView.setLayoutParams(new FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
        root.addView(webView);
        setContentView(root);
        makeImmersive();

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
        webView.setWebChromeClient(new Chrome());
        webView.setDownloadListener(new Downloads());
        load(url);

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

    @Override
    public void onWindowFocusChanged(boolean focused) {
        super.onWindowFocusChanged(focused);
        if (focused) makeImmersive();
    }

    @Override
    protected void onResume() {
        super.onResume();
        if (webView != null) webView.onResume();
        makeImmersive();
    }

    @Override
    protected void onPause() {
        if (webView != null) webView.onPause();
        super.onPause();
    }

    @Override
    protected void onDestroy() {
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
        if (url != null && local(Uri.parse(url)) && !url.equals(originalUrl)) {
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

    private void loadOffline(WebResourceError error) {
        if (offline) return;
        offline = true;
        String q = "?name=" + Uri.encode(programName)
                + "&launch=" + Uri.encode(launchUrl)
                + "&termux=" + Uri.encode(termuxPackage)
                + "&url=" + Uri.encode(originalUrl);
        if (webView != null) webView.loadUrl("file:///android_asset/offline.html" + q);
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

    private final class Chrome extends WebChromeClient {
        @Override
        public boolean onShowFileChooser(WebView view, ValueCallback<Uri[]> callback, FileChooserParams params) {
            if (fileChooserCallback != null) fileChooserCallback.onReceiveValue(null);
            fileChooserCallback = callback;
            Intent i = new Intent(Intent.ACTION_OPEN_DOCUMENT);
            i.addCategory(Intent.CATEGORY_OPENABLE);
            i.setType("*/*");
            i.addFlags(grantFlags());
            if (params != null && params.getMode() == FileChooserParams.MODE_OPEN_MULTIPLE) {
                i.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true);
            }
            try {
                startActivityForResult(i, REQ_FILE_CHOOSER);
                return true;
            } catch (ActivityNotFoundException e) {
                fileChooserCallback = null;
                return false;
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
        return "n" + (nextToken++);
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
        if (requestCode == REQ_FILE_CHOOSER) {
            ValueCallback<Uri[]> cb = fileChooserCallback;
            fileChooserCallback = null;
            if (cb == null) return;
            cb.onReceiveValue(resultCode == RESULT_OK ? urisOf(data) : null);
            return;
        }
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
        try {
            JSONObject out = new JSONObject();
            JSONArray entries = new JSONArray();
            if ("dir".equals(pickKind)) {
                Uri tree = data.getData();
                if (tree == null) throw new IllegalArgumentException("no folder");
                take(tree, data.getFlags());
                String root = DocumentsContract.getTreeDocumentId(tree);
                listTree(tree, root, "", entries, 0);
            } else if ("save".equals(pickKind)) {
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

    private void listTree(Uri tree, String docId, String prefix, JSONArray out, int depth) throws Exception {
        if (depth > 32 || out.length() > 20000) return;
        Uri children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, docId);
        try (android.database.Cursor c = getContentResolver().query(children, new String[]{
                DocumentsContract.Document.COLUMN_DOCUMENT_ID,
                DocumentsContract.Document.COLUMN_DISPLAY_NAME,
                DocumentsContract.Document.COLUMN_MIME_TYPE,
                DocumentsContract.Document.COLUMN_SIZE}, null, null, null)) {
            if (c == null) return;
            while (c.moveToNext() && out.length() <= 20000) {
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
                    listTree(tree, id, rel, out, depth + 1);
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
            if (pickerOpen || request == null) return;
            JSONArray mimes = new JSONArray();
            try { if (mimeJson != null && !mimeJson.isEmpty()) mimes = new JSONArray(mimeJson); } catch (Throwable ignored) { }
            startPicker(request, kind == null ? "open" : kind, multiple, mimes, suggested);
        }

        @JavascriptInterface
        public String readPicked(String key, int offset) {
            Uri uri = pickedUris.get(key);
            if (uri == null) return null;
            try (InputStream in = getContentResolver().openInputStream(uri)) {
                if (in == null) return null;
                skip(in, offset);
                byte[] buf = new byte[512 * 1024];
                int n = in.read(buf);
                if (n <= 0) return "";
                return Base64.encodeToString(buf, 0, n, Base64.NO_WRAP);
            } catch (Throwable t) {
                return null;
            }
        }

        @JavascriptInterface
        public String writePicked(String key, String b64, int offset, boolean last) {
            Uri uri = pickedUris.get(key);
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

        @JavascriptInterface
        public void startTermux() {
            runOnUiThread(() -> {
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
            });
        }

        @JavascriptInterface
        public void retry() {
            runOnUiThread(() -> load(originalUrl));
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
