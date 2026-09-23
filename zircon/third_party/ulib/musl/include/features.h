#ifndef SYSROOT_FEATURES_H_
#define SYSROOT_FEATURES_H_

#include "__llvm-libc-common.h"

#if defined(_ALL_SOURCE) && !defined(_GNU_SOURCE)
#define _GNU_SOURCE 1
#endif

#if !defined(_BSD_SOURCE)
#define _BSD_SOURCE 1
#endif

#if !defined(_POSIX_SOURCE) && !defined(_POSIX_C_SOURCE) && !defined(_XOPEN_SOURCE) && \
    !defined(_GNU_SOURCE) && !defined(_BSD_SOURCE) && !defined(__STRICT_ANSI__)
#define _BSD_SOURCE 1
#define _XOPEN_SOURCE 700
#endif

#if __STDC_VERSION__ >= 199901L || defined(__cplusplus)
#define __inline inline
#endif

#define __nothrow_fn __NOEXCEPT  // __llvm-libc-common.h defines this

#endif  // SYSROOT_FEATURES_H_
