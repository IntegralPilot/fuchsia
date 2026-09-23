// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <boot-config-contents.h>
#include <lib/fit/defer.h>
#include <string.h>

#include <cstdio>

#include "../test-main.h"
#include "lib/zbi-format/zbi.h"
#include "phys/main.h"

#include <ktl/enforce.h>

int TestMain(void* zbi_ptr, ktl::optional<EarlyBootZbi> early_zbi, arch::EarlyTicks) {
  MainSymbolize symbolize("boot-config-zbi-smoke-test");

  zbitl::View zbi(
      zbitl::StorageFromRawHeader<ktl::span<ktl::byte>>(static_cast<zbi_header_t*>(zbi_ptr)));
  auto cleanup = fit::defer([&]() { zbi.ignore_error(); });
  auto it = zbi.find(ZBI_TYPE_LINUX_BOOTCONFIG);

  if (kExpectedBootConfigContents.empty()) {
    if (it == zbi.end()) {
      printf("%s: Empty boot config.\n", ProgramName());
      return 0;
    }
    printf("%s: Unexpected boot-config item.\n", ProgramName());
    return -1;
  }

  if (it == zbi.end()) {
    printf("Expected `ZBI_TYPE_LINUX_BOOTCONFIG' item not found.\n");
    return -1;
  }

  auto [header, payload] = *it;

  if (header->length != kExpectedBootConfigContents.size()) {
    printf("%s: Expected boot config of size %zx but found %x\n", ProgramName(),
           kExpectedBootConfigContents.size(), header->length);
    return -1;
  }

  if (memcmp(payload.data(), kExpectedBootConfigContents.data(),
             kExpectedBootConfigContents.size()) != 0) {
    printf("%s: Expected boot config payload mismatch.\n Actual:\n%*s\nExpected:\n%*s\n",
           ProgramName(), static_cast<int>(payload.size_bytes()),
           reinterpret_cast<const char*>(payload.data()),
           static_cast<int>(kExpectedBootConfigContents.size()),
           kExpectedBootConfigContents.data());
    return -1;
  }

  printf("%s: Boot Config(Size = %zx) is OK!.\n", ProgramName(), payload.size_bytes());
  return 0;
}
