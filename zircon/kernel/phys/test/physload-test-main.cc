// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "physload-test-main.h"

#include <stdio.h>

#include <ktl/utility.h>

#include "../physload.h"

#include <ktl/enforce.h>

extern "C" void PhysLoadModuleMain(UartDriver& uart, PhysBootTimes boot_times,
                                   KernelStorage kernel_storage) {
  if (PhysLoadTestMain(kernel_storage) == 0) {
    printf("\n*** Test succeeded ***\n%s\n\n", BOOT_TEST_SUCCESS_STRING);
  }
  abort();
}
