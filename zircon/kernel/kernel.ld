/* Copyright 2020 The Fuchsia Authors
 *
 * Use of this source code is governed by a MIT-style
 * license that can be found in the LICENSE file or at
 * https://opensource.org/licenses/MIT
 */

/*
 * Symbols used in the kernel proper are defined with PROVIDE_HIDDEN:
 * HIDDEN because everything in the kernel is STV_HIDDEN to make it
 * clear that direct PC-relative references should be generated in PIC;
 * PROVIDE because their only purpose is to satisfy kernel references.
 */

SECTIONS {
  . = 0;

  /*
   * This symbol is used by code the compiler generates.
   * It serves no particular purpose in the kernel.
   */
  PROVIDE_HIDDEN(__dso_handle = .);

  /*
   * This symbol marks the beginning of the whole image, from here to _end.
   * This name is chosen to match the built-in default in the linkers for
   * this.  (In normal ELF links __ehdr_start also points here, but in the
   * GNU linkers __executable_start is defined in links without in-memory ELF
   * headers, where __ehdr_start is not.)
   */
  PROVIDE_HIDDEN(__executable_start = .);

  /*
   * This symbol marks the beginning of the read-only data (RODATA) segment,
   * from here to __rodata_end.
   *
   * The RODATA segment comes first so it can start with the ELF file and
   * program headers, which are always at the beginning of the file anyway.
   * That means the next available space is not page-aligned, so it's best to
   * stuff in non-aligned bits there to minimize the zero-padding required to
   * align the RODATA sections that need to be page-aligned.
   */
  PROVIDE_HIDDEN(__rodata_start = .);
  . += SIZEOF_HEADERS;

  /*
   * Place this as close as possible to the ELF headers so only a little
   * memory needs to be scanned to identify the ELF image completely.
   */
  .note.gnu.build-id : { *(.note.gnu.build-id) }

  /*
   * This is the metadata physboot uses to load and relocate the PIE kernel.
   * Each one has to be a separate output section because they're specially
   * produced by the linker, or in the case of `.interp` recognized by name
   * by the linker.
   */
  .interp : { *(.interp) }
  .dynamic : ONLY_IF_RO { *(.dynamic) }
  .hash : { *(.hash) }
  .gnu.hash : { *(.gnu.hash) }
  .dynsym : { *(.dynsym) }
  .dynstr : { *(.dynstr) }
  .relr.auth.dyn : { *(.relr.auth.dyn) }
  ASSERT(SIZEOF(.relr.auth.dyn) == 0, ".relr.auth.dyn is not supported")
  .relr.dyn : { *(.relr.dyn) }
  .rela.dyn : { *(.rela.*) }
  .rel.dyn : { *(.rel.*) }

  /*
   * This is the remainder of miscellaneous RODATA.
   */
  .rodata : { *(SORT_BY_ALIGNMENT(.rodata*)) }

  /*
   * The named sections starting with kcountdesc are sorted by name so that
   * tools can provide binary search lookup for counters::Descriptor::name[]
   * variables.  This is page-aligned and padded out to page size so it can be
   * exported as a VMO without exposing any other .rodata contents.
   */
  .kcounter.desc : ALIGN(CONSTANT(MAXPAGESIZE)) {
    PROVIDE_HIDDEN(k_counter_desc_vmo_begin = .);
    *(.kcounter.desc.header)
    ASSERT(. - k_counter_desc_vmo_begin == 16,
           "lib/counters/counters.cc and kernel.ld mismatch");
    QUAD(kcountdesc_end - kcountdesc_begin);
    PROVIDE_HIDDEN(kcountdesc_begin = .);
    ASSERT(kcountdesc_begin - k_counter_desc_vmo_begin == 24,
           "lib/counters/counters.cc and kernel.ld mismatch");
    *(SORT_BY_NAME(kcountdesc.*))
    PROVIDE_HIDDEN(kcountdesc_end = .);
    . = ALIGN(CONSTANT(MAXPAGESIZE));
    PROVIDE_HIDDEN(k_counter_desc_vmo_end = .);
  }

  /*
   * Any read-only data "orphan" sections will be inserted here.  Ideally we'd
   * put those into the .rodata output section, but there isn't a way to do
   * that that guarantees all same-named input sections collect together as a
   * contiguous unit, which is what we need them for.  Linkers differ in how
   * they'll place another empty section here relative to the orphans, so
   * there's no good way to define __rodata_end to be exactly the end of all
   * the orphans sections.  But the only use we have for __rodata_end is to
   * round it up to page size anyway, so just define it inside the code segment
   * below, which is exactly the end of the orphans rounded up to page size.
   */

  /*
   * This symbol marks the beginning of the code (executable) segment,
   * from here to __code_end.
   */
  . = ALIGN(CONSTANT(MAXPAGESIZE));
  PROVIDE_HIDDEN(__rodata_end = .);
  PROVIDE_HIDDEN(__code_start = .);

  /*
   * There should be no executable sections that don't start with `.text`.
   *
   * Sorting by alignment minimizes intersection padding; secondarily sorting
   * by name groups prefixes like `.text.hot.*` or `.text.cold.*` together.
   */
  .text : {
    PROVIDE_HIDDEN(__text_start = .);
    *(SORT_BY_ALIGNMENT(.text*))
  }

  PROVIDE_HIDDEN(__code_end = .);

  /*
   * The DATA_SEGMENT_ALIGN doesn't really do anything special here in terms of
   * the . setting.  But GNU ld requires it and the corresponding uses of
   * DATA_SEGMENT_RELRO_END and DATA_SEGMENT_END to generate PT_GNU_RELRO.
   */
  . = DATA_SEGMENT_ALIGN(CONSTANT(MAXPAGESIZE), CONSTANT(COMMONPAGESIZE));
  . = ALIGN(CONSTANT(MAXPAGESIZE));
  ASSERT(. % CONSTANT(MAXPAGESIZE) == 0, "unexpected misalignment")

  PROVIDE_HIDDEN(__relro_start = .);

  /*
   * When compiling PIC, the compiler puts things into sections it thinks need
   * to be writable until after dynamic relocation.  In the kernel, these
   * things all just form another read-only segment.
   */

  .dynamic : ONLY_IF_RW { *(.dynamic) }

  /*
   * This inherits the SHT_INIT_ARRAY sh_type from the input sections, and
   * that generates DT_INIT_ARRAY for physboot to collect while relocating.
   */
  .init_array : {
    KEEP(*(SORT_BY_INIT_PRIORITY(.init_array.*)
           SORT_BY_INIT_PRIORITY(.ctors.*)))
    KEEP(*(.init_array .ctors))
    ASSERT(ALIGNOF(.init_array) == 8 || SIZEOF(.init_array) == 0,
           ".init_array overalignment -> maybe padding gaps");
  }

  /*
   * Ideally there should be no .got section generated at all.
   * But GNU (BFD) ld for arm64 insists on producing one unnecessarily.
   * KERNEL_GOT_TOLERANCE is set via -defsym in BUILD.gn.
   */
  .got : { *(.got*) }
  ASSERT(SIZEOF(.got) <= KERNEL_GOT_TOLERANCE, "unexpected GOT slots")

  /*
   * LLD only makes particular named sections part of PT_GNU_RELRO, so
   * all the special RELRO sections have to be merged into one output section.
   */
  .data.rel.ro : {
    PROVIDE_HIDDEN(__start_commands = .);
    *(.data.rel.ro.commands)
    PROVIDE_HIDDEN(__stop_commands = .);

    PROVIDE_HIDDEN(__start_lk_init = .);
    *(.data.rel.ro.lk_init)
    PROVIDE_HIDDEN(__stop_lk_init = .);

    PROVIDE_HIDDEN(__start_unittest_testcases = .);
    *(.data.rel.ro.unittest_testcases)
    PROVIDE_HIDDEN(__stop_unittest_testcases = .);

    PROVIDE_HIDDEN(__start_asan_globals = .);
    KEEP(*(asan_globals))
    PROVIDE_HIDDEN(__stop_asan_globals = .);

    /* TODO(https://fxbug.dev/384994543): Alignment should actually be 8 if present. */
    *(.data.rel.ro*)
    ASSERT(ALIGNOF(.data.rel.ro) <= 32 || SIZEOF(.data.rel.ro) == 0,
           ".data.rel.ro overalignment -> padding gaps");

    /*
     * Ensures that the relro region and its corresponding load segment will
     * both have page-aligned sizes.
     */
    . = ALIGN(CONSTANT(MAXPAGESIZE));
    PROVIDE_HIDDEN(__relro_end = .);
  }

  /*
   * This is a no-op for . but GNU ld requires it to generate PT_GNU_RELRO.
   */
   . = DATA_SEGMENT_RELRO_END(0, .);

  .data : ALIGN(CONSTANT(MAXPAGESIZE)) {
    PROVIDE_HIDDEN(__data_start = .);

    /* Pull out any aligned data into a separate section to make sure
     * individual variables do not alias with any unaligned vars.
     */
    *(.data.cpu_align_exclusive)
    . = ALIGN(128);

    PROVIDE_HIDDEN(__start_rust_lock_classes = .);
    KEEP(*(rust_lock_classes))
    PROVIDE_HIDDEN(__stop_rust_lock_classes = .);

    *(SORT_BY_ALIGNMENT(.data*))
  }

  /*
   * Orphans that are actually writable can go here and get the linker's
   * default `__start_*` and `__stop_*` symbols.
   *
   * There's no way to define `__data_end` precisely since orphans would
   * go after it.  So it's just aligned to match __bss_start.
   */

  .bss : ALIGN(16) {
    PROVIDE_HIDDEN(__data_end = .);
    PROVIDE_HIDDEN(__bss_start = .);

    /*
     * See kernel/include/lib/counters.h; the KCOUNTER macro defines a
     * kcounter.NAME array in the .bss.kcounter.NAME section that allocates
     * SMP_MAX_CPUS counter slots.  Here we collect all those together to
     * make up the kcounters_arena contiguous array.  There is no particular
     * reason to sort these, but doing so makes them line up in parallel
     * with the sorted .kcounter.desc section.  Note that placement of the
     * input sections in the arena has no actual bearing on how the space is
     * used, because nothing ever refers to these arrays as variables--they
     * exist only to get the right amount of space allocated in the arena.
     * Instead, the order of the .kcounter.desc entries is what determines
     * how the arena is used: each index in the desc table corresponds to an
     * index in a per-CPU array, and the arena is a contiguous block of
     * SMP_MAX_CPUS such arrays.  The region containing the arena is
     * page-aligned and padded out to page size so that it can be exported
     * as a VMO without exposing any other .bss contents.
     */
    . = ALIGN(CONSTANT(MAXPAGESIZE));
    PROVIDE_HIDDEN(kcounters_arena = .);
    *(SORT_BY_NAME(.bss.kcounter.*))
    PROVIDE_HIDDEN(kcounters_arena_end = .);
    . = ALIGN(CONSTANT(MAXPAGESIZE));
    PROVIDE_HIDDEN(kcounters_arena_page_end = .);

    /*
     * Sanity check that the aggregate size of kcounters_arena SMP_MAX_CPUS
     * slots for each counter.  The counters::Descriptor structs in
     * .kcounter.desc are 64 bytes each.  (It's only for this sanity check
     * that we need to care how big counters::Descriptor is.)
     */
    ASSERT(kcounters_arena_end - kcounters_arena ==
           (kcountdesc_end - kcountdesc_begin) * 8 * SMP_MAX_CPUS / 64,
           "kcounters_arena size mismatch");

    *(SORT_BY_ALIGNMENT(.bss*))
  }

  /*
   * Any SHT_NOBITS (.bss-like) sections would be inserted here.
   */

  . = ALIGN(CONSTANT(MAXPAGESIZE));

  /*
   * This is a no-op for . but GNU ld requires it to generate PT_GNU_RELRO.
   */
  . = DATA_SEGMENT_END(.);

  PROVIDE_HIDDEN(_end = .);
}
