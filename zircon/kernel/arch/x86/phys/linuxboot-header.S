// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/asm.h>

#include "phys32.h"
#include "linuxboot-asm.h"

// The phys.ld linker script places this first in the image.
// The linuxboot.ld linker script arranges that this means it
// it is "before" the actual fixed load address of the bzImage
// 32-bit code (which includes the start32.S entry point).
.section .boot.header, "awx", %progbits

// This is the putative boot_params object at the start of the image.
// See linuxboot.h for full details about the protocol.  In fact, this
// just corresponds to the boot_params layout for purposes of locating
// the boot_params::setup_header, below.
.org 0
.label zero_page

// This is not used by the Linux/x86 boot protocol.  Historically,
// this format could be used directly as a Master Boot Record image.
// This ancient protocol consists of simply loading the first sector
// into a well-known location in memory and jumping to it in 16-bit
// real mode.  This is not supported by Linux kernels any more, but a
// legacy stub is provided to panic nicely if loaded in the original
// x86 PC way by firmware supporting the old 16-bit x86 BIOS ABI.
.code16
.function MbrEntry, nosection=nosection
  // This is the traditional physical load address.  Make sure
  // it's the exact start of the %cs segment.
  ljmp $0x07c0, $(0f - MbrEntry)
0:

  // Reset segment registers to be safe and don't trust any stack pointer.
  mov %cs, %ax
  mov %ax, %ds
  mov %ax, %es
  mov %ax, %ss
  mov %ax, %ss
  xor %sp, %sp

  // Normalize flags that might affect the BIOS or instructions used here.
  sti
  cld

  // cf https://en.wikipedia.org/wiki/BIOS_interrupt_call
  mov $(MbrEntryMessage - MbrEntry), %si
  mov $MbrEntryMessageSize, %cx
.Lbios_write_char:
  lodsb         // %ax = *%si++
  mov $0xe, %ah // Write character in TTY mode.
  mov $0xd, %bx // Color (light magenta).
  int $0x10     // https://en.wikipedia.org/wiki/INT_10H
  loop .Lbios_write_char

.Lbios_wait_for_key:
  xor %ax, %ax
  int $0x16

.Lbios_reboot:
  int $0x19             // Ask the BIOS to reboot.  Should not return.
  ljmp $0xf000, $0xfff0 // If it does, jump directly to the BIOS entry point.
.end_function

.object MbrEntryMessage, nosection=nosection
  .ascii "Legacy x86 MBR booting is not supported.\r\n"
  .ascii "Install a boot loader that supports Linux/x86 bzImage format.\r\n"
  .ascii "Hit any key to reboot...\r\n"
  MbrEntryMessageSize = . - MbrEntryMessage
.end_object

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
  .int LINUXBOOT_SYSSIZE

  // This is a required magic number the boot loader checks for.
  .org BOOT_PARAMS_HDR + SETUP_HEADER_BOOT_FLAG
  .short LINUXBOOT_BOOT_FLAG

  // This is offset 512, where the 16-bit entry point is.  Since there's a
  // required header field right after, this must be a two-byte instruction.
  // So we make it a jump to past the end of the fixed header layout, where
  // we define the real-mode 16-bit entry path.
  .org BOOT_PARAMS_HDR + SETUP_HEADER_JUMP
  // This must be a 1-byte displacement to make a 2-byte instruction that
  // fits before the next field.  That makes a maximum jump distance of 127,
  // to 0x202 + 127 = 0x281.
  jmp Phys16Entry

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
  .org BOOT_PARAMS_HDR + SETUP_HEADER_LOADFLAGS
  .byte LOADFLAGS_LOADED_HIGH

  // This gives the boot loader license to place the initrd RAMDISK image
  // anywhere in the 4G address space.  (This will probably be ignored by a
  // boot loader using the 64-bit boot protocol, where it can be above 4GiB.)
  .org BOOT_PARAMS_HDR + SETUP_HEADER_INITRD_ADDR_MAX
  .int 0xffffffff

  // Preferred alignment for the load address is just 4KiB.  Without
  // relocatable_kernel set, this shouldn't matter either way for the 32-bit
  // (or 16-bit) boot path--the load address is fixed.  But a boot loader
  // taking the 64-bit boot path instead might use it.
  .org BOOT_PARAMS_HDR + SETUP_HEADER_KERNEL_ALIGNMENT
  .int 0x1000

  // Load addresses must be aligned to 4KiB (1 << 12).
  // As above, only relevant if using the 64-bit boot protocol.
  .org BOOT_PARAMS_HDR + SETUP_HEADER_MIN_ALIGNMENT
  .byte 12

  // The value (XLF_KERNEL_64 | XLF_CAN_BE_LOADED_ABOVE_4G), here indicates a
  // 64-bit kernel that can both use the 64-bit protocol and accept all
  // addresses of interest being past the 4GiB barrier (that's how this is set
  // in linuxboot64-header.S).
  //
  // In a pure phys32 image, LINUXBOOT_XLOADFLAGS is undefined and so filled as
  // zero: no flags set in xloadflags, so the image can be loaded using the
  // 32-bit protocol or the 16-bit protocol, both of which are implemented here
  // in this file.
  //
  // In a bi-arch image, LINUXBOOT_XLOADFLAGS will be defined to set the
  // xloadflags value above.  Linux64Entry will be defined to point directly to
  // _start (in start.S), preempting the weak definition below.
  .weak LINUXBOOT_XLOADFLAGS
  .org BOOT_PARAMS_HDR + SETUP_HEADER_XLOADFLAGS
  .short LINUXBOOT_XLOADFLAGS

   // This constrains the maximum size of kernel command line data passed.
  .org BOOT_PARAMS_HDR + SETUP_HEADER_CMDLINE_SIZE
  .int 0xffffffff

  // This tells the boot loader how much space to leave from the start of the
  // kernel's load image.  This ensures it leaves space for the .bss and not
  // just the image it copies in.
  .org BOOT_PARAMS_HDR + SETUP_HEADER_INIT_SIZE
  .int LINUXBOOT_INIT_SIZE

   // The rest of setup_header contains fields a boot loader might examine
   // or modify though they're not meaningful to the version of the protocol
   // we support if they have zero-initialized values.
  .org BOOT_PARAMS_HDR + SIZEOF_SETUP_HEADER
.end_object

// This is close to the farthest the two-byte jump above can reach.
// The boot loader enters at offset 512, which is that short jump here.
.function Phys16Entry, nosection=nosection
  // Normalize flags that might affect the BIOS or instructions used here.
  sti
  cld

  // To be safe, only trust %ds to have been set by the boot loader.
  mov %ds, %ax
  mov %ax, %es

  // Copy the setup_header, as modified by the boot loader, into the
  // boot_params struct to be provided to the 32-bit entry path.
  mov $(boot_params + BOOT_PARAMS_HDR - zero_page), %di
  mov $(setup_header - zero_page), %si
  mov $SIZEOF_SETUP_HEADER, %cx
  rep movsb

  // boot_params::e820_table needs to be populated using the legacy BIOS ABI.
  xor %ebx, %ebx    // Cleared for first BIOS call.
  movb %bl, (boot_params + BOOT_PARAMS_E820_ENTRIES - zero_page)
.Le820_loop:
  // If you've been wondering where the name came from...
  mov $0xe820, %eax

  // %ebx is zero for the first iteration and is updated by the BIOS
  // to new, unspecified, nonzero values with each iteration.

  // This gives the buffer size: we read only one entry at a time.
  mov $SIZEOF_E820ENTRY, %ecx

  // This is a second magic number that must be set exactly.
  mov $E820_MAGIC, %edx

  // Use a single temporary buffer so as not to trust the BIOS to handle
  // an address advancing through the table properly.
  mov $(gE820Buffer - zero_page), %di

  // Call the BIOS.  The carry flag indicates the call failed.
  // Otherwise a few registers have results.
  int $0x15
  jc .Le820_end

  // Ignore the whole table if the BIOS is not following the protocol.
  cmp $E820_MAGIC, %eax
  je 0f
  movb $0, (boot_params + BOOT_PARAMS_E820_ENTRIES - zero_page)
  jmp .Le820_end
0:

  // Copy the entry.
  mov $SIZEOF_E820ENTRY, %cx
  mov $(gE820Buffer - zero_page), %si
  mov $(boot_params + BOOT_PARAMS_E820_TABLE - zero_page), %di
  movb (boot_params + BOOT_PARAMS_E820_ENTRIES - zero_page), %al
  mov %al, %dl
  mul %cl
  add %ax, %di
  rep movsb

  // Update the boot_params::e820_entries counter.
  inc %dl
  movb %dl, (boot_params + BOOT_PARAMS_E820_ENTRIES - zero_page)

  // Bail out if the table is full.
  cmpb $MAX_E820_TABLE_ENTRIES, %dl
  jle .Le820_end

  // The BIOS returns zero in %ebx when there are no more entries.
  test %ebx, %ebx
  jnz .Le820_loop
.Le820_end:

  // Make sure interrupts are disabled just in case.
  cli

  // Materialize the linear address of the code below, which will be the
  // new PC in the flat 32-bit code segment.
  mov %ds, %eax
  shl $4, %eax
  add $(Protected32Entry - zero_page), %eax

  // Push the long return address that will be popped below.
  pushl $PHYS32_CODE32_SEL
  .cfi_adjust_cfa_offset 4
  pushl %eax
  .cfi_adjust_cfa_offset 4

  // Now point %esi at the boot_params being filled out here.
  // This is the argument register for the 32-bit entry path.
  mov %ds, %esi
  shl $4, %esi
  add $(boot_params - zero_page), %esi

  // Linux booting protocol requires these all be cleared.  It doesn't really
  // matter to our code, but it makes it consistent with what a boot loader
  // doing direct 32-bit entry will do.
  xor %ebx, %ebx
  xor %ecx, %ecx
  xor %edx, %edx
  xor %ebp, %ebp
  xor %edi, %edi

  // Build a temporary descriptor pointing at the GDT to load it.
  // We reuse the gdt32.cc setup that start32.S will load again later.
  // But we need it loaded first so we can enable 32-bit protected mode.
  movl $gPhys32Gdt, -4(%esp)
  movw $(PHYS32_GDT_SIZE - 1), -6(%esp)
  lgdtl -6(%esp)

  // Enable protected mode.  The Intel manual recommends doing a far jump
  // into a new %cs segment immediately after setting the PE bit.
  mov %cr0, %eax
  orb $X86_CR0_PE, %al
  mov %eax, %cr0

  // Immediately return into the new 32-bit code segment.
  lretl
.end_function

// Execution resumes here, now in 32-bit protected mode.
// This code still has to be purely position-independent.
.code32
.function Protected32Entry, align=16, nosection=nosection
  // Reload all the segment registers (except %cs).  The Intel manual
  // recommends doing this immediately after jumping into the new %cs segment.
  mov $PHYS32_DATA32_SEL, %ax // Flat data segment for %ds, %es, and %ss.
  mov %ax, %ds
  mov %ax, %es
  mov %ax, %ss
  mov %bx, %fs // Null segment selector for %fs and %gs.
  mov %bx, %gs

  // Now jump to the linux entry point.
  mov $Linux32Entry, %eax
  jmp *%eax
.end_function

// When the 16-bit entry path is used, it passes this to the 32-bit path after
// copying setup_header, as previously modified by the boot loader, into it.
// The 16-bit setup code here is the only source of information other than what
// the boot loader wrote into setup_header.  When the boot loader uses the
// 32-bit entry path, it passes its own pointer to a boot_params struct it has
// allocated elsewhere and filled with appropriate data, including copying our
// setup_header into that struct.  In that case this space is not used at all.
.balign 16, 0
.object boot_params, nosection=nosection
  .space SIZEOF_BOOT_PARAMS, 0
.end_object

.object gE820Buffer, align=4, nosection=nosection
  .space SIZEOF_E820ENTRY, 0
.end_object

// Pad out to a whole number of 512-byte sectors and record that total size (in
// bytes) as the value of the LINUXBOOT_SETUP_SIZE symbol (a SHN_ABS symbol
// that's a size, not an address).  The setup_header::setup_sects field
// initializer is calculated from this value above.  The linuxboot.ld linker
// script also uses this symbol to compute the LINUXBOOT_SYSSIZE symbol that's
// needed for the setup_header::syssize field initializer above.
.balign 512, 0
.label LINUXBOOT_SETUP_SIZE, global, notype, . - zero_page

// This matches the fixed LINUXBOOT_LOAD_ADDRESS, but the assembler doesn't
// know what absolute addresses will be, so it needs to refer to it only as a
// label defined in this file.
.label LinuxBase

// This is the entrypoint for bootloaders using the 32-bit Linux boot protocol.
// The data earlier in the image is not actually part of what's loaded into
// memory.  Instead, the kernel's memory image starts here.  For the 32-bit
// non-relocatable protocol, that's exactly at 1MiB.  For the relocatable
// protocol, it's at some aligned address.  In the 64-bit boot protocol, it's
// anywhere in the address space, not necessarily in the low 4GiB, and the boot
// loader jumps to 0x200 past here in 64-bit mode.
.function Linux32Entry, nosection=nosection
  // Jump to the start32.S entry point at its fixed address.
  mov $Phys32Entry, %eax
  jmp *%eax
.end_function

# Add the in-kernel config file, which is gzipped.
# Some emulators look for this file using extract-ikconfig before booting.
.ascii "IKCFG_ST"
.incbin IKCONFIG_FILE
.ascii "IKCFG_ED"

// The 64-bit boot protocol always jumps here.  Just trampoline to where the
// entry point for 64-bit mode is defined elsewhere.
.org Linux32Entry + 0x200
.code64
.function Linux64Trampoline, nosection=nosection
  jmp Linux64Entry
.end_function

// If no other definition overrides it, a 64-bit entry gets here to crash.
// (Note this is defined in its own section so it can be GC'd if overridden.)
.function Linux64Entry, weak
0:ud2
  jmp 0b
.end_function
