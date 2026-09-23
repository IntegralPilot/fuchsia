// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/asm.h>

#include "linuxboot-asm.h"

// This is linked into the 32-bit phys executable that uses linuxboot-header.S
// to make it a bi-arch Linux/x86 boot protocol phys executable, actually it's
// tri-arch, but only sort of.  That is, the 32-bit phys executable supports
// the Linux/x86 16-bit boot protocol and the Linux/x86 32-bit boot protocol,
// but both landing in 32-bit mode and executing the phys32 build of the code.
// It runs all the phys C++ code in true 32-bit mode (up until it tries to set
// up page tables, anyway).  So if booted on an actual (or virtual) 32-bit CPU,
// it can get far enough to print things and complain at least.

// With this file linked in, linuxboot-header.S uses the value below for the
// setup_header::xloadflags field, telling a boot loader that this kernel also
// compatible with the Linux/x86 64-bit boot protocol.
.label LINUXBOOT_XLOADFLAGS, global, notype, \
    XLF_KERNEL_64 | XLF_CAN_BE_LOADED_ABOVE_4G

// When the boot loader chooses the 64-bit protocol instead of the 16-bit or
// 32-bit protocol, it will never enter the phys32 code in the "outer" phys
// executable this file is linked into.  Instead, it will enter at the
// Linux64Trampoline function in linuxboot-header.S in 64-bit mode (and
// therefore, by necessity, with paging enabled).  That jumps to Linux64Entry,
// defined below (overriding a weak stub definition in linuxboot-header.S).

// The protocol from the boot loader at that point is very similar to the x86
// ZBI boot protocol (if not more generous) as to segmentation and limited
// identity-mapping page table setup guarantees, so the 64-bit phys code is
// right at home.  So all the 32-bit code is completely ignored and instead a
// separate 64-bit phys executable takes over.  That is, this apparently 32-bit
// binary (with tiny assembly-only 16-bit support from linuxboot-header.S) is
// actually a "fat" binary of a sort, with completely separate 64-bit binary
// embedded within.
//
// The 64-bit binary is linked separately using raw-header.S rather than
// linuxboot64-header.S, so instead of any kind of header the image starts
// immediately with a jump instruction the generic phys start.S entry point
// _start.  That code already expects the bootloader data pointer in %rsi,
// where the Linux/x86 boot protocol passes boot_params.

// This is placed as an RODATA object because it's not really code from the
// perspective of the "outer" 32-bit phys executable.  The whole image is in
// fact mapped as read-write-executable when entered in 64-bit mode where that
// matters.  As soon as we get here, we're not really in the outer 32-bit phys
// executable any more, it's just what happens to be in the memory around the
// image of the inner 64-bit phys executable.  The Linux/x86 64-bit boot
// protocol will align the whole image to 4KiB, here that being the whole outer
// 32-bit phys executable (without the header parts of linuxboot-header.S).
// The link-time LINUXBOOT_LOAD_ADDRESS is also aligned to 4KiB, so aligning
// here too ensures that the whole 64-bit image gets aligned as it expects.
.object Linux64Entry, rodata, global, align=0x1000
#ifndef LINUXBIBOOT64_FILENAME
#error "build system should define LINUXBIBOOT64_FILENAME
#endif
  .incbin LINUXBIBOOT64_FILENAME
.end_object
