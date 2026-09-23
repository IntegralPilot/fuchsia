// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

.function write_to_stack, global
  li a0, 0xaaaabbbbccccdddd
  add sp, sp, -16
  sd a0, 8(sp)
  ecall
.end_function
