// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

.syntax unified
.thumb

// Thumb syscall function.
// Sets r7 = 64 and issues svc #0 from Thumb mode.
.function thumb_syscall_bounce, export
  movs r7, #64
  svc #0
.label thumb_syscall_bounce_post_syscall, export
  b .
.end_function
