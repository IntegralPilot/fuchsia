// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/asm.h>

#include "linuxboot-asm.h"

// The phys.ld linker script places this first in the image.  The linuxboot.ld
// linker script arranges that this means it it is "before" the actual physical
// load address of the bzImage 32-bit code.  That code is skipped over by the
// 64-bit boot path to reach the 64-bit entry point.
.section .boot.header, "awx", %progbits

// This is the putative boot_params object at the start of the image.
// See linuxboot.h for full details about the protocol.  In fact, this
// just corresponds to the boot_params layout for purposes of locating
// the boot_params::setup_header, below.
.org 0
.label zero_page

// This is where ancient MBR code went, so in the abstract, from here on
// starts the 16-bit portion of the code (linuxboot-header.S has a real one).
.code16

// The space before boot_params::hdr is not examined by the boot loader.  A
// boot loader using the 16-bit entry protocol reads hdr.setup_sects and then
// loads this whole file's contents into memory at zero_page before entering
// at setup_header::jump, below.
.org BOOT_PARAMS_HDR

.object setup_header, nosection=nosection
  // This tells the boot loader how many 512-byte sectors after this first
  // one to load.  LINUXBOOT_SETUP_SIZE is defined at the end of the file.
  .org BOOT_PARAMS_HDR + SETUP_HEADER_SETUP_SECTS
  .byte ((LINUXBOOT_SETUP_SIZE >> 9) - 1)

  // This tells the boot loader how many 16-byte units (Intelspeak paragraphs)
  // to load at the fixed load address for the 32-bit kernel image.  The
  // linuxboot.ld linker script calculates the value of LINUXBOOT_SYSSIZE.
  .org BOOT_PARAMS_HDR + SETUP_HEADER_SYSSIZE
  // In the 32-bit case, it's a fixed-position link and so linuxboot-header.S
  // just does `.int LINUXBOOT_SYSSIZE` here and that's fine.  LLD in fact
  // allows that for the 64-bit position-independent case too, but GNU (BFD) ld
  // does not.  It will only allow a PC-relative relocation.  So linuxboot.ld
  // also computes LINUXBOOT_SYSSIZE_RELATIVE as the difference between the
  // LINUXBOOT_SYSSIZE value and the link-time load address (PHYS_LOAD_ADDRESS,
  // aka the zero_page label above); it's not a meaningful address since it's
  // scaled by 16, but it's the right addend for a PC-relative relocation to
  // resolve to exactly LINUXBOOT_SYSSIZE.
  .int LINUXBOOT_SYSSIZE_RELATIVE - zero_page

  // This is a required magic number the boot loader checks for.
  .org BOOT_PARAMS_HDR + SETUP_HEADER_BOOT_FLAG
  .short LINUXBOOT_BOOT_FLAG

  // This is offset 512, where the 16-bit entry point is.  Since there's a
  // required header field right after, this must be a two-byte instruction.
  // This doesn't get executed as a jump in either the 32-bit or the 64-bit
  // protocol, but it still must be what it would have been.  That is, it must
  // be a jump to the end of the setup header, where the 16-bit code would be
  // (linuxboot-header.S has real 16-bit code there).  This is not going to be
  // used as a jump, but boot loaders know that the second byte after the JMP
  // opcode is an offset that also serves to say how big the setup header is.
  // So this is necessary just to ensure that the boot loader considers the
  // whole setup_header and doesn't ignore the later fields.
  .org BOOT_PARAMS_HDR + SETUP_HEADER_JUMP
  jmp setup_header + SIZEOF_SETUP_HEADER

  // This is a required magic number the boot loader checks for.
  .org BOOT_PARAMS_HDR + SETUP_HEADER_HEADER
  .ascii "HdrS"

  // This tells the boot loader which precise version of the protocol for
  // encoding bits in these headers this kernel image is compatible with.
  // Version 2.12 is not the newest, but none of the Zircon shim code needs
  // to take advantange of any of the newer features.
  .org BOOT_PARAMS_HDR + SETUP_HEADER_VERSION
  .short 0x020c

  // This indicates "bzImage" format: load the 32-bit code at 1MiB.
  // This is probably ignored when using the 64-bit protocol enabled
  // in xloadflags, below.  But it's the usual baseline, so set it.
  .org BOOT_PARAMS_HDR + SETUP_HEADER_LOADFLAGS
  .byte LOADFLAGS_LOADED_HIGH

  // Preferred alignment for the load address is just 4KiB.
  .org BOOT_PARAMS_HDR + SETUP_HEADER_KERNEL_ALIGNMENT
  .int 0x1000

  // Any load address is fine.  This is probably ignored when using the 64-bit
  // protocol, which stipulates it can always be loaded anywhere.  But it
  // doesn't hurt to set it.  There is no 32-bit entry path here, so it doesn't
  // matter that the actual 32-bit build doesn't support relocatability.
  .org BOOT_PARAMS_HDR + SETUP_HEADER_RELOCATABLE_KERNEL
  .byte 1

  // Load addresses must be aligned to 4KiB (1 << 12).
  .org BOOT_PARAMS_HDR + SETUP_HEADER_MIN_ALIGNMENT
  .byte 12

  // This indicates a 64-bit kernel that can use the 64-bit protocol.
  // This version of the header code supports only the 64-bit protocol.
  .org BOOT_PARAMS_HDR + SETUP_HEADER_XLOADFLAGS
  .short XLF_KERNEL_64 | XLF_CAN_BE_LOADED_ABOVE_4G

   // This constrains the maximum size of kernel command line data passed.
  .org BOOT_PARAMS_HDR + SETUP_HEADER_CMDLINE_SIZE
  .int 0xffffffff

  // This tells the boot loader how much space to leave from the start of the
  // kernel's load image.  This ensures it leaves space for the .bss and not
  // just the image it copies in.
  .org BOOT_PARAMS_HDR + SETUP_HEADER_INIT_SIZE
  .int _end - LINUXBOOT_LOAD_ADDRESS

   // The rest of setup_header contains fields a boot loader might examine
   // or modify though they're not meaningful to the version of the protocol
   // we support if they have zero-initialized values.
  .org BOOT_PARAMS_HDR + SIZEOF_SETUP_HEADER
.end_object

// Pad out to a whole number of 512-byte sectors and record that total size (in
// bytes) as the value of the LINUXBOOT_SETUP_SIZE symbol (a SHN_ABS symbol
// that's a size, not an address).  The setup_header::setup_sects field
// initializer is calculated from this value above.  The linuxboot.ld linker
// script also uses this symbol to compute the LINUXBOOT_SYSSIZE symbol that's
// needed for the setup_header::syssize field initializer above.
//
// This also becomes the base address at which the kernel image is loaded.  At
// runtime it will be page-aligned, but can be at any address.  The link-time
// base address (aka PHYS_LOAD_ADDRESS) is zero, but that has to leave space
// for the zero_page (including setup_header) that is never actually part of
// the memory image, only part of the bzImage binary file format.  To make the
// usual assumptions about link-time vs runtime alignment hold true for
// simplicity's sake, we align this to 4KiB in the file too.  It might waste a
// few KiB in the image size and probably things would work fine without it,
// but it makes the address arithmetic easier to understand when debugging.
.p2align 12, 0
.label LINUXBOOT_SETUP_SIZE, global, notype, . - zero_page
.label LINUXBOOT_LOAD_ADDRESS, global

// It's also the entrypoint for bootloaders using the 32-bit boot protocol.
// That's not supported.  This entry code can only be used with boot loaders
// that use the 64-bit Linux boot protocol, so just crash here.
.code32
.function Linux32Entry, nosection=nosection
  ud2
.end_function

// Add the in-kernel config file, which is gzipped.  Some emulators look for
// this file using extract-ikconfig before booting.  The compressed file (and
// even its uncompressed input) is very tiny, so it fits before the fixed
// offset of the entry point code below.
.ascii "IKCFG_ST"
.incbin IKCONFIG_FILE
.ascii "IKCFG_ED"

// The 64-bit boot protocol always jumps here.
.org LINUXBOOT_LOAD_ADDRESS + ENTRY64_OFFSET

// Just jump to the generic start.S code.  The boot_params pointer is in %rsi,
// which is where that code already expects the boot loader's data pointer.
.code64
.function Linux64Entry, nosection=nosection
  jmp _start
.end_function
