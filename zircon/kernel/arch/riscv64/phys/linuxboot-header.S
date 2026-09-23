// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/asm.h>

// The phys.ld linker script places this first in the image.
.section .boot.header, "awx", %progbits

// cf https://www.kernel.org/doc/html/latest/riscv/boot-image-header.html

.org 0

// The boot CPU enters here with a1 pointing at the Deviceetree FDT header.
// The a0 register contains the HART ID.
.function LinuxbootEntry, nosection=nosection
  // This is a harmless instruction that also happens to have "MZ" as its
  // first two (little-endian) bytes, which is the EFI (PE-COFF) magic number.
  // Some boot loaders look for this, apparently.
  c.li s4, -13

  // Jump to the start.S code, which passes a1 up to PhysMain.
  // Note this has to be a single `j` instruction, which has limited range.
  // The following non-instructions below are expected at a fixed offset.
  j .Lstart
.end_function

// Linux boot loaders may look for this header.  (A loader that simply slaps
// the kernel image someplace in memory and jumps there will usually work too.)
.object LinuxbootHeader, nosection=nosection
  // The initial two instructions are sort of part of the header.
  .org 8

  // Image load offset.
  .quad 0

  // Size of the kernel in memory.  This includes the bss after the image
  // so that the boot loader is sure to place the kernel where it has
  // enough headroom after the storage image is loaded.  This symbol is
  // defined by the linuxboot.ld linker script.
  .quad LINUXBOOT_SIZE

  // Flags: place anywhere (aligned to 2MiB, but we only presume 4KiB anyway)
  .quad 0x4

  // Deprecated magic number field, may be ignored.
  .org 48
  .ascii "RISCV"

  // An obvious magic number that boot loaders will check for.
  .org 56
  .ascii "RSC\x05"

  // For EFI booting, this is an offset to PE-COFF header stuff.  We don't
  // define all that.  This shim is meant for Linux-specific boot loaders.
  // Booting Zircon on EFI will be done by a proper EFI-based loader/shim.
  .org 60
  .int 0
.end_object

// This is always a short branch away from the entry point above.  It's a
// double-jump so it can use the linker-relaxed pseudo-instruction that doesn't
// presume the jump target is within a certain distance of the jump itself.
.Lstart:
  tail _start
