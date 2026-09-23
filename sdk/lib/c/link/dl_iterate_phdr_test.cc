// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/link/dl_iterate_phdr.h"

#include <lib/ld/dl-phdr-info.h>
#include <lib/ld/module.h>
#include <lib/ld/tls.h>

#include <concepts>
#include <optional>

#include "../dlfcn/dl/test/module-phdr-info.h"
#include "../dlfcn/dlfcn-abi.h"
#include "../ld/ld-abi.h"
#include "src/__support/common.h"
#include "test/UnitTest/Test.h"

namespace LIBC_NAMESPACE_DECL {
namespace {

// Because the global declaration from <link.h> is also in scope, all calls
// here have to be disambiguated with explicit LIBC_NAMESPACE::dl_iterate_phdr.
// The libc unittests binaries this is linked into don't include dlopen calls,
// so there should never be any new modules other than the startup modules.

int CallIterate(std::invocable<const dl_phdr_info&> auto&& callback)
  requires(std::same_as<int, decltype(callback(dl_phdr_info{}))>)
{
  using Ptr = decltype(&callback);
  return LIBC_NAMESPACE::dl_iterate_phdr(
      [](dl_phdr_info* info, size_t size, void* callback) {
        EXPECT_EQ(size, sizeof(dl_phdr_info));
        return (*static_cast<Ptr>(callback))(*info);
      },
      &callback);
}

TEST(LibcDlIteratePhdrTests, Modules) {
  ASSERT_EQ(&_dlfcn_iterate_phdr, nullptr);

  dl::testing::ModuleInfoList impl_list;
  EXPECT_EQ(LIBC_NAMESPACE::dl_iterate_phdr(dl::testing::CollectModulePhdrInfo, &impl_list), 0);

  // Under gtest we'd just do:
  // EXPECT_THAT(impl_list, ::testing::ContainerEq(dl::testing::gStartupPhdrInfo));
  ASSERT_EQ(impl_list.size(), dl::testing::gStartupPhdrInfo.size());
  for (size_t i = 0; i < impl_list.size(); ++i) {
    EXPECT_EQ(impl_list[i], dl::testing::gStartupPhdrInfo[i]);
  }
}

TEST(LibcDlIteratePhdrTests, Counts) {
  ASSERT_EQ(&_dlfcn_phdr_info_counts, nullptr);

  std::optional<ld::DlPhdrInfoCounts> counts;
  EXPECT_EQ(CallIterate([&counts](const dl_phdr_info& info) {
              const ld::DlPhdrInfoCounts info_counts = {
                  .adds = info.dlpi_adds,
                  .subs = info.dlpi_subs,
              };
              if (!counts) {  // First iteration.
                counts = info_counts;
              } else {  // Later iterations should all report the same.
                EXPECT_EQ(info_counts, *counts);
              }
              return 0;
            }),
            0);
  ASSERT_TRUE(counts);
  EXPECT_EQ(*counts, ld::DlPhdrInfoInitialExecCounts(_ld_abi));
}

}  // namespace
}  // namespace LIBC_NAMESPACE_DECL
