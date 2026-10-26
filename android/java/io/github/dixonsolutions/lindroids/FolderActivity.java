package io.github.dixonsolutions.lindroids;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;
import android.os.Environment;
import android.provider.DocumentsContract;

/** Android's folder picker, for `lindroids pick-folder`; answers with a filesystem path. */
public final class FolderActivity extends Activity {
    int link, request;

    static void start(Context ctx, int link, int request) {
        Intent i = new Intent(ctx, FolderActivity.class).putExtra("link", link).putExtra("request", request);
        i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        try {
            ctx.startActivity(i);
        } catch (Throwable t) {
            Native.linkFolderPicked(link, request, null);
        }
    }

    @Override protected void onCreate(Bundle b) {
        super.onCreate(b);
        link = getIntent().getIntExtra("link", -1);
        request = getIntent().getIntExtra("request", 0);
        startActivityForResult(new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE), 1);
    }

    @Override protected void onActivityResult(int code, int result, Intent data) {
        String path = null;
        if (result == RESULT_OK && data != null && data.getData() != null) path = toPath(data.getData());
        Native.linkFolderPicked(link, request, path);
        finish();
    }

    /** "primary:Documents/x" -> /storage/emulated/0/Documents/x ; "ABCD-1234:Music" -> /storage/ABCD-1234/Music */
    static String toPath(Uri tree) {
        try {
            String doc = DocumentsContract.getTreeDocumentId(tree);
            int colon = doc.indexOf(':');
            String vol = colon < 0 ? doc : doc.substring(0, colon);
            String rel = colon < 0 ? "" : doc.substring(colon + 1);
            String root = vol.equals("primary") ? Environment.getExternalStorageDirectory().getPath() : "/storage/" + vol;
            return rel.isEmpty() ? root : root + "/" + rel;
        } catch (Exception e) {
            return null;
        }
    }
}
