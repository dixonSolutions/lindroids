package io.github.dixonsolutions.lindroids;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.content.Context;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.OpenableColumns;

import java.io.File;
import java.io.FileNotFoundException;
import java.io.IOException;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Shares the files Linux programs open with Android apps (OpenFile): read-only, only what is in
 * {@code cacheDir/open}, only to an app the user picked and that was granted the URI. Not
 * exported. A small FileProvider of our own: the APK is built without AndroidX.
 *
 * URIs are {@code content://<package>.files/open/<dir>/<name>}.
 */
public final class FilesProvider extends ContentProvider {
    /** Types the Linux side gave, by URI; otherwise the type is guessed from the name. */
    private static final Map<String, String> types = new ConcurrentHashMap<>();

    static String authority(Context ctx) { return ctx.getPackageName() + ".files"; }

    static File root(Context ctx) { return new File(ctx.getCacheDir(), "open"); }

    /** The URI for a file under {@link #root}, or null if it is anywhere else. */
    static Uri uriFor(Context ctx, File f) {
        try {
            String base = root(ctx).getCanonicalPath() + "/";
            String path = f.getCanonicalPath();
            if (!path.startsWith(base)) return null;
            Uri.Builder b = new Uri.Builder().scheme("content").authority(authority(ctx)).appendPath("open");
            for (String seg : path.substring(base.length()).split("/")) b.appendPath(seg);
            return b.build();
        } catch (IOException e) {
            return null;
        }
    }

    static void setType(Uri uri, String mime) { types.put(uri.toString(), mime); }

    /** The file a URI names, or null if it is not one of ours. */
    private File fileFor(Uri uri) {
        Context ctx = getContext();
        if (ctx == null || !authority(ctx).equals(uri.getAuthority())) return null;
        java.util.List<String> segs = uri.getPathSegments();
        if (segs.size() < 2 || !segs.get(0).equals("open")) return null;
        File f = root(ctx);
        for (String s : segs.subList(1, segs.size())) {
            if (s.isEmpty() || s.equals(".") || s.equals("..") || s.contains("/")) return null;
            f = new File(f, s);
        }
        try {
            // Symlinks or anything else that leads out of the directory.
            if (!f.getCanonicalPath().startsWith(root(ctx).getCanonicalPath() + "/")) return null;
        } catch (IOException e) {
            return null;
        }
        return f.isFile() ? f : null;
    }

    @Override public boolean onCreate() { return true; }

    @Override public String getType(Uri uri) {
        String t = types.get(uri.toString());
        if (t != null) return t;
        File f = fileFor(uri);
        return f == null ? null : Portal.guessMime(f.getName());
    }

    @Override public ParcelFileDescriptor openFile(Uri uri, String mode) throws FileNotFoundException {
        if (!"r".equals(mode)) throw new SecurityException("read-only");
        File f = fileFor(uri);
        if (f == null) throw new FileNotFoundException(uri.toString());
        return ParcelFileDescriptor.open(f, ParcelFileDescriptor.MODE_READ_ONLY);
    }

    @Override public Cursor query(Uri uri, String[] projection, String selection, String[] args, String sort) {
        File f = fileFor(uri);
        if (f == null) return null;
        if (projection == null) projection = new String[]{OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE};
        MatrixCursor c = new MatrixCursor(projection, 1);
        Object[] row = new Object[projection.length];
        for (int i = 0; i < projection.length; i++) {
            if (OpenableColumns.DISPLAY_NAME.equals(projection[i])) row[i] = f.getName();
            else if (OpenableColumns.SIZE.equals(projection[i])) row[i] = f.length();
        }
        c.addRow(row);
        return c;
    }

    @Override public Uri insert(Uri uri, ContentValues values) { throw new UnsupportedOperationException("read-only"); }

    @Override public int delete(Uri uri, String selection, String[] args) { throw new UnsupportedOperationException("read-only"); }

    @Override public int update(Uri uri, ContentValues values, String selection, String[] args) {
        throw new UnsupportedOperationException("read-only");
    }
}
