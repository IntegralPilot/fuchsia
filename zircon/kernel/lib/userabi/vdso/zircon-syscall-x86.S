// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

.macro zircon_syscall num, name, caller
  mov $\num, %eax
  syscall

  // This symbol at the return address identifies this as an approved call
  // site.
  .label CODE_SYSRET_\name\()_VIA_\caller, global

  // The `nop` exists due to the difference in instruction size between
  // `syscall` (2 bytes) and `vmcall` (3 bytes). The `nop` padding allows us to
  // patch the vDSO code to use `vmcall` instead of `syscall`.
  nop
.endm
