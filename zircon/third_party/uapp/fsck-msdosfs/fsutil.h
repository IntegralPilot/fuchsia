/* Copyright 2026 The Fuchsia Authors. All rights reserved.
 * Use of this source code is governed by a BSD-style license that can be
 * found in the LICENSE file.
 */

/*
 * Fuchsia Portability Shim: "fsutil.h"
 * Provides print macros (perr, pwarn, pfatal) and device helper shims
 * expected by FreeBSD fsck_msdosfs source files.
 */

#ifndef ZIRCON_THIRD_PARTY_UAPP_FSCK_MSDOSFS_FSUTIL_H_
#define ZIRCON_THIRD_PARTY_UAPP_FSCK_MSDOSFS_FSUTIL_H_

#include <stdio.h>

#define perr(...) printf(__VA_ARGS__)
#define pfatal(...) printf(__VA_ARGS__)
#define pwarn(...) printf(__VA_ARGS__)
#define setcdevname(a, b) do {} while(0)

#endif  // ZIRCON_THIRD_PARTY_UAPP_FSCK_MSDOSFS_FSUTIL_H_
