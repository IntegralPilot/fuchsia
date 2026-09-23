// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <boot-config-contents.h>
#include <string.h>

#include <phys/address-space.h>

#include "../test-main.h"
#include "linux-boot-config.h"

#include <ktl/enforce.h>

int TestMain(void* zbi_ptr, ktl::optional<EarlyBootZbi> early_zbi, arch::EarlyTicks) {
  MainSymbolize symbolize("boot-config-smoke-test");

  // Initialize memory for allocation/free.
  AddressSpace aspace;
  InitMemory(zbi_ptr, ktl::move(early_zbi), &aspace);

  zbitl::View zbi(
      zbitl::StorageFromRawHeader<ktl::span<ktl::byte>>(static_cast<zbi_header_t*>(zbi_ptr)));
  auto linux_boot_config = GetLinuxBootConfig();
  if (!linux_boot_config.has_value()) {
    if (kExpectedBootConfigContents.empty()) {
      printf("%s: Empty boot config.\n", ProgramName());
      return 0;
    }
    return -1;
  }

  if (linux_boot_config->size_bytes() != kExpectedBootConfigContents.size()) {
    printf("%s: Expected boot config of size %zx but found %zx\n", ProgramName(),
           kExpectedBootConfigContents.size(), linux_boot_config->size_bytes());
    return -1;
  }

  if (memcmp(linux_boot_config->contents().data(), kExpectedBootConfigContents.data(),
             kExpectedBootConfigContents.size()) != 0) {
    printf("%s: Expected boot config payload mismatch.\n Actual:\n%*s\nExpected:\n%*s\n",
           ProgramName(), static_cast<int>(linux_boot_config->size_bytes()),
           linux_boot_config->contents().data(),
           static_cast<int>(kExpectedBootConfigContents.size()),
           kExpectedBootConfigContents.data());
    return -1;
  }

  printf("%s: Boot Config(Size = %zx) is OK!.\n", ProgramName(), linux_boot_config->size_bytes());
  return 0;
}
