/* Copyright 2023 The Fuchsia Authors
 *
 * Use of this source code is governed by a MIT-style
 * license that can be found in the LICENSE file or at
 * https://opensource.org/licenses/MIT
 */

/*
 * This is a very minimal linker script meant to induce BFD ld to produce
 * the layout with page-aligned segments and RODATA before code that LLD
 * produces by default.  It only defines a few key sections and relies on
 * orphans placement to place everything else in the segments created.
 */

ENTRY(_start)  /* This is overridden by -e switches.  */

SECTIONS {
  . = SIZEOF_HEADERS;

  .rela.dyn : { INPUT_SECTION_FLAGS(SHF_ALLOC) *(.rela.*) }
  .rel.dyn : { INPUT_SECTION_FLAGS(SHF_ALLOC) *(.rel.*) }

  .rodata : { *(.rodata*) *(.srodata*) }

  . = ALIGN(CONSTANT(MAXPAGESIZE));

  .text : { *(.stext*) *(.text*) }

   PROVIDE_HIDDEN(_etext = .);

  . = ALIGN(CONSTANT(MAXPAGESIZE));

  /*
   * This has no effect but it's necessary for the RELRO machinery to work.
   * Note that even though it should make no difference, passing the page
   * sizes here makes BFD ld do some wonky layout with a misaligned segment.
   */
  . = DATA_SEGMENT_ALIGN(1, 1);

  .init_array : {
    KEEP(*SORT_BY_INIT_PRIORITY(.init_array.*))
    KEEP(*(.init_array))
  }

  .fini_array : {
    KEEP(*SORT_BY_INIT_PRIORITY(.fini_array.*))
    KEEP(*(.fini_array))
  }

  .data.rel.ro : {  *(.sdata.rel.ro*) *(.data.rel.ro*) }

  .dynamic : { *(.dynamic) }

  .got : { *(.got*) }

  . = DATA_SEGMENT_RELRO_END (0, .);

  .data : { *(.sdata*) *(.data*) }

   PROVIDE_HIDDEN(_edata = .);

   PROVIDE_HIDDEN(__bss_start = .);

  .bss : { *(.sbss*) *(.bss* COMMON) }

   PROVIDE_HIDDEN(_end = .);
  . = DATA_SEGMENT_END (.);

  /DISCARD/ : { *(.note.GNU-stack) *(.gnu_debuglink) }
}
