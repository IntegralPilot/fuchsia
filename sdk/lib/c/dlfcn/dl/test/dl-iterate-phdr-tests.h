// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_DLFCN_DL_TEST_DL_ITERATE_PHDR_TESTS_H_
#define LIB_C_DLFCN_DL_TEST_DL_ITERATE_PHDR_TESTS_H_

#include <lib/ld/dl-phdr-info.h>

#include <gtest/gtest.h>

#include "module-phdr-info.h"

namespace dl::testing {

// TODO(https://fxbug.dev/394873439): Test the sequential changes in state of
// these counters.
//
// Return loaded and unloaded counters; these counters describe the state of
// the number of modules that have been loaded and unloaded when this call was
// made.
template <class TestFixture>
inline ld::DlPhdrInfoCounts GetGlobalCounters(TestFixture* test_fixture) {
  // This callback assumes the counters taken from the first dl_phdr_info
  // reflects the true global state, ignoring the possibility of
  // de-synchronization of these counters when another thread dlopen/dlclose
  // and changes these values.
  auto get_counters = [](dl_phdr_info* phdr_info, size_t size, void* data) {
    auto* counters = reinterpret_cast<ld::DlPhdrInfoCounts*>(data);
    counters->adds = phdr_info->dlpi_adds;
    counters->subs = phdr_info->dlpi_subs;
    // Use the first set of counters we see and halt enumeration.
    return 1;
  };

  ld::DlPhdrInfoCounts counters;
  EXPECT_EQ(test_fixture->DlIteratePhdr(get_counters, &counters), 1);
  return counters;
}

// Locate the dl_phdr_info for a specific module by its file basename.
inline ModulePhdrInfo GetPhdrInfoForModule(auto& test, std::string_view module_name) {
  ModuleInfoList list;
  EXPECT_EQ(test.DlIteratePhdr(CollectModulePhdrInfo, &list), 0);
  auto info = std::ranges::find_if(
      list, [&](const ModulePhdrInfo& info) { return info.basename() == module_name; });
  EXPECT_NE(info, list.end());
  return info == list.end() ? ModulePhdrInfo{} : *info;
}

}  // namespace dl::testing

#endif  // LIB_C_DLFCN_DL_TEST_DL_ITERATE_PHDR_TESTS_H_
