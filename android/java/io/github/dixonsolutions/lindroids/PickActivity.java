package io.github.dixonsolutions.lindroids;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.ClipData;
import android.content.ContentResolver;
import android.content.Context;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;
import android.os.Environment;
import android.provider.DocumentsContract;
import android.provider.OpenableColumns;
import android.util.Log;

import java.util.ArrayList;

/**
 * Android's document picker for Linux programs (the FileChooser portal): open one or more
 * documents, a folder, or a place to save. Answers with every picked document's URI, name, size,
 * type, and where Rust can work out a path for it, the provider and document id.
 */
public final class PickActivity extends Activity {
    int link, request;
    boolean save, directory;

    static void start(Context ctx, int link, int request, boolean save, boolean multiple, boolean directory,
                      String[] mimes, String name) {
        Intent i = new Intent(ctx, PickActivity.class)
                .putExtra("link", link).putExtra("request", request)
                .putExtra("save", save).putExtra("multiple", multiple).putExtra("directory", directory)
                .putExtra("mimes", mimes).putExtra("name", name);
        i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        try {
            Portal.backgroundStartCheck(ctx);
            ctx.startActivity(i);
        } catch (Throwable t) {
            Log.w(LinkService.TAG, "cannot open the picker", t);
            answer(link, request, new ArrayList<>());
        }
    }

    @Override protected void onCreate(Bundle b) {
        super.onCreate(b);
        Intent in = getIntent();
        link = in.getIntExtra("link", -1);
        request = in.getIntExtra("request", 0);
        save = in.getBooleanExtra("save", false);
        directory = in.getBooleanExtra("directory", false);
        // Recreated (rotation, process kept) while the picker is up: it answers this instance.
        if (b != null) return;
        String[] mimes = in.getStringArrayExtra("mimes");
        if (mimes == null) mimes = new String[0];
        Intent pick;
        if (save) {
            pick = new Intent(Intent.ACTION_CREATE_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE)
                    .setType(mimes.length > 0 ? mimes[0] : "*/*");
            String name = in.getStringExtra("name");
            if (name != null) pick.putExtra(Intent.EXTRA_TITLE, name);
        } else if (directory) {
            pick = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
        } else {
            pick = new Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE)
                    .putExtra(Intent.EXTRA_ALLOW_MULTIPLE, in.getBooleanExtra("multiple", false));
            if (mimes.length == 1) {
                pick.setType(mimes[0]);
            } else {
                pick.setType("*/*");
                if (mimes.length > 1) pick.putExtra(Intent.EXTRA_MIME_TYPES, mimes);
            }
        }
        try {
            startActivityForResult(pick, 1);
        } catch (ActivityNotFoundException e) {
            Log.w(LinkService.TAG, "no document picker", e);
            answer(link, request, new ArrayList<>());
            finish();
        }
    }

    @Override protected void onActivityResult(int code, int result, Intent data) {
        ArrayList<Uri> uris = new ArrayList<>();
        if (result == RESULT_OK && data != null) {
            ClipData clip = data.getClipData();
            if (clip != null && !save && !directory) {
                for (int i = 0; i < clip.getItemCount(); i++) if (clip.getItemAt(i).getUri() != null) uris.add(clip.getItemAt(i).getUri());
            } else if (data.getData() != null) {
                uris.add(data.getData());
            }
            // Keep access across restarts of the app (Linux programs keep the paths and URIs).
            int want = Intent.FLAG_GRANT_READ_URI_PERMISSION | (save || directory ? Intent.FLAG_GRANT_WRITE_URI_PERMISSION : 0);
            int take = want & data.getFlags();
            for (Uri u : uris) {
                try {
                    getContentResolver().takePersistableUriPermission(u, take);
                } catch (RuntimeException e) {
                    Log.w(LinkService.TAG, "no lasting access to " + u, e);
                }
            }
        }
        final int l = link, r = request;
        final boolean tree = directory;
        final ContentResolver cr = getApplicationContext().getContentResolver();
        finish();
        // Providers may be slow to describe their documents (cloud storage): not on the main thread.
        new Thread(() -> answer(l, r, describe(cr, uris, tree)), "lindroids-picked").start();
    }

    /** One row per document: uri, name, size, mime, authority, document id. */
    static ArrayList<String[]> describe(ContentResolver cr, ArrayList<Uri> uris, boolean tree) {
        ArrayList<String[]> out = new ArrayList<>();
        for (Uri u : uris) {
            String docId = null, name = null, mime = null, size = "0";
            Uri doc = u;
            try {
                if (tree) {
                    docId = DocumentsContract.getTreeDocumentId(u);
                    doc = DocumentsContract.buildDocumentUriUsingTree(u, docId);
                } else {
                    docId = DocumentsContract.getDocumentId(u);
                }
            } catch (RuntimeException ignored) {
                // Not a documents provider URI: no id, no path.
            }
            try (Cursor c = cr.query(doc, new String[]{OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE}, null, null, null)) {
                if (c != null && c.moveToFirst()) {
                    if (!c.isNull(0)) name = c.getString(0);
                    if (!c.isNull(1)) size = Long.toString(c.getLong(1));
                }
            } catch (RuntimeException e) {
                Log.w(LinkService.TAG, "cannot describe " + u, e);
            }
            try {
                mime = tree ? DocumentsContract.Document.MIME_TYPE_DIR : cr.getType(doc);
            } catch (RuntimeException ignored) {}
            out.add(new String[]{u.toString(), name, size, mime, u.getAuthority(), docId});
        }
        return out;
    }

    /** getExternalStorageDirectory is deprecated for reading files, not for naming the volume. */
    @SuppressWarnings("deprecation")
    static void answer(int link, int request, ArrayList<String[]> rows) {
        int n = rows.size();
        String[] uris = new String[n], names = new String[n], mimes = new String[n], auths = new String[n], ids = new String[n];
        long[] sizes = new long[n];
        for (int i = 0; i < n; i++) {
            String[] r = rows.get(i);
            uris[i] = r[0]; names[i] = r[1]; mimes[i] = r[3]; auths[i] = r[4]; ids[i] = r[5];
            try { sizes[i] = Long.parseLong(r[2]); } catch (NumberFormatException e) { sizes[i] = 0; }
        }
        Native.linkFilesPicked(link, request, uris, names, sizes, mimes, auths, ids,
                Environment.getExternalStorageDirectory().getPath());
    }
}
