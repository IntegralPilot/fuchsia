// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

.function write_to_stack, global
  movabs $0xaaaabbbbccccdddd, %r8
  push %r8
  syscall
.end_function
