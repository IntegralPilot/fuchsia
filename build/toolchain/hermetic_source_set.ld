/* Copyright 2025 The Fuchsia Authors. All rights reserved.
 * Use of this source code is governed by a BSD-style license that can be
 * found in the LICENSE file.
 */

SECTIONS {
  /*
   * This is necessary to protect the unwind information data from
   * --gc-sections.  Otherwise it has no references in the hermetic partial
   * link.  However, it must survive into the final link to get referenced
   * implicitly from .eh_frame_hdr or the hermetic code will be without
   * unwind information.  This means the hermetic link will preserve a
   * unified .eh_frame section of dead references to GC'd code sections.
   * The linkers treat the unwind section references to otherwise-dead code
   * as a special case (as with nonallocated debug sections) and leaves
   * "tombstone" address values rather than taking them as live references.
   * So this doesn't fully defeat --gc-sections from the hermetic link.
   * But the outer link will have to optimize out tombstoned and dead FDEs
   * and CIEs from .eh_frame to recover what --gc-sections would ideally
   * have done at the hermetic stage, rather than just dropping at whole
   * input section granularity.
   */
  .debug_frame : { KEEP(*(.debug_frame)) }
  .eh_frame : { KEEP(*(.eh_frame)) }
  .ARM.extab : { KEEP(*(.ARM.extab)) }
  .ARM.exidx : { KEEP(*(.ARM.exidx)) }

  /*
   * All other sections get either GC or "orphan placement", which for -r
   * just means merging of same-named sections.
   */
}
