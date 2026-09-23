// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "dl-iterate-phdr-tests.h"

#include <gmock/gmock.h>

#include "dl-load-tests.h"
#include "startup-symbols.h"

namespace {

using dl::testing::TestModule;
using dl::testing::TestSym;

using dl::testing::CollectModulePhdrInfo;
using dl::testing::GetGlobalCounters;
using dl::testing::GetPhdrInfoForModule;
using dl::testing::gStartupPhdrInfo;
using dl::testing::ModuleInfoList;
using dl::testing::ModulePhdrInfo;

using dl::testing::DlTests;
TYPED_TEST_SUITE(DlTests, dl::testing::TestTypes, dl::testing::TestNames);

// Test that `dl_iterate_phdr` includes startup modules.
TYPED_TEST(DlTests, DlIteratePhdrStartupModules) {
  ModuleInfoList startup_info_list;
  EXPECT_EQ(this->DlIteratePhdr(CollectModulePhdrInfo, &startup_info_list), 0);

  // If the dlopen implementation can't unload modules or if previous tests
  // tested with RTLD_NODELETE, there will be additional modules that are
  // collected by dl_iterate_phdr that were loaded by tests that ran before this
  // one. This checks that `startup_info_list` at least starts with the elements
  // in `gStartupPhdrInfo`.
  EXPECT_THAT(std::span(startup_info_list).subspan(0, gStartupPhdrInfo.size()),
              ::testing::ElementsAreArray(gStartupPhdrInfo));
}

// Test the following as it affects `dl_iterate_phdr` output:
// dlopen module
// `dl_iterate_phdr` output includes new module
// dlclose module
// `dl_iterate_phdr` output doesn't include the module
TYPED_TEST(DlTests, DlIteratePhdrBasic) {
  const std::string kRet17File = TestModule("ret17");

  // Record initial values to compare against during the test.
  ModuleInfoList initial_info_list;
  EXPECT_EQ(this->DlIteratePhdr(CollectModulePhdrInfo, &initial_info_list), 0);
  const ModulePhdrInfo last_phdr_info = initial_info_list.back();

  const auto [initial_loaded, initial_unloaded] = GetGlobalCounters(this);

  this->ExpectRootModule(kRet17File);

  auto open_ret17 = this->DlOpen(kRet17File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_ret17.is_ok()) << open_ret17.error_value();
  EXPECT_TRUE(open_ret17.value());

  // Check that a struct `dl_pdhr_info` is produced for the dlopen-ed module.
  ModulePhdrInfo ret17_phdr_info = GetPhdrInfoForModule(*this, kRet17File);

  // Check that the `.dlpi_adds` counter is adjusted.
  const size_t loaded_after_open = GetGlobalCounters(this).adds;
  EXPECT_EQ(loaded_after_open, initial_loaded + 1);

  // Look up a symbol from the module and expect that its pointer value should
  // be within the address range of the module's phdrs from its
  // `dl_phdr_info`.
  auto ret17_test_start = this->DlSym(open_ret17.value(), TestSym("TestStart").c_str());
  ASSERT_TRUE(ret17_test_start.is_ok()) << ret17_test_start.error_value();
  EXPECT_TRUE(ret17_test_start.value());

  EXPECT_TRUE(ret17_phdr_info.contains_addr(reinterpret_cast<uintptr_t>(ret17_test_start.value())));

  ASSERT_TRUE(this->DlClose(open_ret17.value()).is_ok());

  // A final check that dl-closing the module will remove its entry and update
  // the `.dlpi_subs` counter.
  ModuleInfoList close_info_list;
  EXPECT_EQ(this->DlIteratePhdr(CollectModulePhdrInfo, &close_info_list), 0);
  // The last entry should be the same as at the beginning of the test.
  const ModulePhdrInfo test_last_phdr_info = close_info_list.back();

  const size_t unloaded_after_close = GetGlobalCounters(this).subs;

  if (TestFixture::kDlCloseUnloadsModules) {
    EXPECT_EQ(unloaded_after_close, initial_unloaded + 1);
    EXPECT_EQ(test_last_phdr_info, last_phdr_info);
    EXPECT_EQ(close_info_list.size(), initial_info_list.size());
  } else {
    // Musl-Fuchsia's dlclose is a no-op and does not change dl_iterate_phdr
    // output: the module entry is preserved.
    EXPECT_EQ(unloaded_after_close, initial_unloaded);
    EXPECT_EQ(test_last_phdr_info, ret17_phdr_info);
    EXPECT_EQ(close_info_list.size(), initial_info_list.size() + 1);
  }
}

TYPED_TEST(DlTests, DlIteratePhdrTlsData) {
  // This test module has only one variable in its PT_TLS segment, so that
  // variable will always be at the start of its block.
  auto info = GetPhdrInfoForModule(*this, "libstatic-tls-var.so");
  EXPECT_EQ(info.tls_data(), &gStaticTlsVar)
      << "  Both should be reative to $tp\n    Which is: " << __builtin_thread_pointer();
}

}  // namespace
