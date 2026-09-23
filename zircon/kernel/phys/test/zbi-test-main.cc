// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <stdio.h>

#include <ktl/utility.h>
#include <phys/main.h>

#include "test-main.h"

// Make sure ZbiMain can't be inlined into PhysMain via LTO, so that
// tests will have a known machine-level backtrace to TestMain.
[[gnu::noinline]] void ZbiMain(void* zbi_ptr, EarlyBootZbi zbi, arch::EarlyTicks ticks) {
  // Early boot may have filled the screen with logs. Add a newline to
  // terminate any previous line, and another newline to leave a blank.
  printf("\n\n");

  // Run the test.
  int status = TestMain(zbi_ptr, ktl::move(zbi), ticks);
  if (status == 0) {
    printf("\n*** Test succeeded ***\n%s\n\n", BOOT_TEST_SUCCESS_STRING);
  } else {
    printf("\n*** Test FAILED: status %d ***\n\n", status);
  }

  ArchPanicReset();
}
