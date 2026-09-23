// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>
#include <lib/boot-shim/devicetree-boot-shim.h>
#include <lib/boot-shim/devicetree.h>
#include <lib/devicetree/devicetree.h>
#include <lib/devicetree/matcher.h>
#include <lib/memalloc/pool.h>
#include <lib/memalloc/range.h>
#include <lib/zbi-format/driver-config.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zbitl/storage-traits.h>
#include <stdint.h>
#include <zircon/assert.h>

#include <ktl/array.h>
#include <ktl/limits.h>
#include <ktl/span.h>
#include <ktl/type_traits.h>
#include <phys/address-space.h>
#include <phys/allocation.h>
#include <phys/boot-options.h>
#include <phys/boot-shim/devicetree.h>
#include <phys/main.h>
#include <phys/stdio.h>
#include <phys/uart-console.h>

#include "test-main.h"

void PhysMain(void* flat_devicetree_blob, arch::EarlyTicks ticks) {
  ArchSetUpCpu();
  InitStdout();
  ApplyRelocations();

  // For testing purposes, we explicitly initialize the uart from the
  // devicetree before anything. There is no harm in reinitializing the
  // uart later if the test chooses to call InitMemory.
  devicetree::ByteView fdt_blob(static_cast<const uint8_t*>(flat_devicetree_blob),
                                std::numeric_limits<uintptr_t>::max());
  devicetree::Devicetree fdt(fdt_blob);
  boot_shim::DevicetreeChosenNodeMatcher<> chosen("devicetree-test-main", stdout);
  ZX_ASSERT(devicetree::Match(fdt, chosen));

  static BootOptions boot_options;
  InstallBootOptions(&boot_options);

  if (auto uart_config = chosen.uart_config()) {
    boot_options.serial = *uart_config;
  }

  EarlyBootZbiBytes early_zbi_bytes{chosen.ramdisk()};
  EarlyBootZbi early_zbi{&early_zbi_bytes};
  SetBootOptions(boot_options, early_zbi, chosen.cmdline().value_or(""));
  SetUartConsole(boot_options.serial);
  ArchSetUpZbi(early_zbi);

  // Early boot may have filled the screen with logs. Add a newline to
  // terminate any previous line, and another newline to leave a blank.
  printf("\n\n");

  // Run the test.
  int status = TestMain(flat_devicetree_blob, {}, ticks);
  if (status == 0) {
    printf("\n*** Test succeeded ***\n%s\n\n", BOOT_TEST_SUCCESS_STRING);
  } else {
    printf("\n*** Test FAILED: status %d ***\n\n", status);
  }

  ArchPanicReset();
}
