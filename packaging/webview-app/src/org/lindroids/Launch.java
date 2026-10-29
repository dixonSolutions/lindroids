package org.lindroids;

/**
 * A program's own app, opened from its icon: the same window as {@link WindowActivity}, but one
 * task however often and from wherever it is opened (the launcher, the installer's Open), where
 * WindowActivity makes a document task per page. Its page starts the program and becomes its
 * window; a window the server opens later for a run is a WindowActivity, and closes this task.
 */
public final class Launch extends WindowActivity {
}
