/*
 * liblindroids-compat.so: LD_PRELOADed into glibc programs under proot on x86 Android.
 *
 * Android's seccomp filter for apps on x86 and x86_64 refuses the legacy alarm(2) syscall (ARM64
 * has no such syscall, so glibc there implements alarm() with setitimer). glibc on x86 calls it
 * directly, so alarm() fails with ENOSYS: xterm, for one, then dies opening /dev/tty because the
 * failed alarm(0) after the open overwrites errno. This alarm() uses setitimer(2), as musl and
 * bionic do.
 *
 * No libc: raw syscalls only, so one file serves glibc and musl distributions alike.
 * Build: clang --target={x86_64,i686}-linux-gnu -O2 -fPIC -shared -nostdlib -o ... this.c
 * License: MIT OR Apache-2.0, like the rest of Lindroids.
 */

struct lc_timeval { long sec; long usec; };
struct lc_itimerval { struct lc_timeval interval; struct lc_timeval value; };

static long lc_setitimer(int which, const struct lc_itimerval *now, struct lc_itimerval *old)
{
    long r;
#if defined(__x86_64__)
    __asm__ volatile("syscall" : "=a"(r) : "a"(38L), "D"((long)which), "S"(now), "d"(old) : "rcx", "r11", "memory");
#elif defined(__i386__)
    __asm__ volatile("int $0x80" : "=a"(r) : "a"(104L), "b"(which), "c"(now), "d"(old) : "memory");
#else
#error "only x86 Android needs this"
#endif
    return r;
}

__attribute__((visibility("default"))) unsigned int alarm(unsigned int seconds)
{
    struct lc_itimerval now = {{0, 0}, {(long)seconds, 0}};
    struct lc_itimerval old = {{0, 0}, {0, 0}};
    if (lc_setitimer(0 /* ITIMER_REAL */, &now, &old) < 0)
        return 0;
    /* What alarm() returns: whole seconds left, rounded like glibc's own. */
    return (unsigned int)(old.value.sec + (old.value.usec >= 500000));
}
