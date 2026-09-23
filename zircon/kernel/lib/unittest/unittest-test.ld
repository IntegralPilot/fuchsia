/*
 * Copyright 2026 The Fuchsia Authors
 *
 * Use of this source code is governed by a MIT-style
 * license that can be found in the LICENSE file or at
 * https://opensource.org/licenses/MIT
 */

/*
 * We link the unittest test executable with this script for the sake of KEEP()
 * paranoia, to ensure that the unittest special section doesn't get GC'ed.
 */

SECTIONS
{
  .data.rel.ro.unittest_testcases : {
    __start_unittest_testcases = .;
    KEEP(*(.data.rel.ro.unittest_testcases))
    __stop_unittest_testcases = .;
  }
}
INSERT AFTER .rodata;
