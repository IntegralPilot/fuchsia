/* Copyright 2021 The Fuchsia Authors
 *
 * Use of this source code is governed by a MIT-style
 * license that can be found in the LICENSE file or at
 * https://opensource.org/licenses/MIT
 */

/*
 * See linuxboot.ld for more details.  This file only sets the symbols
 * that differ between the 32-bit and pure 64-bit versions.
 */

/*
 * We use only "bzImage" protocol, which loads the 32-bit code at fixed 1MiB.
 */
HIDDEN(LINUXBOOT_LOAD_ADDRESS = 0x100000);

/*
 * Size of the kernel in memory, including the bss after the image so that the
 * boot loader is sure to place the kernel where it has enough headroom after
 * the storage image is loaded.  This symbol is used by the linuxboot-header.S
 * assembly code to embed this value, which cannot be calculated directly by a
 * single relocation record.
 */
HIDDEN(LINUXBOOT32_INIT_SIZE = _end - LINUXBOOT_LOAD_ADDRESS);

/*
 * In a pure phys32 image, this will be used.  In a bi-arch image, it will be
 * defined to the MAX of LINUXBOOT32_INIT_SIZE and the 64-bit image's value.
 */
PROVIDE_HIDDEN(LINUXBOOT_INIT_SIZE = LINUXBOOT32_INIT_SIZE);

/*
 * This _start symbol just sets e_entry in ELF headers (as per phys.ld), which
 * don't exist at runtime.  Set it to match the 32-bit protocol's entry point.
 * There is no global symbol for the 16-bit entry point (exactly 512 bytes into
 * the .boot.header region) because the linuxboot-header.S code is not actually
 * part of the same memory image as the rest of the program.
 */
HIDDEN(_start = Phys32Entry);
