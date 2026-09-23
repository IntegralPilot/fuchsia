// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "asm.h"

.section .text.debug_break, "ax", @progbits
.hidden debug_break
ENTRY(debug_break)
  // Use explicit c.ebreak to ensure it's a known 2-byte instruction.
  c.ebreak
  ret
END(debug_break)
