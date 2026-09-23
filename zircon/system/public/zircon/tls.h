// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_TLS_H_
#define ZIRCON_TLS_H_

// These constants are part of the C/C++ ABI known to compilers for
// *-fuchsia targets.  These are offsets from the thread pointer.

// This file must be includable in assembly files.

#if defined(__x86_64__) || defined(__i386__)

#define ZX_TLS_STACK_GUARD_OFFSET 0x10
#define ZX_TLS_UNSAFE_SP_OFFSET 0x18

#elif defined(__aarch64__) || defined(__riscv)

#define ZX_TLS_STACK_GUARD_OFFSET (-0x10)
#define ZX_TLS_UNSAFE_SP_OFFSET (-0x8)

#else

#error what architecture?

#endif

#endif  // ZIRCON_TLS_H_
