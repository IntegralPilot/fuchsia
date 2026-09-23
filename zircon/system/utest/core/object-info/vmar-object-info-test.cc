// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <lib/zx/vmar.h>
#include <zircon/syscalls/object.h>

#include <map>
#include <memory>
#include <utility>
#include <vector>

#include <fbl/string.h>
#include <zxtest/zxtest.h>

#include "helper.h"
#include "process-fixture.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace object_info_test {
namespace {

constexpr auto job_provider = []() -> const zx::job& {
  static const zx::unowned_job job = zx::job::default_job();
  return *job;
};

constexpr auto process_provider = []() -> const zx::process& {
  static const zx::unowned_process process = zx::process::self();
  return *process;
};

constexpr auto thread_provider = []() -> const zx::thread& {
  static const zx::unowned_thread thread = zx::thread::self();
  return *thread;
};

constexpr auto vmar_provider = []() -> const zx::vmar& {
  const static zx::unowned_vmar vmar = zx::vmar::root_self();
  return *vmar;
};

// Tests for ZX_INFO_HANDLE_BASIC.
//
TEST(VmarGetInfoTest, InfoHandleBasicOnSelfSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckSelfInfoSucceeds<zx_info_handle_basic_t>(ZX_INFO_HANDLE_BASIC, vmar_provider())));
}

TEST(VmarGetInfoTest, InfoHandleBasicInvalidHandleFails) {
  ASSERT_NO_FATAL_FAILURE(
      CheckInvalidHandleFails<zx_info_handle_basic_t>(ZX_INFO_HANDLE_BASIC, vmar_provider));
}

TEST(VmarGetInfoTest, InfoHandleBasicNullAvailSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      CheckNullAvailSucceeds<zx_info_handle_basic_t>(ZX_INFO_HANDLE_BASIC, vmar_provider));
}

TEST(VmarGetInfoTest, InfoHandleBasicNullActualSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      CheckNullActualSucceeds<zx_info_handle_basic_t>(ZX_INFO_HANDLE_BASIC, vmar_provider));
}

TEST(VmarGetInfoTest, InfoHandleBasicNullActualAndAvailSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      CheckNullActualAndAvailSucceeds<zx_info_handle_basic_t>(ZX_INFO_HANDLE_BASIC, vmar_provider));
}

TEST(VmarGetInfoTest, InfoHandleBasicInvalidBufferPointerFails) {
  ASSERT_NO_FATAL_FAILURE((
      CheckInvalidBufferPointerFails<zx_info_handle_basic_t>(ZX_INFO_HANDLE_BASIC, vmar_provider)));
}

TEST(VmarGetInfoTest, InfoHandleBasicBadActualIsInvalidArg) {
  ASSERT_NO_FATAL_FAILURE(
      BadActualIsInvalidArgs<zx_info_handle_basic_t>(ZX_INFO_HANDLE_BASIC, vmar_provider));
}

TEST(VmarGetInfoTest, InfoHandleBasicBadAvailIsInvalidArg) {
  ASSERT_NO_FATAL_FAILURE(
      BadAvailIsInvalidArgs<zx_info_handle_basic_t>(ZX_INFO_HANDLE_BASIC, vmar_provider));
}

TEST(VmarGetInfoTest, InfoHandleBasicZeroSizedBufferFails) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckZeroSizeBufferFails<zx_info_handle_basic_t>(ZX_INFO_HANDLE_BASIC, vmar_provider)));
}

// Tests for ZX_INFO_VMAR.
//
TEST(VmarGetInfoTest, InfoVmarOnSelfSucceeds) {
  ASSERT_NO_FATAL_FAILURE((CheckSelfInfoSucceeds<zx_info_vmar_t>(ZX_INFO_VMAR, vmar_provider())));
}

TEST(VmarGetInfoTest, InfoVmarInvalidHandleFails) {
  ASSERT_NO_FATAL_FAILURE((CheckInvalidHandleFails<zx_info_vmar_t>(ZX_INFO_VMAR, vmar_provider)));
}

TEST(VmarGetInfoTest, InfoVmarNullAvailSucceeds) {
  ASSERT_NO_FATAL_FAILURE(CheckNullAvailSucceeds<zx_info_vmar_t>(ZX_INFO_VMAR, vmar_provider));
}

TEST(VmarGetInfoTest, InfoVmarNullActualSucceeds) {
  ASSERT_NO_FATAL_FAILURE(CheckNullActualSucceeds<zx_info_vmar_t>(ZX_INFO_VMAR, vmar_provider));
}

TEST(VmarGetInfoTest, InfoVmarNullActualAndAvailSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      CheckNullActualAndAvailSucceeds<zx_info_vmar_t>(ZX_INFO_VMAR, vmar_provider));
}

TEST(VmarGetInfoTest, InfoVmarInvalidBufferPointerFails) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckInvalidBufferPointerFails<zx_info_vmar_t>(ZX_INFO_VMAR, vmar_provider)));
}

TEST(VmarGetInfoTest, InfoVmarBadActualIsInvalidArg) {
  ASSERT_NO_FATAL_FAILURE(BadActualIsInvalidArgs<zx_info_vmar_t>(ZX_INFO_VMAR, vmar_provider));
}

TEST(VmarGetInfoTest, InfoVmarBadAvailIsInvalidArg) {
  ASSERT_NO_FATAL_FAILURE(BadAvailIsInvalidArgs<zx_info_vmar_t>(ZX_INFO_VMAR, vmar_provider));
}

TEST(VmarGetInfoTest, InfoVmarZeroSizedBufferFails) {
  ASSERT_NO_FATAL_FAILURE((CheckZeroSizeBufferFails<zx_info_vmar_t>(ZX_INFO_VMAR, vmar_provider)));
}

TEST(VmarGetInfoTest, InfoVmarJobHandleIsBadHandle) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckWrongHandleTypeFails<zx_info_vmar_t, 32>(ZX_INFO_VMAR, job_provider)));
}

TEST(VmarGetInfoTest, InfoVmarProcessHandleIsBadHandle) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckWrongHandleTypeFails<zx_info_vmar_t, 32>(ZX_INFO_VMAR, process_provider)));
}

TEST(VmarGetInfoTest, InfoVmarThreadHandleIsBadHandle) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckWrongHandleTypeFails<zx_info_vmar_t, 32>(ZX_INFO_VMAR, thread_provider)));
}

// Tests for ZX_INFO_VMAR_MAPS.
//
using VmarMapsGetInfoTest = ProcessFixture;

TEST_F(VmarMapsGetInfoTest, InfoVmarMapsOnSelfSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckSelfInfoSucceeds<zx_info_maps_t>(ZX_INFO_VMAR_MAPS, vmar_provider())));
}

TEST_F(VmarMapsGetInfoTest, InfoVmarMapsInvalidHandleFails) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckInvalidHandleFails<zx_info_maps_t>(ZX_INFO_VMAR_MAPS, vmar_provider)));
}

TEST_F(VmarMapsGetInfoTest, InfoVmarMapsNullAvailSucceeds) {
  ASSERT_NO_FATAL_FAILURE(CheckNullAvailSucceeds<zx_info_maps_t>(ZX_INFO_VMAR_MAPS, vmar_provider));
}

TEST_F(VmarMapsGetInfoTest, InfoVmarMapsNullActualSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      CheckNullActualSucceeds<zx_info_maps_t>(ZX_INFO_VMAR_MAPS, vmar_provider));
}

TEST_F(VmarMapsGetInfoTest, InfoVmarMapsNullActualAndAvailSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      CheckNullActualAndAvailSucceeds<zx_info_maps_t>(ZX_INFO_VMAR_MAPS, vmar_provider));
}

TEST_F(VmarMapsGetInfoTest, InfoVmarMapsInvalidBufferPointerFails) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckInvalidBufferPointerFails<zx_info_maps_t>(ZX_INFO_VMAR_MAPS, vmar_provider)));
}

TEST_F(VmarMapsGetInfoTest, InfoVmarMapsBadActualIsInvalidArg) {
  ASSERT_NO_FATAL_FAILURE(BadActualIsInvalidArgs<zx_info_maps_t>(ZX_INFO_VMAR_MAPS, vmar_provider));
}

TEST_F(VmarMapsGetInfoTest, InfoVmarMapsBadAvailIsInvalidArg) {
  ASSERT_NO_FATAL_FAILURE(BadAvailIsInvalidArgs<zx_info_maps_t>(ZX_INFO_VMAR_MAPS, vmar_provider));
}

TEST_F(VmarMapsGetInfoTest, InfoVmarMapsZeroSizedBufferIsOk) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckZeroSizeBufferSucceeds<zx_info_maps_t>(ZX_INFO_VMAR_MAPS, vmar_provider)));
}

TEST_F(VmarMapsGetInfoTest, InfoVmarMapsRequiresInspectRights) {
  ASSERT_NO_FATAL_FAILURE((CheckMissingRightsFail<zx_info_maps_t, 32>(
      ZX_INFO_PROCESS_MAPS, ZX_RIGHT_INSPECT, process_provider)));
}

// Tests that ZX_INFO_VMAR_MAPS does not return ZX_ERR_BAD_STATE when the containing process has not
// yet started.
//
// Like ProcessGetInfoTest.InfoProcessMapsUnstartedSucceeds, but for VMARs.
TEST_F(VmarMapsGetInfoTest, InfoVmarMapsUnstartedSucceeds) {
  static constexpr char kProcessName[] = "object-info-unstarted";
  zx::vmar vmar;
  zx::process process;
  ASSERT_OK(zx::process::create(*zx::job::default_job(), kProcessName, sizeof(kProcessName),
                                /* options */ 0u, &process, &vmar));

  size_t actual, avail;
  zx_info_maps_t maps;
  ASSERT_OK(vmar.get_info(ZX_INFO_VMAR_MAPS, &maps, sizeof(maps), &actual, &avail));
  EXPECT_EQ(1, actual);
  EXPECT_EQ(1, avail);
  EXPECT_EQ(0, maps.depth);
  EXPECT_EQ(ZX_INFO_MAPS_TYPE_VMAR, maps.type);
}

// See that ZX_INFO_VMAR_MAPS fails with ZX_ERR_BAD_STATE if the containing process has been
// destroyed.
TEST_F(VmarMapsGetInfoTest, InfoVmarMapsProcessDestroyedFails) {
  static constexpr char kProcessName[] = "object-info-p-destroyed";
  zx::vmar vmar;
  zx::process process;
  ASSERT_OK(zx::process::create(*zx::job::default_job(), kProcessName, sizeof(kProcessName),
                                /* options */ 0u, &process, &vmar));
  process.reset();

  size_t actual, avail;
  zx_info_maps_t maps;
  ASSERT_EQ(ZX_ERR_BAD_STATE, vmar.get_info(ZX_INFO_VMAR_MAPS, &maps, 1, &actual, &avail));
}

// See that ZX_INFO_VMAR_MAPS fails with ZX_ERR_BAD_STATE if the vmar has been destroyed.
TEST_F(VmarMapsGetInfoTest, InfoVmarMapsVmarDestroyedFails) {
  static constexpr char kProcessName[] = "object-info-v-destroyed";
  zx::vmar vmar;
  zx::process process;
  ASSERT_OK(zx::process::create(*zx::job::default_job(), kProcessName, sizeof(kProcessName),
                                /* options */ 0u, &process, &vmar));
  // Destroying a root vmar is not suppotred so create sub-vmar.
  zx::vmar sub_vmar;
  uintptr_t sub_vmar_addr;
  ASSERT_OK(vmar.allocate(ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                          1 * zx_system_get_page_size(), &sub_vmar, &sub_vmar_addr));

  ASSERT_OK(sub_vmar.destroy());

  size_t actual, avail;
  zx_info_maps_t maps;
  ASSERT_EQ(ZX_ERR_BAD_STATE, sub_vmar.get_info(ZX_INFO_VMAR_MAPS, &maps, 1, &actual, &avail));
}

TEST_F(VmarMapsGetInfoTest, InfoVmarMapsSmokeTest) {
  const MappingInfo& test_info = GetInfo();
  const zx::vmar& vmar = GetVmar();

  // Make a buffer big enough to hold all the entries.
  const size_t max_entries = 4 * test_info.num_mappings;
  std::unique_ptr<zx_info_maps_t[]> maps(new zx_info_maps_t[max_entries]);

  // Create a map of the test mappings that we must later find.  As we find them, we'll remove from
  // the map to ensure we got them all.
  std::map<uintptr_t, Mapping> mappings_to_find;
  for (size_t i = 0; i < test_info.num_mappings; ++i) {
    const Mapping& m = test_info.mappings[i];
    mappings_to_find[m.base] = m;
  }

  // Read the map entries.
  size_t actual;
  size_t avail;
  ASSERT_OK(vmar.get_info(ZX_INFO_VMAR_MAPS, static_cast<void*>(maps.get()),
                          max_entries * sizeof(zx_info_maps_t), &actual, &avail));
  ASSERT_LE(test_info.num_mappings, actual);
  ASSERT_LE(test_info.num_mappings, avail);

  auto dump_diag = [&]() -> fbl::String {
    std::string result;
    fxl::StringAppendf(&result, "diagnostics follow:\n");
    for (size_t i = 0; i < actual; ++i) {
      const zx_info_maps_t& entry = maps[i];
      fxl::StringAppendf(&result, "maps entry %lu, depth %lu, type %u, base %lu, size %lu\n", i,
                         entry.depth, entry.type, entry.base, entry.size);
    }
    for (size_t i = 0; i < test_info.num_mappings; ++i) {
      const Mapping& mapping = test_info.mappings[i];
      fxl::StringAppendf(&result, "mapping %lu, base %lu, size %lu\n", i, mapping.base,
                         mapping.size);
    }
    return result;
  };

  // Validate the first entry.
  ASSERT_EQ(ZX_INFO_MAPS_TYPE_VMAR, maps[0].type, "%s", dump_diag().c_str());
  ASSERT_EQ(0, maps[0].depth, "%s", dump_diag().c_str());

  // Rebuild the tree structure using two vectors and a depth.  Skip the first entry.
  //
  // parent_by_index[i] is the index of the parent of maps[i].
  std::vector<size_t> parent_by_index(actual);
  // last_parent_by_depth[i] is the last observed (index of a) parent of a node at depth i.
  std::vector<size_t> last_parent_by_depth(actual);
  // last_depth is the depth of the last observed node.
  size_t last_depth = 0;
  // The max observed parent depth.  Used to assert the traversal order and ensure accesses to
  // |last_parent_by_index| are in bounds.
  size_t max_parent_depth_seen = 0;
  for (size_t i = 1; i < actual; ++i) {
    const zx_info_maps_t& entry = maps[i];
    // Because it's a pre-order DFS, depth should never increase by more than one.
    ASSERT_TRUE(entry.depth <= max_parent_depth_seen + 1);
    switch (entry.type) {
      case ZX_INFO_MAPS_TYPE_VMAR:
        // This node is a potential parent for subsequent nodes.
        last_parent_by_depth[entry.depth + 1] = i;
        max_parent_depth_seen = std::max(max_parent_depth_seen, entry.depth);
        [[fallthrough]];
      case ZX_INFO_MAPS_TYPE_MAPPING:
        // If this node's depth is the same as the previous node, then this node shares a parent
        // with the previous node.  If this node's depth is one greater than the previous node, then
        // the previous node must be this node's parent.  If this node's depth is less than the
        // previous node, then the parent of this node is the parent of the last node we've observed
        // at this depth.
        if (entry.depth == last_depth) {
          parent_by_index[i] = parent_by_index[i - 1];
        } else if (entry.depth == last_depth + 1) {
          parent_by_index[i] = i - 1;
        } else if (entry.depth < last_depth) {
          parent_by_index[i] = last_parent_by_depth[entry.depth];
        } else {
          ASSERT_TRUE(false, "unexpected depth, %zu, with last_depth %zu, at index %lu\n%s",
                      entry.depth, last_depth, i, dump_diag().c_str());
        }
        break;
      default:
        ASSERT_TRUE(false, "unexpected type, %u, at index %lu\n%s", entry.type, i,
                    dump_diag().c_str());
    };
    last_depth = entry.depth;
  }

  // See that the result contain the sub-vmar that holds our test mappings.
  const zx_info_maps_t* maps_begin = maps.get();
  const zx_info_maps_t* maps_end = maps_begin + actual;
  const zx_info_maps_t* subvmar = std::find_if(
      maps_begin, maps_end, [&](const zx_info_maps_t& e) { return e.base == test_info.vmar_base; });
  ASSERT_NE(maps_end, subvmar, "%s", dump_diag().c_str());
  // subvmar now points at the vmar entry that contains the test mappings.

  // Iterate over the all entries (after the first).  Use the tree to validate the depth, base, and
  // size.  If the entry is one of our test mappings, cross it off the list.
  for (size_t i = 1; i < actual; ++i) {
    const zx_info_maps_t& entry = maps[i];
    const zx_info_maps_t& parent = maps[parent_by_index[i]];
    ASSERT_EQ(parent.depth + 1, entry.depth, "index %zu\n%s", i, dump_diag().c_str());
    ASSERT_LE(parent.base, entry.base, "index %zu\n%s", i, dump_diag().c_str());
    ASSERT_GE(parent.base + parent.size, entry.base + entry.size, "index %zu\n%s", i,
              dump_diag().c_str());
    // Is this one of the test mappings we should verify?
    if (entry.type == ZX_INFO_MAPS_TYPE_MAPPING && &parent == subvmar) {
      if (const auto& iter = mappings_to_find.find(entry.base); iter != mappings_to_find.end()) {
        ASSERT_EQ(iter->second.base, entry.base, "index %zu\n%s", i, dump_diag().c_str());
        ASSERT_EQ(iter->second.size, entry.size, "index %zu\n%s", i, dump_diag().c_str());
        ASSERT_EQ(iter->second.flags, entry.u.mapping.mmu_flags, "index %zu\n%s", i,
                  dump_diag().c_str());
        mappings_to_find.erase(iter);
      }
    }
  }

  // See that we found them all.
  ASSERT_EQ(0, mappings_to_find.size());
}

}  // namespace
}  // namespace object_info_test
