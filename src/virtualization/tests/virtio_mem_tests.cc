// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/component/incoming/cpp/protocol.h>

#include "src/lib/fxl/strings/string_printf.h"
#include "src/virtualization/tests/lib/enclosed_guest.h"
#include "src/virtualization/tests/lib/guest_test.h"

// Current Termina and Debian linux kernels do not support virtio-mem driver on arm64
// 5.15 kernel which is used by Termina supports virtio-mem only on x86-64
// https://elixir.bootlin.com/linux/v5.15.81/source/drivers/virtio/Kconfig#L99
//
// TODO(https://fxbug.dev/42051237): Enable ARM once kernel is updated to 5.18 or later
// https://elixir.bootlin.com/linux/v5.18/source/drivers/virtio/Kconfig#L108
#ifndef __aarch64__
namespace {

template <class T>
class MemGuestTest : public GuestTest<T> {
 public:
  MemGuestTest() = default;
};

constexpr uint64_t kOneMebibyte = 1024u * 1024;
constexpr uint64_t kDefaultBlockSize = 4u * kOneMebibyte;
constexpr uint64_t kDefaultMemoryRegionSize = static_cast<uint64_t>(1) * 1024u * kOneMebibyte;

// Memory block size here is the minimal DIMM size configured to 128MiB on Linux x86 and 2MiB on
// Windows
// If virtio mem start address is not aligned to 128MiB, virtio mem would spit out a warning and
// then fails to reserve the given memory range.
// 128MiB is mentioned a few times in virtio-mem patchsets
// See
// https://lore.kernel.org/kvm/c244851d-ef0d-f680-090d-e90b5be3103e@redhat.com/
// https://lists.oasis-open.org/archives/virtio-dev/202005/msg00039.html
constexpr uint64_t kLinuxMinimalDIMMSize = 128u * kOneMebibyte;

class VirtioMemTerminaGuest : public TerminaEnclosedGuest {
 public:
  VirtioMemTerminaGuest(async_dispatcher_t* dispatcher, RunLoopUntilFunc run_loop_until)
      : TerminaEnclosedGuest(dispatcher, std::move(run_loop_until)) {}

  zx_status_t BuildLaunchInfo(GuestLaunchInfo* launch_info) override {
    zx_status_t status = TerminaEnclosedGuest::BuildLaunchInfo(launch_info);
    if (status != ZX_OK) {
      return status;
    }

    auto client_end = component::Connect<fuchsia_kernel::Stats>();
    EXPECT_TRUE(client_end.is_ok());
    auto mem_stats = fidl::Call(*client_end)->GetMemoryStats();
    uint64_t free_bytes = mem_stats.value().stats().free_bytes().value();

    // Use 512 MiB to boot up the guest
    uint64_t kDesiredGuestMemory = 512 * kOneMebibyte;
    // Don't use more than 2/3 of the available memory
    // Align to the DIMM size
    uint64_t upper_limit_guest_memory = (free_bytes / 3 * 2) & ~(kLinuxMinimalDIMMSize - 1);
    uint64_t guest_memory = std::min(kDesiredGuestMemory, upper_limit_guest_memory);

    // TODO(https://fxbug.dev/42051237): Bump up the number of PCI slots
    //
    // Not enough PCI slots, turn off a few devices
    launch_info->config.set_guest_memory(guest_memory);
    launch_info->config.set_virtio_rng(false);
    launch_info->config.set_virtio_mem(true);
    launch_info->config.set_virtio_balloon(true);
    launch_info->config.set_virtio_mem_block_size(kDefaultBlockSize);
    launch_info->config.set_virtio_mem_region_size(kDefaultMemoryRegionSize);
    launch_info->config.set_virtio_mem_region_alignment(kLinuxMinimalDIMMSize);
    return ZX_OK;
  }
};

class VirtioMemDebianGuest : public DebianEnclosedGuest {
 public:
  VirtioMemDebianGuest(async_dispatcher_t* dispatcher, RunLoopUntilFunc run_loop_until)
      : DebianEnclosedGuest(dispatcher, std::move(run_loop_until)) {}

  zx_status_t BuildLaunchInfo(GuestLaunchInfo* launch_info) override {
    zx_status_t status = DebianEnclosedGuest::BuildLaunchInfo(launch_info);
    if (status != ZX_OK) {
      return status;
    }
    // TODO(https://fxbug.dev/42051237): Bump up the number of PCI slots
    //
    // Not enough PCI slots, turn off a few devices
    launch_info->config.set_virtio_rng(false);
    launch_info->config.set_virtio_mem(true);
    launch_info->config.set_virtio_balloon(true);
    launch_info->config.set_virtio_mem_block_size(kDefaultBlockSize);
    launch_info->config.set_virtio_mem_region_size(kDefaultMemoryRegionSize);
    launch_info->config.set_virtio_mem_region_alignment(kLinuxMinimalDIMMSize);
    return ZX_OK;
  }
};

constexpr uint16_t VIRTIO_BALLOON_S_MEMFREE = 4;
constexpr uint16_t VIRTIO_BALLOON_S_MEMTOT = 5;
constexpr uint16_t VIRTIO_BALLOON_S_AVAIL = 6;

std::unordered_map<uint16_t, uint64_t> GetGuestMemStats(
    fuchsia::virtualization::BalloonControllerSyncPtr& balloon_controller) {
  ::fidl::VectorPtr<::fuchsia::virtualization::MemStat> mem_stats;
  int32_t mem_stats_status = 0;
  zx_status_t status = balloon_controller->GetMemStats(&mem_stats_status, &mem_stats);
  EXPECT_EQ(status, ZX_OK);
  EXPECT_EQ(mem_stats_status, ZX_OK);
  std::unordered_map<uint16_t, uint64_t> stats;
  for (auto& el : mem_stats.value()) {
    stats[el.tag] = el.val;
  }
  return stats;
}

void RequestMemoryAndWaitForCompletion(
    const std::string& label, uint64_t new_requested_size,
    fuchsia::virtualization::MemControllerSyncPtr& mem_controller,
    fuchsia::virtualization::BalloonControllerSyncPtr& balloon_controller) {
  auto stats = GetGuestMemStats(balloon_controller);
  FX_LOGS(INFO) << "Memory stats before " << label << ":"
                << " TotalMemory = " << stats[VIRTIO_BALLOON_S_MEMTOT]
                << " AvailableMemory = " << stats[VIRTIO_BALLOON_S_AVAIL]
                << " FreeMemory = " << stats[VIRTIO_BALLOON_S_MEMFREE];

  uint64_t prev_total_memory = stats[VIRTIO_BALLOON_S_MEMTOT];
  mem_controller->RequestSize(new_requested_size);
  FX_LOGS(INFO) << "Adjusting plugged size to " << new_requested_size;

  uint64_t block_size;
  uint64_t region_size;
  uint64_t usable_region_size;
  uint64_t plugged_size;
  uint64_t requested_size;

  while (true) {
    zx_status_t status = mem_controller->GetMemSize(&block_size, &region_size, &usable_region_size,
                                                    &plugged_size, &requested_size);

    stats = GetGuestMemStats(balloon_controller);
    ASSERT_EQ(status, ZX_OK);
    ASSERT_EQ(block_size, kDefaultBlockSize);
    ASSERT_EQ(region_size, kDefaultMemoryRegionSize);
    ASSERT_EQ(usable_region_size, kDefaultMemoryRegionSize);
    ASSERT_EQ(requested_size, new_requested_size);
    uint64_t mem_delta = std::max(stats[VIRTIO_BALLOON_S_MEMTOT], prev_total_memory) -
                         std::min(stats[VIRTIO_BALLOON_S_MEMTOT], prev_total_memory);
    if (plugged_size == requested_size && mem_delta == requested_size) {
      break;
    }
    zx::nanosleep(zx::deadline_after(zx::msec(100)));

    FX_LOGS(INFO) << "Waiting Memory stats:"
                  << " TotalMemory = " << stats[VIRTIO_BALLOON_S_MEMTOT]
                  << " AvailableMemory = " << stats[VIRTIO_BALLOON_S_AVAIL]
                  << " FreeMemory = " << stats[VIRTIO_BALLOON_S_MEMFREE]
                  << " plugged_size = " << plugged_size;
  }

  FX_LOGS(INFO) << "Memory stats after " << label << ":"
                << " TotalMemory = " << stats[VIRTIO_BALLOON_S_MEMTOT]
                << " AvailableMemory = " << stats[VIRTIO_BALLOON_S_AVAIL]
                << " FreeMemory = " << stats[VIRTIO_BALLOON_S_MEMFREE]
                << " plugged_size = " << plugged_size;
}

// TODO(https://fxbug.dev/42051237): Re-enable Debian after finding out why it doesn't update the total system
// memory after the successful hotplug
using GuestTypes = ::testing::Types<VirtioMemTerminaGuest>;
TYPED_TEST_SUITE(MemGuestTest, GuestTypes, GuestTestNameGenerator);

TYPED_TEST(MemGuestTest, PlugAndUnplug) {
  fuchsia::virtualization::MemControllerSyncPtr mem_controller;
  ASSERT_TRUE(this->ConnectToMem(mem_controller.NewRequest()));

  // Validate the initial state of the virtio-mem
  uint64_t block_size;
  uint64_t region_size;
  uint64_t usable_region_size;
  uint64_t plugged_size;
  uint64_t requested_size;

  ASSERT_EQ(ZX_OK, mem_controller->GetMemSize(&block_size, &region_size, &usable_region_size,
                                              &plugged_size, &requested_size));
  ASSERT_EQ(block_size, kDefaultBlockSize);
  ASSERT_EQ(region_size, kDefaultMemoryRegionSize);
  ASSERT_EQ(usable_region_size, kDefaultMemoryRegionSize);
  ASSERT_EQ(plugged_size, 0u);
  ASSERT_EQ(requested_size, 0u);

  // use balloon GetGuestMemStats to query the total available memory before and after plug
  fuchsia::virtualization::BalloonControllerSyncPtr balloon_controller;
  ASSERT_TRUE(this->ConnectToBalloon(balloon_controller.NewRequest()));

  uint64_t free_mem_before_plug = GetGuestMemStats(balloon_controller)[VIRTIO_BALLOON_S_MEMFREE];

  constexpr uint64_t kTestPlugSize = 512 * kOneMebibyte;

  RequestMemoryAndWaitForCompletion("Plug all", kTestPlugSize, mem_controller, balloon_controller);

  const uint64_t alloc_amount_mib = free_mem_before_plug / kOneMebibyte;
  FX_LOGS(INFO) << fxl::StringPrintf("Allocate and release %lu MiB in the guest", alloc_amount_mib);

  // This call will allocate and immediate release the specified amount of
  // memory in the guest.
  // From the guest perspective, memory is available once it got released.
  // From the host perspective, memory is taken by the guest and not available
  // until it will be reclaimed by the free page reporting.
  std::string result;
  ASSERT_EQ(this->RunUtil(
                fxl::StringPrintf("memory_test_util alloc --size-mb 1 --num %lu", alloc_amount_mib),
                {}, &result),
            ZX_OK);

  // Unplug half of the memory
  RequestMemoryAndWaitForCompletion("Unplug half", kTestPlugSize / 2, mem_controller,
                                    balloon_controller);
}

}  // namespace
#endif  // not __aarch64__
