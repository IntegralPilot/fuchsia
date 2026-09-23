// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

.function _start, global, retain=R
  .prologue.fp 128
  .epilogue.fp 128
  ret
.end_function

.function second, global, retain=R
  .prologue.fp 256
  // This makes it different from the first one on x86 so they don't get ICF'd
  // The functions are identical on x86, though they have different metadata.
  nop
  .epilogue.fp 256
  ret
.end_function
