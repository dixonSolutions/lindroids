/*
 * liblindroids-gtk.so: GTK 3's own file dialogs become the desktop portal's, which Lindroids
 * answers with Android's picker. LD_PRELOADed into the programs Lindroids runs.
 *
 * GTK 3 asks the portal (with GTK_USE_PORTAL=1) only for a GtkFileChooserNative. Most GTK 3
 * programs (Mousepad, Geany, GIMP, Xournal++, Inkscape, LibreOffice...) build a
 * GtkFileChooserDialog instead, which is GTK's own "Open File" window browsing the Linux side. So
 * such a dialog is never shown: when the program runs it (gtk_dialog_run) or shows it
 * (gtk_widget_show, gtk_widget_show_all, gtk_window_present), a GtkFileChooserNative with the same
 * action, title, parent, filters, multiple selection, file name and folder goes to the portal
 * instead, and its answer comes back as the dialog's: gtk_dialog_run returns the dialog's accept
 * response (or Cancel), a shown dialog emits "response", and the program reads the files with
 * gtk_file_chooser_get_filename(s)/get_uri(s)/get_file(s), which answer with the picked files.
 *
 * Only GTK 3, only under Lindroids (LINDROIDS_RUNTIME_DIR set), only open, save and folder
 * dialogs; LINDROIDS_GTK_DIALOGS=1 keeps GTK's own. Where the portal cannot be used, GTK's
 * native dialog falls back to its own dialog, which then shows as usual.
 *
 * No libc and no link to GTK: every function is looked up with dlsym, so one file serves bionic
 * (Termux) and glibc (Flatpak, distros) programs, and programs without GTK are left alone.
 * Build: clang --target=<arch>-linux-gnu -O2 -fPIC -shared -nostdlib -o ... this.c
 * License: MIT OR Apache-2.0, like the rest of Lindroids.
 */

typedef unsigned long GType;
typedef int gboolean;
typedef unsigned long gulong;
typedef struct _GSList {
    void *data;
    struct _GSList *next;
} GSList;
typedef void (*GDestroyNotify)(void *);
typedef void (*GCallback)(void);

/* From whichever libc the program has; weak, so a missing one is a null pointer. */
extern void *dlsym(void *handle, const char *name) __attribute__((weak));
extern char *getenv(const char *name) __attribute__((weak));
/* Only bionic has it: tells bionic's RTLD_* values from glibc's on 32-bit. */
extern int __system_property_get(const char *name, char *value) __attribute__((weak));

#define EXPORT __attribute__((visibility("default")))

enum { RESPONSE_NONE = -1, RESPONSE_ACCEPT = -3, RESPONSE_OK = -5, RESPONSE_CANCEL = -6, RESPONSE_YES = -8, RESPONSE_APPLY = -10 };
enum { ACTION_OPEN = 0, ACTION_SAVE = 1, ACTION_SELECT_FOLDER = 2 };
/* G_SIGNAL_MATCH_ID | G_SIGNAL_MATCH_DATA */
#define MATCH_ID_DATA (1 | 16)

static void *rtld_next(void)
{
    if (sizeof(void *) == 4 && __system_property_get)
        return (void *)0xfffffffeUL;
    return (void *)-1L;
}

static void *rtld_default(void)
{
    if (sizeof(void *) == 4 && __system_property_get)
        return (void *)0xffffffffUL;
    return (void *)0;
}

/* GTK's own function `name` (behind this library's), or 0. */
static void *next(const char *name)
{
    return dlsym ? dlsym(rtld_next(), name) : 0;
}

/* A function of the program's GLib/GTK, or 0. */
static void *fn(const char *name)
{
    return dlsym ? dlsym(rtld_default(), name) : 0;
}

/* Calls the program's `name`, looked up once per call site. */
#define CALL(ret, name, args, ...)                     \
    ({                                                 \
        static void *p_;                               \
        if (!p_)                                       \
            p_ = fn(#name);                            \
        ((ret(*) args)p_)(__VA_ARGS__);                \
    })
/* GTK's own `name`, behind this library's, looked up once. */
#define NEXT(type, name)                               \
    ({                                                 \
        static void *p_;                               \
        if (!p_)                                       \
            p_ = next(#name);                          \
        (type)p_;                                      \
    })

static int streq(const char *a, const char *b)
{
    while (*a && *a == *b)
        a++, b++;
    return *a == *b;
}

static const char KEY_FILES[] = "lindroids-picked-files";
static const char KEY_NATIVE[] = "lindroids-native-dialog";
/* The file the program named for a save dialog (gtk_file_chooser_set_file and friends), which an
 * unshown GTK dialog does not hand back until it has read the folder. */
static const char KEY_NAMED[] = "lindroids-named-file";

/* 1 on, 0 off, -1 not decided yet. */
static int enabled = -1;
/* Inside our own native dialog's show/run: its fallback dialog is GTK's to show. */
static int in_native;

static int on(void)
{
    if (enabled >= 0)
        return enabled;
    enabled = 0;
    if (!dlsym || !getenv || !getenv("LINDROIDS_RUNTIME_DIR"))
        return 0;
    const char *own = getenv("LINDROIDS_GTK_DIALOGS");
    if (own && streq(own, "1"))
        return 0;
    unsigned (*major)(void) = (unsigned (*)(void))fn("gtk_get_major_version");
    if (!major || major() != 3 || !fn("gtk_file_chooser_native_new") || !fn("g_signal_handler_find"))
        return 0;
    enabled = 1;
    return 1;
}

/* A GtkFileChooserDialog for open, save or a folder, that is ours to answer. */
static int ours(void *w)
{
    static GType type;
    if (!w || in_native || !on())
        return 0;
    if (!type)
        type = CALL(GType, g_type_from_name, (const char *), "GtkFileChooserDialog");
    if (!type || !CALL(gboolean, g_type_check_instance_is_a, (void *, GType), w, type))
        return 0;
    int action = CALL(int, gtk_file_chooser_get_action, (void *), w);
    return action == ACTION_OPEN || action == ACTION_SAVE || action == ACTION_SELECT_FOLDER;
}

/* A native (portal) dialog asking what `dlg` asks. */
static void *native_for(void *dlg)
{
    int action = CALL(int, gtk_file_chooser_get_action, (void *), dlg);
    const char *title = CALL(const char *, gtk_window_get_title, (void *), dlg);
    void *parent = CALL(void *, gtk_window_get_transient_for, (void *), dlg);
    void *n = CALL(void *, gtk_file_chooser_native_new, (const char *, void *, int, const char *, const char *), title, parent, action, 0, 0);
    if (!n)
        return 0;
    CALL(void, gtk_native_dialog_set_modal, (void *, gboolean), n, 1);
    CALL(void, gtk_file_chooser_set_select_multiple, (void *, gboolean), n, CALL(gboolean, gtk_file_chooser_get_select_multiple, (void *), dlg));
    GSList *filters = CALL(GSList *, gtk_file_chooser_list_filters, (void *), dlg);
    for (GSList *l = filters; l; l = l->next)
        CALL(void, gtk_file_chooser_add_filter, (void *, void *), n, l->data);
    CALL(void, g_slist_free, (GSList *), filters);
    void *filter = CALL(void *, gtk_file_chooser_get_filter, (void *), dlg);
    if (filter)
        CALL(void, gtk_file_chooser_set_filter, (void *, void *), n, filter);
    if (action == ACTION_SAVE) {
        char *name = fn("gtk_file_chooser_get_current_name") ? CALL(char *, gtk_file_chooser_get_current_name, (void *), dlg) : 0;
        void *named = CALL(void *, g_object_get_data, (void *, const char *), dlg, KEY_NAMED);
        if ((!name || !*name) && named) {
            CALL(void, g_free, (void *), name);
            name = CALL(char *, g_file_get_basename, (void *), named);
        }
        if (name && *name)
            CALL(void, gtk_file_chooser_set_current_name, (void *, const char *), n, name);
        CALL(void, g_free, (void *), name);
        CALL(void, gtk_file_chooser_set_do_overwrite_confirmation, (void *, gboolean), n, 1);
    }
    void *folder = CALL(void *, gtk_file_chooser_get_current_folder_file, (void *), dlg);
    if (folder) {
        CALL(gboolean, gtk_file_chooser_set_current_folder_file, (void *, void *, void **), n, folder, 0);
        CALL(void, g_object_unref, (void *), folder);
    }
    return n;
}

static void free_files(void *list)
{
    CALL(void, g_slist_free_full, (GSList *, GDestroyNotify), (GSList *)list, (GDestroyNotify)fn("g_object_unref"));
}

/* The response the program waits for when the user picked something: the dialog's own. */
static int accept_response(void *dlg)
{
    static const int ids[] = {RESPONSE_ACCEPT, RESPONSE_OK, RESPONSE_YES, RESPONSE_APPLY};
    for (unsigned i = 0; i < sizeof ids / sizeof ids[0]; i++)
        if (CALL(void *, gtk_dialog_get_widget_for_response, (void *, int), dlg, ids[i]))
            return ids[i];
    void *def = CALL(void *, gtk_window_get_default_widget, (void *), dlg);
    if (def) {
        int r = CALL(int, gtk_dialog_get_response_for_widget, (void *, void *), dlg, def);
        if (r != RESPONSE_NONE)
            return r;
    }
    return RESPONSE_ACCEPT;
}

/* The native dialog answered `r`: its files become the dialog's; the dialog's response. */
static int answered(void *dlg, void *n, int r)
{
    if (r == RESPONSE_ACCEPT) {
        GSList *(*get_files)(void *) = NEXT(GSList * (*)(void *), gtk_file_chooser_get_files);
        GSList *files = get_files ? get_files(n) : 0;
        CALL(void, g_object_set_data_full, (void *, const char *, void *, GDestroyNotify), dlg, KEY_FILES, files, free_files);
        if (files)
            return accept_response(dlg);
    } else {
        CALL(void, g_object_set_data_full, (void *, const char *, void *, GDestroyNotify), dlg, KEY_FILES, 0, 0);
    }
    return RESPONSE_CANCEL;
}

static GSList *picked(void *chooser)
{
    if (!chooser || enabled != 1)
        return 0;
    return CALL(GSList *, g_object_get_data, (void *, const char *), chooser, KEY_FILES);
}

/* gtk_dialog_run: a file dialog of ours is the portal's, run until it answers. */
EXPORT int gtk_dialog_run(void *dlg)
{
    int (*run)(void *) = NEXT(int (*)(void *), gtk_dialog_run);
    if (!ours(dlg))
        return run ? run(dlg) : RESPONSE_NONE;
    void *n = native_for(dlg);
    if (!n)
        return run ? run(dlg) : RESPONSE_NONE;
    CALL(void *, g_object_ref, (void *), dlg);
    in_native++;
    int r = CALL(int, gtk_native_dialog_run, (void *), n);
    in_native--;
    int out = answered(dlg, n, r);
    CALL(void, gtk_native_dialog_destroy, (void *), n);
    CALL(void, g_object_unref, (void *), n);
    CALL(void, g_object_unref, (void *), dlg);
    return out;
}

/* The native dialog of a shown file dialog answered: the dialog responds. GtkFileChooserDialog's
 * own first "response" handler would stop an accept its (never shown) file list does not hold,
 * so it is held back meanwhile. */
static void native_response(void *n, int r, void *dlg)
{
    int out = answered(dlg, n, r);
    unsigned sig = CALL(unsigned, g_signal_lookup, (const char *, GType), "response", CALL(GType, gtk_dialog_get_type, (void)));
    gulong guard = CALL(gulong, g_signal_handler_find, (void *, int, unsigned, unsigned, void *, void *, void *), dlg, MATCH_ID_DATA, sig, 0, 0, 0, 0);
    if (guard)
        CALL(void, g_signal_handler_block, (void *, gulong), dlg, guard);
    CALL(void, gtk_dialog_response, (void *, int), dlg, out);
    if (guard && CALL(gboolean, g_signal_handler_is_connected, (void *, gulong), dlg, guard))
        CALL(void, g_signal_handler_unblock, (void *, gulong), dlg, guard);
    CALL(void, gtk_native_dialog_destroy, (void *), n);
    /* Drops our reference to the native dialog (the emission holds one until it ends). */
    CALL(void, g_object_set_data_full, (void *, const char *, void *, GDestroyNotify), dlg, KEY_NATIVE, 0, 0);
    CALL(void, g_object_unref, (void *), dlg);
}

/* A file dialog of ours being shown: the portal's instead. True when it was ours. */
static int show_native(void *dlg)
{
    if (!ours(dlg))
        return 0;
    if (CALL(void *, g_object_get_data, (void *, const char *), dlg, KEY_NATIVE))
        return 1; /* asked already, waiting for the answer */
    void *n = native_for(dlg);
    if (!n)
        return 0;
    CALL(void *, g_object_ref, (void *), dlg);
    CALL(void, g_object_set_data_full, (void *, const char *, void *, GDestroyNotify), dlg, KEY_NATIVE, n, (GDestroyNotify)fn("g_object_unref"));
    CALL(gulong, g_signal_connect_data, (void *, const char *, GCallback, void *, void *, int), n, "response", (GCallback)native_response, dlg, 0, 0);
    in_native++;
    CALL(void, gtk_native_dialog_show, (void *), n);
    in_native--;
    return 1;
}

EXPORT void gtk_widget_show(void *w)
{
    if (show_native(w))
        return;
    void (*f)(void *) = NEXT(void (*)(void *), gtk_widget_show);
    if (f)
        f(w);
}

EXPORT void gtk_widget_show_all(void *w)
{
    if (show_native(w))
        return;
    void (*f)(void *) = NEXT(void (*)(void *), gtk_widget_show_all);
    if (f)
        f(w);
}

EXPORT void gtk_window_present(void *w)
{
    if (show_native(w))
        return;
    void (*f)(void *) = NEXT(void (*)(void *), gtk_window_present);
    if (f)
        f(w);
}

EXPORT void gtk_window_present_with_time(void *w, unsigned t)
{
    if (show_native(w))
        return;
    void (*f)(void *, unsigned) = NEXT(void (*)(void *, unsigned), gtk_window_present_with_time);
    if (f)
        f(w, t);
}

/* What the program names for saving: kept for the portal's dialog (see KEY_NAMED). */
static void named(void *c, void *file)
{
    if (c && file && enabled == 1)
        CALL(void, g_object_set_data_full, (void *, const char *, void *, GDestroyNotify), c, KEY_NAMED, file, (GDestroyNotify)fn("g_object_unref"));
    else if (file)
        CALL(void, g_object_unref, (void *), file);
}

EXPORT gboolean gtk_file_chooser_set_file(void *c, void *file, void **error)
{
    gboolean (*f)(void *, void *, void **) = NEXT(gboolean(*)(void *, void *, void **), gtk_file_chooser_set_file);
    if (on() && file)
        named(c, CALL(void *, g_object_ref, (void *), file));
    return f ? f(c, file, error) : 0;
}

EXPORT gboolean gtk_file_chooser_set_filename(void *c, const char *path)
{
    gboolean (*f)(void *, const char *) = NEXT(gboolean(*)(void *, const char *), gtk_file_chooser_set_filename);
    if (on() && path)
        named(c, CALL(void *, g_file_new_for_path, (const char *), path));
    return f ? f(c, path) : 0;
}

EXPORT gboolean gtk_file_chooser_set_uri(void *c, const char *uri)
{
    gboolean (*f)(void *, const char *) = NEXT(gboolean(*)(void *, const char *), gtk_file_chooser_set_uri);
    if (on() && uri)
        named(c, CALL(void *, g_file_new_for_uri, (const char *), uri));
    return f ? f(c, uri) : 0;
}

/* What the program reads back: the picked files, where the portal answered for the dialog. */

EXPORT GSList *gtk_file_chooser_get_files(void *c)
{
    GSList *p = picked(c);
    if (!p) {
        GSList *(*f)(void *) = NEXT(GSList * (*)(void *), gtk_file_chooser_get_files);
        return f ? f(c) : 0;
    }
    GSList *out = 0;
    for (; p; p = p->next)
        out = CALL(GSList *, g_slist_append, (GSList *, void *), out, CALL(void *, g_object_ref, (void *), p->data));
    return out;
}

EXPORT void *gtk_file_chooser_get_file(void *c)
{
    GSList *p = picked(c);
    if (!p) {
        void *(*f)(void *) = NEXT(void *(*)(void *), gtk_file_chooser_get_file);
        return f ? f(c) : 0;
    }
    return CALL(void *, g_object_ref, (void *), p->data);
}

EXPORT GSList *gtk_file_chooser_get_filenames(void *c)
{
    GSList *p = picked(c);
    if (!p) {
        GSList *(*f)(void *) = NEXT(GSList * (*)(void *), gtk_file_chooser_get_filenames);
        return f ? f(c) : 0;
    }
    GSList *out = 0;
    for (; p; p = p->next) {
        char *s = CALL(char *, g_file_get_path, (void *), p->data);
        if (s)
            out = CALL(GSList *, g_slist_append, (GSList *, void *), out, s);
    }
    return out;
}

EXPORT char *gtk_file_chooser_get_filename(void *c)
{
    GSList *p = picked(c);
    if (!p) {
        char *(*f)(void *) = NEXT(char *(*)(void *), gtk_file_chooser_get_filename);
        return f ? f(c) : 0;
    }
    return CALL(char *, g_file_get_path, (void *), p->data);
}

EXPORT GSList *gtk_file_chooser_get_uris(void *c)
{
    GSList *p = picked(c);
    if (!p) {
        GSList *(*f)(void *) = NEXT(GSList * (*)(void *), gtk_file_chooser_get_uris);
        return f ? f(c) : 0;
    }
    GSList *out = 0;
    for (; p; p = p->next)
        out = CALL(GSList *, g_slist_append, (GSList *, void *), out, CALL(char *, g_file_get_uri, (void *), p->data));
    return out;
}

EXPORT char *gtk_file_chooser_get_uri(void *c)
{
    GSList *p = picked(c);
    if (!p) {
        char *(*f)(void *) = NEXT(char *(*)(void *), gtk_file_chooser_get_uri);
        return f ? f(c) : 0;
    }
    return CALL(char *, g_file_get_uri, (void *), p->data);
}
