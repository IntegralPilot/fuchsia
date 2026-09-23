// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_UTEST_CORE_OBJECT_INFO_PROCESS_FIXTURE_H_
#define ZIRCON_SYSTEM_UTEST_CORE_OBJECT_INFO_PROCESS_FIXTURE_H_

#include <lib/fit/defer.h>
#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <lib/zx/vmo.h>
#include <zircon/syscalls/object.h>

#include <cinttypes>
#include <climits>
#include <type_traits>

#include <mini-process/mini-process.h>
#include <zxtest/zxtest.h>

namespace object_info_test {

inline zx_status_t GetKoid(const zx::object_base& object, zx_koid_t* koid) {
  zx_info_handle_basic_t info;
  zx_status_t status =
      zx_object_get_info(object.get(), ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  if (status == ZX_OK) {
    *koid = info.koid;
  }
  return status;
}

// Structs to keep track of the VMARs/mappings in test child process.
struct Mapping {
  uintptr_t base;
  size_t size = 0;
  // ZX_INFO_MAPS_MMU_FLAG_PERM_{READ,WRITE,EXECUTE}
  uint32_t flags;
};

// A VMO that the test process maps or has a handle to.
struct Vmo {
  zx_koid_t koid;
  size_t size = 0;
  uint32_t flags;
};

struct MappingInfo {
  uintptr_t vmar_base = 0;
  size_t vmar_size = 0;
  // num_mappings entries
  size_t num_mappings = 0;
  std::unique_ptr<Mapping[]> mappings = nullptr;
  // num_vmos entries
  size_t num_vmos = 0;
  std::unique_ptr<Vmo[]> vmos = 0;
};

class ProcessFixture : public zxtest::Test {
 public:
  static void SetUpTestSuite() {
    // Create a VMO whose handle we'll give to the test process.
    // It will not be mapped into the test process's VMAR.
    zx::vmo unmapped_vmo;
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0u, &unmapped_vmo),
              "Failed to create unmapped_vmo.");

    zx_koid_t unmapped_vmo_koid = ZX_KOID_INVALID;
    ASSERT_OK(GetKoid(unmapped_vmo, &unmapped_vmo_koid), "Failed to obtain koid");

    // Try to set the name, but ignore any errors.
    unmapped_vmo.set_property(ZX_PROP_NAME, kUnmappedVmoName, sizeof(kUnmappedVmoName));

    // Failures from here on will start to leak handles, but they'll
    // be cleaned up when this binary exits.

    ASSERT_OK(zx::process::create(*zx::job::default_job(), kProcessName, sizeof(kProcessName),
                                  /* options */ 0u, &process_, &vmar_));

    zx::thread thread;
    ASSERT_OK(zx::thread::create(process_, kThreadName, sizeof(kThreadName), 0u, &thread),
              "Failed to create thread.");

    zx::channel minip_channel;
    // Start the process before we mess with the VMAR,
    // so we don't step on the mapping done by start_mini_process_etc.
    ASSERT_OK(
        start_mini_process_etc(process_.get(), thread.get(), vmar_.get(), unmapped_vmo.release(),
                               true, minip_channel.reset_and_get_address()),
        "Failed to start mini process.");
    minip_channel.reset();

    // Create a child VMAR and a mapping under it, so we have
    // something interesting to look at when getting the process's
    // memory maps. After this, the process maps should at least contain:
    //
    //   Root Aspace
    //   - Root VMAR
    //     - Code+stack mapping created by start_mini_process_etc
    //     - Sub VMAR created below
    //       - kNumMappings mappings created below
    info_.num_mappings = kNumMappings;
    info_.mappings.reset(new Mapping[kNumMappings]);

    // Big enough to fit all of the mappings with some slop.
    info_.vmar_size = zx_system_get_page_size() * kNumMappings * 16;
    zx::vmar sub_vmar;

    ASSERT_OK(vmar_.allocate(ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_EXECUTE, 0,
                             info_.vmar_size, &sub_vmar, &info_.vmar_base));

    zx::vmo vmo;
    // Create the VMO twice as large as needed, so that we can map in every second page. Mapping in
    // every second page ensures that mappings cannot get merged in the kernel, and so we will have
    // the exact number of mappings we made reported back to us, without needing to perform
    // additional processing to notice the merge.
    const size_t kVmoSize = zx_system_get_page_size() * kNumMappings * 2;
    ASSERT_OK(zx::vmo::create(kVmoSize, 0u, &vmo), "Failed to create vmo.");

    zx_koid_t vmo_koid = ZX_KOID_INVALID;
    ASSERT_OK(GetKoid(vmo, &vmo_koid));

    // Try to set the name, but ignore any errors.
    static constexpr char kVmoName[] = "test:mapped";

    vmo.set_property(ZX_PROP_NAME, kVmoName, sizeof(kVmoName));

    // TODO(mdempsky): Restructure test to satisfy W^X.
    zx::vmo replace;
    vmo.replace_as_executable(zx::resource(), &replace);
    vmo = std::move(replace);

    // Record the VMOs now that we have both of them.
    info_.num_vmos = 2;
    info_.vmos.reset(new Vmo[2]);
    info_.vmos[0].koid = unmapped_vmo_koid;
    info_.vmos[0].size = zx_system_get_page_size();
    info_.vmos[0].flags = ZX_INFO_VMO_VIA_HANDLE;
    info_.vmos[1].koid = vmo_koid;
    info_.vmos[1].size = kVmoSize;
    info_.vmos[1].flags = ZX_INFO_VMO_VIA_MAPPING;

    // Map each page of the VMO to some arbitray location in the VMAR.
    for (size_t i = 0; i < kNumMappings; ++i) {
      Mapping& m = info_.mappings[i];
      m.size = zx_system_get_page_size();

      // Pick flags for this mapping; cycle through different
      // combinations for the test. Must always have READ set
      // to be mapped.
      m.flags = ZX_VM_PERM_READ;
      if (i & 1) {
        m.flags |= ZX_VM_PERM_WRITE;
      }
      if (i & 2) {
        m.flags |= ZX_VM_PERM_EXECUTE;
      }

      // Map in every second page of the VMO to ensure mappings do not get merged internally.
      ASSERT_OK(sub_vmar.map(m.flags, 0, vmo, (i * 2) * zx_system_get_page_size(),
                             zx_system_get_page_size(), &m.base),
                "zx::vmar::map [%zd]", i);
    }

    // Check that everything is ok.
    ASSERT_TRUE(process_.is_valid());
  }

  static void TearDownTestSuite() {
    if (vmar_.is_valid()) {
      vmar_.destroy();
      vmar_.reset();
    }

    if (process_.is_valid()) {
      process_.kill();
      process_.reset();
    }
  }

  // Returns the process singleton to be used for ZX_INFO_PROCESS_MAPS.
  const zx::process& GetProcess() const { return process_; }

  // Returns the vmar singleton to be used for ZX_INFO_VMAR_MAPS.  This vmar contains the one
  // described by |GetInfo|.
  const zx::vmar& GetVmar() const { return vmar_; }

  // Return the MappingInfo for the process institated for this fixture.
  const MappingInfo& GetInfo() const { return info_; }

  const auto& GetHandleProvider() const { return handle_provider; }

 private:
  // Constants.
  static constexpr char kProcessName[] = "object-info-mini-proc";
  static constexpr char kUnmappedVmoName[] = "test:unmapped";
  static constexpr char kThreadName[] = "object-info-mini-thrd";
  static constexpr size_t kNumMappings = 8;

  // Singletons for the ProcessFixture
  static zx::process process_;
  static zx::vmar vmar_;
  static MappingInfo info_;

  fit::function<const zx::process&()> handle_provider = [this]() -> const zx::process& {
    return GetProcess();
  };
};

}  // namespace object_info_test

#endif  // ZIRCON_SYSTEM_UTEST_CORE_OBJECT_INFO_PROCESS_FIXTURE_H_
