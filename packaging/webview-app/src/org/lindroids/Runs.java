package org.lindroids;

import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.net.Uri;

import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.charset.StandardCharsets;

/**
 * Which run each of the app's tasks shows, and its ticket: kept by the page's bridge
 * ({@code attached}) so that the app can still speak for a window whose page is gone (its task
 * swiped from Recents: {@link CloseWatch}; the run ended: {@link End}; a card left over from an
 * earlier launch). A task is known by its base intent: a run's window by its URL, a program's own
 * app's single task ({@link Launch}) as "launch". A window's URL carries its run and ticket too
 * ({@code ?window&run=<pid>#t=<ticket>}), which is what is used when nothing was kept.
 */
final class Runs {
    private Runs() {}

    private static final String PREFS = "org.lindroids.runs";

    /** The key of the task whose base intent this is. */
    static String key(Intent base) {
        if (base == null) return "";
        ComponentName c = base.getComponent();
        if (c != null && c.getClassName().endsWith(".Launch")) return "launch";
        String u = base.getDataString();
        if (u == null) u = base.getStringExtra(WindowActivity.EXTRA_URL);
        if (u == null) return "";
        int hash = u.indexOf('#');
        return hash < 0 ? u : u.substring(0, hash);
    }

    private static SharedPreferences prefs(Context c) {
        return c.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    static void remember(Context c, String key, String run, String ticket, String host) {
        if (key.isEmpty()) return;
        prefs(c).edit().putString(key, run + "\n" + ticket + "\n" + host).apply();
    }

    static void forget(Context c, String key) {
        if (!key.isEmpty()) prefs(c).edit().remove(key).apply();
    }

    /** {run, ticket, host:port} of the task whose base intent this is, or null. */
    static String[] of(Context c, Intent base) {
        String kept = prefs(c).getString(key(base), null);
        if (kept != null) {
            String[] r = kept.split("\n", -1);
            if (r.length == 3 && !r[0].isEmpty() && !r[1].isEmpty()) return r;
        }
        String u = base == null ? null : base.getDataString();
        if (u == null && base != null) u = base.getStringExtra(WindowActivity.EXTRA_URL);
        return ofUrl(u);
    }

    /** {run, ticket, host:port} from a window's URL, or null. */
    static String[] ofUrl(String url) {
        if (url == null) return null;
        Uri u = Uri.parse(url);
        if (!WindowActivity.local(u)) return null;
        String run = u.getQueryParameter("run");
        String frag = u.getEncodedFragment();
        String ticket = null;
        if (frag != null) {
            for (String kv : frag.split("&")) {
                if (kv.startsWith("t=")) ticket = Uri.decode(kv.substring(2));
            }
        }
        if (run == null || run.isEmpty() || ticket == null || ticket.isEmpty()) return null;
        return new String[]{run, ticket, u.getHost() + ":" + (u.getPort() > 0 ? u.getPort() : 80)};
    }

    private static String oneLine(String s) {
        return s.replace("\r", "").replace("\n", "");
    }

    /**
     * Tells the server about run {@code r} ({@code closed}: its window is gone for good;
     * {@code alive}: does it still run?). Returns the HTTP status (200: it runs; 410: it is over),
     * or -1 when the server cannot be reached. Blocks: never on the main thread. A plain socket to
     * the phone's loopback, with the page's own host name.
     */
    static int tell(String[] r, String what) {
        String host = r[2];
        int colon = host.lastIndexOf(':');
        int port;
        try {
            port = Integer.parseInt(host.substring(colon + 1));
        } catch (Throwable t) {
            return -1;
        }
        try (Socket s = new Socket()) {
            s.connect(new InetSocketAddress("127.0.0.1", port), 1500);
            s.setSoTimeout(3000);
            String req = "GET /run/" + Uri.encode(r[0]) + "/" + what + " HTTP/1.1\r\nHost: " + oneLine(host)
                    + "\r\nX-Lindroids-Ticket: " + oneLine(r[1]) + "\r\nConnection: close\r\n\r\n";
            OutputStream out = s.getOutputStream();
            out.write(req.getBytes(StandardCharsets.UTF_8));
            out.flush();
            InputStream in = s.getInputStream();
            byte[] buf = new byte[64];
            int n = 0;
            while (n < 12) {
                int k = in.read(buf, n, buf.length - n);
                if (k <= 0) break;
                n += k;
            }
            String head = new String(buf, 0, n, StandardCharsets.US_ASCII);
            // "HTTP/1.1 200 ..."
            return head.length() >= 12 ? Integer.parseInt(head.substring(9, 12)) : -1;
        } catch (Throwable t) {
            return -1;
        }
    }
}
