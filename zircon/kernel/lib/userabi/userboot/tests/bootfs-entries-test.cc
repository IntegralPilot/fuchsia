// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.boot/cpp/wire_types.h>
#include <lib/standalone-test/standalone.h>
#include <lib/zx/channel.h>
#include <lib/zx/process.h>
#include <link.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <algorithm>
#include <set>

#include <zxtest/zxtest.h>

#include "helper.h"

namespace {

TEST(UserbootBootfsEntriesTest, CheckBootfsEntries) {
  std::vector<BootfsFileVmo> entries;
  ASSERT_NO_FAILURES(GetBootfsEntries(entries));
  // Base address of RO section of each module.
  std::set<uintptr_t> modules;
  dl_iterate_phdr(
      [](struct dl_phdr_info *hdr, size_t hdr_size, void *ck) -> int {
        auto *modules = static_cast<std::set<uint64_t> *>(ck);
        // Just pick the first, doesn't really matter which, its just for finding the VMO that is
        // backing the mapping.
        modules->insert(hdr->dlpi_addr + hdr->dlpi_phdr[0].p_vaddr);
        return 0;
      },
      &modules);
  // One of the entries is the VDSO which is not a blob handed over from bootfs.
  ASSERT_EQ(modules.size(), entries.size() + 1);

  // Determine that all the blobs in entries exist in this process by checking the base addresses
  // and matching them up.
  size_t actual_count = 0;
  size_t avail_count = 0;
  std::vector<zx_info_maps_t> mappings;
  // Just so we don't generate an extra mapping when resizing the vector below.
  ASSERT_OK(
      zx::process::self()->get_info(ZX_INFO_PROCESS_MAPS, nullptr, 0, &actual_count, &avail_count));
  ASSERT_GT(avail_count, 0);
  // Add some extra entries, just in case reallocation triggers any sort of additional mappings.
  mappings.resize(avail_count + 5);
  avail_count = 0;
  ASSERT_OK(zx::process::self()->get_info(ZX_INFO_PROCESS_MAPS, mappings.data(),
                                          mappings.size() * sizeof(zx_info_maps_t), &actual_count,
                                          &avail_count));
  ASSERT_EQ(actual_count, avail_count);

  std::set<zx_koid_t> module_koids;
  for (size_t i = 0; i < actual_count; ++i) {
    auto &mapping = mappings[i];
    if (mapping.type != ZX_INFO_MAPS_TYPE_MAPPING) {
      continue;
    }

    auto it = modules.lower_bound(mapping.base);
    if (it == modules.end()) {
      continue;
    }
    // `it` may or not be contained in our mapping.
    if (*it < mapping.base + mapping.size) {
      module_koids.insert(mapping.u.mapping.vmo_koid);
    }
  }

  // Now we have a unique mapping from a module to a mapping. Let's check that the mappings KOIDs
  // and content match. We don't need to match the entire VMO contents, since some sections may be
  // execute only, but the selected mapping will give us the offset in the vmo and the mapping size
  // should give us the length we can use to compare. The VMO KOID will allow us to match those
  // handed over by userboot protocol.
  std::set<zx_koid_t> entry_koids;
  for (const auto &entry : entries) {
    entry_koids.insert(GetKoid(entry.contents.get()));
  }

  std::vector<zx_koid_t> not_found_koids;
  std::ranges::set_difference(module_koids, entry_koids, std::back_inserter(not_found_koids));
  // There should only be one missing KOID, that is the vDSO VMO.
  EXPECT_EQ(not_found_koids.size(), 1);

  not_found_koids.clear();

  std::ranges::set_difference(entry_koids, module_koids, std::back_inserter(not_found_koids));
  // All VMOs in `entries` are present in the mappings, but not all VMOs in mappings are present in
  // entries to do the vDSO.
  EXPECT_TRUE(not_found_koids.empty());
}

}  // namespace
