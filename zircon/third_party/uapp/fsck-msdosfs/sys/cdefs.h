/* Copyright 2026 The Fuchsia Authors. All rights reserved.
 * Use of this source code is governed by a BSD-style license that can be
 * found in the LICENSE file.
 */

/*
 * Fuchsia Portability Shim: <sys/cdefs.h>
 * Provides BSD-specific type definitions (u_int, u_int64_t, etc.) and
 * compiler macros (__RCSID, __FBSDID, __dead2, etc.) required by upstream
 * FreeBSD fsck_msdosfs source files when compiling under Fuchsia's toolchain.
 */

#ifndef ZIRCON_THIRD_PARTY_UAPP_FSCK_MSDOSFS_SYS_CDEFS_H_
#define ZIRCON_THIRD_PARTY_UAPP_FSCK_MSDOSFS_SYS_CDEFS_H_

#include <stdbool.h>
#include <stdint.h>
#include <sys/types.h>

typedef uint8_t u_int8_t;
typedef uint16_t u_int16_t;
typedef uint32_t u_int32_t;
typedef uint64_t u_int64_t;
typedef unsigned int u_int;
typedef unsigned char u_char;
typedef unsigned short u_short;

#ifndef __RCSID
#define __RCSID(x) struct __rcsid_dummy
#endif

#ifndef __FBSDID
#define __FBSDID(x) struct __fbsdid_dummy
#endif

#ifndef __dead2
#define __dead2 __attribute__((noreturn))
#endif

#ifndef __nonstring
#define __nonstring __attribute__((nonstring))
#endif

#ifndef __printflike
#define __printflike(a, b) __attribute__((format(printf, a, b)))
#endif

#ifndef powerof2
#define powerof2(x) ((((x) - 1) & (x)) == 0)
#endif

#ifndef roundup2
#define roundup2(x, y) (((x) + ((y) - 1)) & ~((y) - 1))
#endif

#ifndef LONG_BIT
#define LONG_BIT (sizeof(long) * 8)
#endif

#endif  // ZIRCON_THIRD_PARTY_UAPP_FSCK_MSDOSFS_SYS_CDEFS_H_
