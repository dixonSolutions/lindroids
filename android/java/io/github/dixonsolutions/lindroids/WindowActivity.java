package io.github.dixonsolutions.lindroids;

import android.app.Activity;
import android.app.ActivityManager;
import android.content.Context;
import android.graphics.Color;
import android.graphics.drawable.GradientDrawable;
import android.os.Build;
import android.os.Bundle;
import android.text.InputType;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowManager;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;
import android.widget.FrameLayout;
import android.widget.TextView;

/**
 * One Linux toplevel window. Android composites it like any other app window: it has its own task
 * in Recents, follows split screen and freeform/desktop mode, and its Surface goes straight to the
 * server that draws into it.
 */
public final class WindowActivity extends Activity {
    int link, win;
    LinkService.Win state;
    LinuxView view;
    boolean keyboardShown;

    @Override protected void onCreate(Bundle b) {
        super.onCreate(b);
        link = getIntent().getIntExtra("link", -1);
        win = getIntent().getIntExtra("win", -1);
        state = LinkService.windows.get(LinkService.key(link, win));
        if (state == null) { finishAndRemoveTask(); return; }
        state.activity = this;
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        if (Build.VERSION.SDK_INT >= 28) {
            getWindow().getAttributes().layoutInDisplayCutoutMode =
                    WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
        }

        FrameLayout root = new FrameLayout(this);
        root.setBackgroundColor(Color.BLACK);
        view = new LinuxView(this);
        root.addView(view, new FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));

        // A small keyboard button: phones have no physical keyboard, and plenty of Linux apps do
        // not announce text fields through text-input-v3.
        TextView kb = new TextView(this);
        kb.setText("⌨");
        kb.setTextSize(TypedValue.COMPLEX_UNIT_SP, 18);
        kb.setTextColor(Color.WHITE);
        kb.setGravity(Gravity.CENTER);
        GradientDrawable bg = new GradientDrawable();
        bg.setShape(GradientDrawable.OVAL);
        bg.setColor(0x66000000);
        kb.setBackground(bg);
        kb.setAlpha(0.7f);
        kb.setContentDescription("Toggle keyboard");
        kb.setOnClickListener(v -> showKeyboard(!keyboardShown));
        int size = dp(40);
        FrameLayout.LayoutParams lp = new FrameLayout.LayoutParams(size, size, Gravity.BOTTOM | Gravity.END);
        lp.setMargins(0, 0, dp(12), dp(12));
        root.addView(kb, lp);

        setContentView(root);
        applyTaskDescription();
    }

    int dp(int v) { return (int) (v * getResources().getDisplayMetrics().density + 0.5f); }

    void applyTaskDescription() {
        if (state == null) return;
        setTitle(state.title);
        if (state.icon != null) setTaskDescription(new ActivityManager.TaskDescription(state.title, state.icon));
        else setTaskDescription(new ActivityManager.TaskDescription(state.title));
    }

    void showKeyboard(boolean show) {
        keyboardShown = show;
        InputMethodManager imm = (InputMethodManager) getSystemService(Context.INPUT_METHOD_SERVICE);
        if (show) {
            view.requestFocus();
            imm.showSoftInput(view, InputMethodManager.SHOW_FORCED);
        } else {
            imm.hideSoftInputFromWindow(view.getWindowToken(), 0);
        }
    }

    @Override public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        Native.linkFocus(link, win, hasFocus);
        if (hasFocus && state != null) state.focusedAt = android.os.SystemClock.uptimeMillis();
        if (hasFocus) shareClipboard();
    }

    /** Android only lets the focused app read the clipboard: offer it to Linux apps now. */
    void shareClipboard() {
        try {
            android.content.ClipboardManager cm = (android.content.ClipboardManager) getSystemService(CLIPBOARD_SERVICE);
            android.content.ClipData clip = cm.getPrimaryClip();
            if (clip == null || clip.getItemCount() == 0) return;
            CharSequence t = clip.getItemAt(0).coerceToText(this);
            if (t == null) return;
            String text = t.toString();
            if (text.equals(LinkService.lastClipboard)) return;
            LinkService.lastClipboard = text;
            Native.linkClipboard(link, text);
        } catch (Throwable ignored) {}
    }

    @Override protected void onDestroy() {
        if (state != null) {
            state.activity = null;
            // Swiped away from Recents, or finished by the user: ask the Linux app to close.
            if (!state.closedByServer && isFinishing()) Native.linkClose(link, win);
        }
        super.onDestroy();
    }

    @Override public void onBackPressed() {
        // Back is Escape for the Linux app; close a window from Recents.
        int t = (int) android.os.SystemClock.uptimeMillis();
        Native.linkKey(link, win, KeyEvent.KEYCODE_ESCAPE, true, t);
        Native.linkKey(link, win, KeyEvent.KEYCODE_ESCAPE, false, t);
    }

    static int now(MotionEvent e) { return (int) e.getEventTime(); }

    final class LinuxView extends SurfaceView implements SurfaceHolder.Callback {
        LinuxView(Context c) {
            super(c);
            getHolder().addCallback(this);
            setFocusable(true);
            setFocusableInTouchMode(true);
        }

        @Override public void surfaceCreated(SurfaceHolder h) {}

        @Override public void surfaceChanged(SurfaceHolder h, int format, int w, int hgt) {
            Native.windowSurface(link, win, h.getSurface());
            Native.linkResize(link, win, w, hgt, getResources().getDisplayMetrics().density);
        }

        @Override public void surfaceDestroyed(SurfaceHolder h) {
            Native.windowSurface(link, win, null);
        }

        @Override public boolean onTouchEvent(MotionEvent e) {
            if (e.isFromSource(InputDevice.SOURCE_MOUSE)) return mouse(e);
            int action = e.getActionMasked();
            int t = now(e);
            switch (action) {
                case MotionEvent.ACTION_DOWN:
                case MotionEvent.ACTION_POINTER_DOWN: {
                    int i = e.getActionIndex();
                    Native.linkTouch(link, win, 0, e.getPointerId(i), e.getX(i), e.getY(i), t);
                    break;
                }
                case MotionEvent.ACTION_MOVE:
                    for (int i = 0; i < e.getPointerCount(); i++)
                        Native.linkTouch(link, win, 1, e.getPointerId(i), e.getX(i), e.getY(i), t);
                    break;
                case MotionEvent.ACTION_UP:
                case MotionEvent.ACTION_POINTER_UP: {
                    int i = e.getActionIndex();
                    Native.linkTouch(link, win, 2, e.getPointerId(i), e.getX(i), e.getY(i), t);
                    break;
                }
                case MotionEvent.ACTION_CANCEL:
                    Native.linkTouch(link, win, 3, 0, 0, 0, t);
                    break;
            }
            return true;
        }

        int buttons;

        boolean mouse(MotionEvent e) {
            int t = now(e);
            float x = e.getX(), y = e.getY();
            switch (e.getActionMasked()) {
                case MotionEvent.ACTION_HOVER_ENTER: Native.linkPointer(link, win, 0, x, y, 0, t); break;
                case MotionEvent.ACTION_HOVER_EXIT: Native.linkPointer(link, win, 1, x, y, 0, t); break;
                case MotionEvent.ACTION_HOVER_MOVE:
                case MotionEvent.ACTION_MOVE: Native.linkPointer(link, win, 2, x, y, 0, t); break;
                case MotionEvent.ACTION_SCROLL:
                    Native.linkScroll(link, win, -e.getAxisValue(MotionEvent.AXIS_HSCROLL),
                            -e.getAxisValue(MotionEvent.AXIS_VSCROLL), t);
                    break;
                case MotionEvent.ACTION_DOWN:
                case MotionEvent.ACTION_UP:
                case MotionEvent.ACTION_BUTTON_PRESS:
                case MotionEvent.ACTION_BUTTON_RELEASE: {
                    int now = e.getButtonState();
                    // A touch-emulated mouse (and some VMs) reports no buttons: treat DOWN/UP as left.
                    if (now == 0 && e.getActionMasked() == MotionEvent.ACTION_DOWN) now = MotionEvent.BUTTON_PRIMARY;
                    Native.linkPointer(link, win, 2, x, y, 0, t);
                    int[] bits = {MotionEvent.BUTTON_PRIMARY, MotionEvent.BUTTON_SECONDARY, MotionEvent.BUTTON_TERTIARY,
                            MotionEvent.BUTTON_BACK, MotionEvent.BUTTON_FORWARD};
                    for (int i = 0; i < bits.length; i++) {
                        boolean was = (buttons & bits[i]) != 0, is = (now & bits[i]) != 0;
                        if (was != is) Native.linkPointer(link, win, is ? 3 : 4, x, y, i + 1, t);
                    }
                    buttons = now;
                    break;
                }
            }
            return true;
        }

        @Override public boolean onGenericMotionEvent(MotionEvent e) {
            if (e.isFromSource(InputDevice.SOURCE_CLASS_POINTER)) return mouse(e);
            return super.onGenericMotionEvent(e);
        }

        @Override public boolean onKeyDown(int code, KeyEvent e) { return key(code, e, true); }
        @Override public boolean onKeyUp(int code, KeyEvent e) { return key(code, e, false); }

        boolean key(int code, KeyEvent e, boolean down) {
            if (code == KeyEvent.KEYCODE_VOLUME_UP || code == KeyEvent.KEYCODE_VOLUME_DOWN
                    || code == KeyEvent.KEYCODE_VOLUME_MUTE || code == KeyEvent.KEYCODE_HOME) return false;
            if (code == KeyEvent.KEYCODE_BACK && e.getSource() != InputDevice.SOURCE_KEYBOARD) return false;
            Native.linkKey(link, win, code, down, (int) e.getEventTime());
            return true;
        }

        @Override public boolean onCheckIsTextEditor() { return true; }

        @Override public InputConnection onCreateInputConnection(EditorInfo out) {
            out.inputType = InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD
                    | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS;
            out.imeOptions = EditorInfo.IME_FLAG_NO_FULLSCREEN | EditorInfo.IME_FLAG_NO_EXTRACT_UI | EditorInfo.IME_ACTION_NONE;
            return new BaseInputConnection(this, false) {
                @Override public boolean commitText(CharSequence text, int newCursorPosition) {
                    String s = text.toString();
                    if (s.equals("\n")) tap(KeyEvent.KEYCODE_ENTER);
                    else if (!s.isEmpty()) Native.linkText(link, win, s);
                    return true;
                }

                @Override public boolean deleteSurroundingText(int before, int after) {
                    for (int i = 0; i < before; i++) tap(KeyEvent.KEYCODE_DEL);
                    for (int i = 0; i < after; i++) tap(KeyEvent.KEYCODE_FORWARD_DEL);
                    return true;
                }

                @Override public boolean sendKeyEvent(KeyEvent e) {
                    if (e.getAction() == KeyEvent.ACTION_DOWN || e.getAction() == KeyEvent.ACTION_UP)
                        Native.linkKey(link, win, e.getKeyCode(), e.getAction() == KeyEvent.ACTION_DOWN, (int) e.getEventTime());
                    return true;
                }
            };
        }

        void tap(int code) {
            int t = (int) android.os.SystemClock.uptimeMillis();
            Native.linkKey(link, win, code, true, t);
            Native.linkKey(link, win, code, false, t);
        }
    }
}
