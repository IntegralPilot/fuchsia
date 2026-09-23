/* Copyright 2023 The Fuchsia Authors. All rights reserved.
 * Use of this source code is governed by a BSD-style license that can be
 * found in the LICENSE file.
 */

/*
 * This sets the entry point in the ELF file and avoids a warning about not
 * setting an explicit entry point, but it has no effect.  The assertion below
 * ensures that the entry point is always at the beginning of the blob.
 */
ENTRY(HERMETIC_ENTRY)

/*
 * This is a linker script to produce a single hermetic leaf function binary blob.
 * It's used with --oformat binary to directly produce the raw blob to be patched in.
 * It requires the global symbol HERMETIC_ENTRY be set to the function's entry point,
 * and requires that this entry point be the first instruction in the code.
 * No non-code sections are allowed unless HERMETIC_RODATA_OK is defined.
 * No allocated sections other than .text* and .rodata* are ever allowed.
 */
SECTIONS {
  . = 0;

  /*
   * The .text* sections become the actual binary blob.
   * The entry point must be first, so it can be put into .text.entry.
   */
  .text : { *(.text.entry) *(.text*) }
  ASSERT(HERMETIC_ENTRY == 0,
         "hermetic code blob requires entry point at beginning of code")

  .rodata : { *(.rodata*) }
  ASSERT(DEFINED(HERMETIC_RODATA_OK) || SIZEOF(.rodata) == 0,
        "RODATA not allowed in this hermetic code blob")

  /*
   * These clauses detect any relocation records that haven't been applied by the hermetic link.
   * No references outside the hermetic blob are allowed.
   */
  .relr.auth.dyn : { *(.relr.auth.dyn) }
  .relr : { *(.relr.*) }
  .rela : { *(.rela.*) }
  .rel : { *(.rel.*) }
  ASSERT(SIZEOF(.relr.auth.dyn) + SIZEOF(.relr) + SIZEOF(.rela) + SIZEOF(.rel) == 0,
         "hermetic code blob cannot use dynamic relocations")

  /*
   * There's no way to make use of DWARF CFI for an hermetic code blob.
   */
  /DISCARD/ : { *(.eh_frame*) }

  /*
   * This should detect any section not called .text*, such as .data et al.
   * These are not allowed.
   */
  .other : { INPUT_SECTION_FLAGS(SHF_ALLOC) *(*) }
  ASSERT(SIZEOF(.other) == 0,
         "unexpected SHF_ALLOC sections not .text* or .rodata*")
}
