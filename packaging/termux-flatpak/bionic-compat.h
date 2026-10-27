/* Bionic lacks a few glibc conveniences that ostree and flatpak use. */
#pragma once
#ifndef _GNU_SOURCE
#define _GNU_SOURCE 1
#endif
#include <sched.h>
#include <ctype.h>
#include <endian.h>
#include <sys/endian.h>
#include <string.h>
#include <alloca.h>
#ifndef strdupa
#define strdupa(s) ({ const char *__s = (s); size_t __n = strlen(__s) + 1; char *__d = (char *) alloca(__n); (char *) memcpy(__d, __s, __n); })
#endif
#ifndef strndupa
#define strndupa(s, n) ({ const char *__s = (s); size_t __n = strnlen(__s, (n)); char *__d = (char *) alloca(__n + 1); __d[__n] = 0; (char *) memcpy(__d, __s, __n); })
#endif
#include <unistd.h>
#include <sys/syscall.h>
#if defined(__ANDROID_API__) && __ANDROID_API__ < 28
static inline int syncfs(int fd) { return (int) syscall(__NR_syncfs, fd); }
#endif

/* strverscmp, from musl (MIT). */
static inline int lindroids_strverscmp(const char *l0, const char *r0)
{
	const unsigned char *l = (const void *)l0, *r = (const void *)r0;
	size_t i, dp, j;
	int z = 1;
	for (dp = i = 0; l[i] == r[i]; i++) {
		int c = l[i];
		if (!c) return 0;
		if (!isdigit(c)) dp = i + 1, z = 1;
		else if (c != 0) z = 0;
	}
	if (l[dp] - 1 < 9U && r[dp] - 1 < 9U) {
		for (j = i; isdigit(l[j]); j++)
			if (!isdigit(r[j])) return 1;
		if (isdigit(r[j])) return -1;
	} else if (z && dp < i && (isdigit(l[i]) || isdigit(r[i]))) {
		return (unsigned char)(l[i] - 0) - (unsigned char)(r[i] - 0);
	}
	return l[i] - r[i];
}
#define strverscmp lindroids_strverscmp
#if defined(__ANDROID_API__) && __ANDROID_API__ < 34
static inline void lindroids_explicit_bzero(void *p, size_t n) { memset(p, 0, n); __asm__ __volatile__("" : : "r"(p) : "memory"); }
#define explicit_bzero lindroids_explicit_bzero
#endif
/* qsort_r (glibc argument order), through GLib, which every user of it here links. */
#define qsort_r(base, n, size, compar, arg) g_sort_array((base), (n), (size), (GCompareDataFunc)(compar), (arg))
