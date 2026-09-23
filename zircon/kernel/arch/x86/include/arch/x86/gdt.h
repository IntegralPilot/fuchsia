// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_X86_INCLUDE_ARCH_X86_GDT_H_
#define ZIRCON_KERNEL_ARCH_X86_INCLUDE_ARCH_X86_GDT_H_

// Loads the kernel's early boot GDT. This should be done as early as possible:
// ideally done before touching the segment registers as that will cause the
// CPU to verify that the GDTR points to valid memory for the GDT. Eventually,
// the GDT handed off from physboot will be reallocated, but there's no danger
// of that in early kernel bootstrap.
extern "C" void load_startup_gdt();

#endif  // ZIRCON_KERNEL_ARCH_X86_INCLUDE_ARCH_X86_GDT_H_
