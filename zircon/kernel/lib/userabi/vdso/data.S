// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

// This is in assembly so that the LTO compiler cannot see the
// initializer values and decide it's OK to optimize away references.

.object DATA_TIME_VALUES, rodata, global, align=VDSO_MAX_PAGE_SIZE
  .fill VDSO_MAX_PAGE_SIZE / 4, 4, 0xdeadbeef
.end_object

.object DATA_CONSTANTS, rodata, global, align=VDSO_MAX_PAGE_SIZE
  .fill VDSO_MAX_PAGE_SIZE / 4, 4, 0xdeadbeef
.end_object
