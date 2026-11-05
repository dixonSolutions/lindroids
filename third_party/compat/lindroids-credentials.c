/* Linux/glibc fake-root credentials for GIO's D-Bus handshake. Cortex's getuid/getgid
 * return 0, but SCM_CREDENTIALS must name the kernel user or sendmsg fails with EPERM,
 * before the bus can even offer ANONYMOUS authentication. Only replace credentials
 * naming this process's apparent identity; leave all other messages untouched.
 * Built without libc headers, like the other preload helpers. MIT OR Apache-2.0.
 */
typedef unsigned long usize;
struct message {
    void *name; unsigned int namelen;
    void *iov; usize iovlen;
    void *control; usize controllen;
    int flags;
};
struct control { usize len; int level; int type; };
struct credentials { int pid; unsigned int uid, gid; };
extern void *dlsym(void *, const char *);
extern long syscall(long, ...);
extern unsigned int getuid(void), getgid(void);
extern int getpid(void);
extern int *__errno_location(void);

#if defined(__x86_64__)
#define GETPID 39
#define GETUID 102
#define GETGID 104
#elif defined(__aarch64__)
#define GETPID 172
#define GETUID 174
#define GETGID 176
#elif defined(__arm__) || defined(__i386__)
#define GETPID 20
#define GETUID 199
#define GETGID 200
#else
#error unsupported architecture
#endif
#define ALIGN(n) (((n) + sizeof(usize) - 1) & ~(sizeof(usize) - 1))

__attribute__((visibility("default")))
long sendmsg(int fd, const struct message *msg, int flags)
{
    long (*next)(int, const struct message *, int) = dlsym((void *)-1L, "sendmsg");
    if (!next) { *__errno_location() = 38; return -1; }
    if (!msg || !msg->control || msg->controllen > 4096)
        return next(fd, msg, flags);
    /* Align the copied control buffer for cmsghdr on both 32 and 64 bit. */
    usize copy[4096 / sizeof(usize)];
    unsigned char *buf = (unsigned char *)copy;
    for (usize i = 0; i < msg->controllen; ++i)
        buf[i] = ((const unsigned char *)msg->control)[i];
    int changed = 0;
    for (usize off = 0; off + sizeof(struct control) <= msg->controllen;) {
        struct control *c = (struct control *)(buf + off);
        if (c->len < ALIGN(sizeof(*c)) || c->len > msg->controllen - off) break;
        if (c->level == 1 /* SOL_SOCKET */ && c->type == 2 /* SCM_CREDENTIALS */ &&
            c->len >= ALIGN(sizeof(*c)) + sizeof(struct credentials)) {
            struct credentials *cred = (struct credentials *)(buf + off + ALIGN(sizeof(*c)));
            if (cred->pid == getpid() && cred->uid == getuid() && cred->gid == getgid()) {
                cred->pid = (int)syscall(GETPID);
                cred->uid = (unsigned int)syscall(GETUID);
                cred->gid = (unsigned int)syscall(GETGID);
                changed = 1;
            }
        }
        off += ALIGN(c->len);
    }
    if (!changed) return next(fd, msg, flags);
    struct message fixed = *msg;
    fixed.control = copy;
    return next(fd, &fixed, flags);
}
