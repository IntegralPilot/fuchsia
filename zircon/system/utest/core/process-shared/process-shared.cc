// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/defer.h>
#include <lib/maybe-standalone-test/maybe-standalone.h>
#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <lib/zx/profile.h>
#include <lib/zx/resource.h>
#include <lib/zx/result.h>
#include <lib/zx/thread.h>
#include <zircon/errors.h>
#include <zircon/processargs.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/resource.h>

#include <mini-process/mini-process.h>
#include <zxtest/zxtest.h>

#include "../needs-next.h"

NEEDS_NEXT_SYSCALL(zx_process_create_shared);

namespace {

zx::result<zx::resource> GetSystemProfileResource(zx::unowned_resource& resource) {
  zx::resource system_profile_resource;
  const zx_status_t status =
      zx::resource::create(*resource, ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_PROFILE_BASE, 1, nullptr,
                           0, &system_profile_resource);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(system_profile_resource));
}

void SetDeadlineMemoryPriority(zx::unowned_resource& resource, zx::vmar& vmar) {
  zx::profile profile;
  zx_profile_info_t profile_info = {.flags = ZX_PROFILE_INFO_FLAG_MEMORY_PRIORITY,
                                    .priority = ZX_PRIORITY_HIGH};

  zx::result<zx::resource> result = GetSystemProfileResource(resource);
  ASSERT_OK(result.status_value());
  zx_status_t status = zx::profile::create(result.value(), 0u, &profile_info, &profile);
  if (status == ZX_ERR_ACCESS_DENIED) {
    // If we are running as a component test, and not a zbi test, we do not have the root job and
    // cannot create a profile. This is not an issue as when running tests as a component
    // compression is not enabled so the profile is not needed anyway.
    // TODO(https://fxbug.dev/42138396): Once compression is enabled for builds with component tests
    // support setting a profile via the profile provider.
    return;
  }
  ASSERT_OK(status);
  EXPECT_OK(vmar.set_profile(profile, 0));
}

}  // namespace

TEST(ProcessShared, MapInPrototype) {
  NEEDS_NEXT_SKIP(zx_process_create_shared);

  zx::process prototype_process;
  zx::vmar shared_vmar;
  constexpr const char kPrototypeName[] = "prototype_process";
  ASSERT_OK(zx::process::create(*zx::job::default_job(), kPrototypeName, sizeof(kPrototypeName),
                                ZX_PROCESS_SHARED, &prototype_process, &shared_vmar));

  zx::process process;
  zx::vmar restricted_vmar;
  constexpr const char kProcessName[] = "process";
  ASSERT_OK(zx_process_create_shared(prototype_process.get(), 0, kProcessName, sizeof(kProcessName),
                                     process.reset_and_get_address(),
                                     restricted_vmar.reset_and_get_address()));

  // Map a vmo into the shared vmar using the prototype handle.
  zx_handle_t vmo;
  ASSERT_OK(zx_vmo_create(zx_system_get_page_size(), 0, &vmo));
  uintptr_t addr;
  ASSERT_OK(zx_vmar_map(shared_vmar.get(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                        zx_system_get_page_size(), &addr));

  // Write some data to the vmo.
  std::vector<char> shared_data = {'a', 'b', 'c'};
  ASSERT_OK(zx_vmo_write(vmo, shared_data.data(), 0, shared_data.size()));

  // Read process memory from both processes to make sure they have the data.
  std::vector<char> read_data = {'d', 'e', 'f'};
  size_t actual = 0;
  ASSERT_EQ(prototype_process.read_memory(addr, read_data.data(), read_data.size(), &actual),
            ZX_OK);
  ASSERT_EQ(read_data, shared_data);

  // Now read the same address from the second process.
  read_data = {'d', 'e', 'f'};
  ASSERT_EQ(process.read_memory(addr, read_data.data(), read_data.size(), &actual), ZX_OK);
  ASSERT_EQ(read_data, shared_data);
}

TEST(ProcessShared, RestrictedVmarNotShared) {
  NEEDS_NEXT_SKIP(zx_process_create_shared);

  zx::process prototype_process;
  zx::vmar shared_vmar;
  constexpr const char kPrototypeName[] = "prototype_process";
  ASSERT_OK(zx::process::create(*zx::job::default_job(), kPrototypeName, sizeof(kPrototypeName),
                                ZX_PROCESS_SHARED, &prototype_process, &shared_vmar));

  zx::process process;
  zx::vmar restricted_vmar;
  constexpr const char kProcessName[] = "process";
  ASSERT_OK(zx_process_create_shared(prototype_process.get(), 0, kProcessName, sizeof(kProcessName),
                                     process.reset_and_get_address(),
                                     restricted_vmar.reset_and_get_address()));

  // Map a vmo into the restricted vmar of the second process.
  zx_handle_t vmo;
  ASSERT_OK(zx_vmo_create(zx_system_get_page_size(), 0, &vmo));
  uintptr_t addr;
  ASSERT_OK(zx_vmar_map(restricted_vmar.get(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                        zx_system_get_page_size(), &addr));

  // Write some data to the vmo.
  std::vector<char> restricted_data = {'a', 'b', 'c'};
  ASSERT_OK(zx_vmo_write(vmo, restricted_data.data(), 0, restricted_data.size()));

  std::vector<char> read_data = {'d', 'e', 'f'};
  size_t actual = 0;
  // Now try to read the same address from the first process, which should fail.
  if (prototype_process.read_memory(addr, read_data.data(), read_data.size(), &actual) == ZX_OK) {
    // If `addr` happened to be valid, make sure that the read data was not the restricted data from
    // the prototype.
    ASSERT_NE(read_data, restricted_data);
  }
}

TEST(ProcessShared, InvalidPrototype) {
  NEEDS_NEXT_SKIP(zx_process_create_shared);

  zx::process prototype_process;
  zx::vmar prototype_vmar;
  constexpr const char kPrototypeName[] = "prototype_process";
  ASSERT_OK(zx::process::create(*zx::job::default_job(), kPrototypeName, sizeof(kPrototypeName), 0,
                                &prototype_process, &prototype_vmar));

  zx::process process;
  zx::vmar restricted_vmar;
  constexpr const char kProcessName[] = "process";
  ASSERT_EQ(zx_process_create_shared(prototype_process.get(), 0, kProcessName, sizeof(kProcessName),
                                     process.reset_and_get_address(),
                                     restricted_vmar.reset_and_get_address()),
            ZX_ERR_INVALID_ARGS);
}

TEST(ProcessShared, InfoProcessVmos) {
  NEEDS_NEXT_SKIP(zx_process_create_shared);

  // Build a shareable process.
  static constexpr char kSharedProcessName[] = "object-info-shar-proc";
  zx::process shared_process;
  zx::vmar shared_vmar;
  ASSERT_OK(zx::process::create(*zx::job::default_job(), kSharedProcessName,
                                sizeof(kSharedProcessName), ZX_PROCESS_SHARED, &shared_process,
                                &shared_vmar));

  // Build another process that shares its address space.
  static constexpr char kPrivateProcessName[] = "object-info-priv-proc";
  zx::process private_process;
  zx::vmar private_vmar;
  ASSERT_OK(zx_process_create_shared(
      shared_process.get(), 0, kPrivateProcessName, sizeof(kPrivateProcessName),
      private_process.reset_and_get_address(), private_vmar.reset_and_get_address()));

  // Map a single VMO that contains some code (a busy loop) and a stack into the shared address
  // space.
  zx_vaddr_t stack_base, sp;
  ASSERT_OK(mini_process_load_stack(shared_vmar.get(), true, &stack_base, &sp));

  // Allocate two extra VMOs.
  const size_t kVmoSize = zx_system_get_page_size();
  zx::vmo vmo1, vmo2;
  ASSERT_OK(zx::vmo::create(kVmoSize, 0u, &vmo1));
  ASSERT_OK(zx::vmo::create(kVmoSize, 0u, &vmo2));

  // Start the shared process and pass a handle to vmo1 to it.
  zx::thread shared_thread;
  static constexpr char kSharedThreadName[] = "object-info-shar-thrd";
  ASSERT_OK(zx::thread::create(shared_process, kSharedThreadName, sizeof(kSharedThreadName), 0u,
                               &shared_thread));
  ASSERT_OK(shared_process.start(shared_thread, stack_base, sp, std::move(vmo1), 0));
  auto shared_cleanup = fit::defer([&] { shared_process.kill(); });

  // Start the private process.
  zx::thread private_thread;
  static constexpr char kPrivateThreadName[] = "object-info-priv-thrd";
  ASSERT_OK(zx::thread::create(private_process, kPrivateThreadName, sizeof(kPrivateThreadName), 0u,
                               &private_thread));
  ASSERT_OK(private_process.start(private_thread, stack_base, sp, zx::handle(), 0));
  auto private_cleanup = fit::defer([&] { private_process.kill(); });

  // Map vmo2 into the private address space only.
  zx_vaddr_t vaddr;
  ASSERT_OK(private_vmar.map(ZX_VM_PERM_READ, 0u, vmo2, 0, kVmoSize, &vaddr));

  // Buffer big enough to read all of the test processes' VMO entries: the mini-process VMO, vmo1
  // and vmo2.
  const size_t buf_size = 3;
  std::unique_ptr<zx_info_vmo_t[]> buf(new zx_info_vmo_t[buf_size]);

  // Read the VMO entries of the shared process.
  size_t actual, available_shared;
  ASSERT_OK(shared_process.get_info(ZX_INFO_PROCESS_VMOS, buf.get(),
                                    buf_size * sizeof(zx_info_vmo_t), &actual, &available_shared));
  ASSERT_EQ(actual, available_shared);
  ASSERT_EQ(2, available_shared);

  // Read the VMO entries of the private process.
  size_t available_private;
  ASSERT_OK(private_process.get_info(ZX_INFO_PROCESS_VMOS, buf.get(),
                                     buf_size * sizeof(zx_info_vmo_t), &actual,
                                     &available_private));
  ASSERT_EQ(actual, available_private);
  ASSERT_EQ(available_shared + 1, available_private);

  // Verify that the VMO entries from the private address space are accounted correctly if they
  // don't fit in the buffer.
  const size_t smallbuf_size = available_shared;
  std::unique_ptr<zx_info_vmo_t[]> smallbuf(new zx_info_vmo_t[smallbuf_size]);
  size_t actual_smallbuf, available_smallbuf;
  ASSERT_OK(private_process.get_info(ZX_INFO_PROCESS_VMOS, smallbuf.get(),
                                     smallbuf_size * sizeof(zx_info_vmo_t), &actual_smallbuf,
                                     &available_smallbuf));
  ASSERT_EQ(smallbuf_size, actual_smallbuf);
  ASSERT_EQ(available_private, available_smallbuf);
}

// Verify mappings in shared processes are properly accounted for in zx_info_task_stats_t.
//
// See also https://fxbug.dev/42074452.
TEST(ProcessShared, InfoTaskStats) {
  NEEDS_NEXT_SKIP(zx_process_create_shared);

  // First, verify we have access to the system resource to run this test.
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available, skipping\n");
  }
  // We're going to create 3 processes, proc1, proc2, and proc3, with a total of 4 VMARs.  proc1 and
  // proc2 will share a region (vmar_shared) and each have their own private region (vmar1 and
  // vmar2).  proc3 will have one (unshared) region (vmar3).
  //
  // We'll then map 6 VMOs (m[0] through m[5]) and verify that each process's zx_info_task_stats_t
  // accurately counts the private/shared pages and that the mem_scaled_shared_bytes "sums to 1".

  // First, we'll need to create a process with a shared region that we can use to create proc1 and
  // proc2.  We're only creating this process, proc0, so that we can create the region that proc1
  // and proc2 will share.  We'll destroy proc0 before we create any mappings.
  zx::process proc0;
  zx::vmar vmar_shared;
  static constexpr char kNameProc0[] = "proc0";
  ASSERT_OK(zx::process::create(*zx::job::default_job(), kNameProc0, sizeof(kNameProc0),
                                ZX_PROCESS_SHARED, &proc0, &vmar_shared));

  // Create proc1 and proc2.
  zx::process proc1;
  zx::vmar vmar1;
  static constexpr char kNameProc1[] = "proc1";
  ASSERT_OK(zx_process_create_shared(proc0.get(), 0, kNameProc1, sizeof(kNameProc1),
                                     proc1.reset_and_get_address(), vmar1.reset_and_get_address()));
  zx::process proc2;
  zx::vmar vmar2;
  static constexpr char kNameProc2[] = "proc2";
  ASSERT_OK(zx_process_create_shared(proc0.get(), 0, kNameProc2, sizeof(kNameProc2),
                                     proc2.reset_and_get_address(), vmar2.reset_and_get_address()));

  // We can now destroy proc0 since it has served its purpose (to facilitate creation of proc1 and
  // proc2).
  proc0.reset();

  // Create proc3, a regular old process.
  zx::process proc3;
  zx::vmar vmar3;
  static constexpr char kNameProc3[] = "proc3";
  ASSERT_OK(zx::process::create(*zx::job::default_job(), kNameProc3, sizeof(kNameProc3), 0, &proc3,
                                &vmar3));

  // With all the processes created apply a deadline memory priority to them all so that our memory
  // stats are predictable and will not change due to compression.
  ASSERT_NO_FATAL_FAILURE(SetDeadlineMemoryPriority(system_resource, vmar_shared));
  ASSERT_NO_FATAL_FAILURE(SetDeadlineMemoryPriority(system_resource, vmar1));
  ASSERT_NO_FATAL_FAILURE(SetDeadlineMemoryPriority(system_resource, vmar2));
  ASSERT_NO_FATAL_FAILURE(SetDeadlineMemoryPriority(system_resource, vmar3));

  // Now create the 6 VMOs of 1 page each.
  const size_t kSize = zx_system_get_page_size();
  zx::vmo m[6];
  const char buffer[1] = {'A'};
  for (zx::vmo& i : m) {
    ASSERT_OK(zx::vmo::create(kSize, 0u, &i));
    // Write into the VMO to ensure a page is committed.
    ASSERT_OK(i.write(buffer, 0, sizeof(buffer)));
  }

  // Make sure all the task stats start as 0.
  auto assertProcStatsAreZero = [](zx::process& proc) {
    zx_info_task_stats_t stats{};
    size_t actual{};
    size_t avail{};
    ASSERT_OK(proc.get_info(ZX_INFO_TASK_STATS, &stats, sizeof(stats), &actual, &avail));
    EXPECT_EQ(0, stats.mem_mapped_bytes);
    EXPECT_EQ(0, stats.mem_private_bytes);
    EXPECT_EQ(0, stats.mem_shared_bytes);
    EXPECT_EQ(0, stats.mem_scaled_shared_bytes);
  };
  zx::process* procs[] = {&proc1, &proc2, &proc3};
  for (zx::process* p : procs) {
    ASSERT_NO_FAILURES(assertProcStatsAreZero(*p));
  }

  // Now we'll start mapping the VMOs and verify the stats as we go.

  // vmar1 will get m[0] and m[1].
  zx_vaddr_t vaddr{};
  ASSERT_OK(vmar1.map(ZX_VM_PERM_READ | ZX_VM_MAP_RANGE, 0u, m[0], 0, kSize, &vaddr));
  ASSERT_OK(vmar1.map(ZX_VM_PERM_READ | ZX_VM_MAP_RANGE, 0u, m[1], 0, kSize, &vaddr));
  zx_info_task_stats_t stats{};
  size_t actual{};
  size_t avail{};
  ASSERT_OK(proc1.get_info(ZX_INFO_TASK_STATS, &stats, sizeof(stats), &actual, &avail));
  ASSERT_EQ(1, actual);
  EXPECT_EQ(2 * kSize, stats.mem_mapped_bytes);
  EXPECT_EQ(2 * kSize, stats.mem_private_bytes);
  EXPECT_EQ(0, stats.mem_shared_bytes);
  EXPECT_EQ(0, stats.mem_scaled_shared_bytes);
  ASSERT_NO_FAILURES(assertProcStatsAreZero(proc2));
  ASSERT_NO_FAILURES(assertProcStatsAreZero(proc3));

  // vmar_shared will get m[2] and m[3].
  ASSERT_OK(vmar_shared.map(ZX_VM_PERM_READ | ZX_VM_MAP_RANGE, 0u, m[2], 0, kSize, &vaddr));
  ASSERT_OK(vmar_shared.map(ZX_VM_PERM_READ | ZX_VM_MAP_RANGE, 0u, m[3], 0, kSize, &vaddr));

  // At this point, we have a total of 4 pages mapped.  2 are private and 2 are shared, with proc1
  // and proc2 each being fractionally-responsible for 1 of the 2 shared pages.

  ASSERT_OK(proc1.get_info(ZX_INFO_TASK_STATS, &stats, sizeof(stats), &actual, &avail));
  EXPECT_EQ(4 * kSize, stats.mem_mapped_bytes);
  EXPECT_EQ(2 * kSize, stats.mem_private_bytes);
  EXPECT_EQ(2 * kSize, stats.mem_shared_bytes);
  EXPECT_EQ(kSize, stats.mem_scaled_shared_bytes);

  ASSERT_OK(proc2.get_info(ZX_INFO_TASK_STATS, &stats, sizeof(stats), &actual, &avail));
  EXPECT_EQ(2 * kSize, stats.mem_mapped_bytes);
  EXPECT_EQ(0, stats.mem_private_bytes);
  EXPECT_EQ(2 * kSize, stats.mem_shared_bytes);
  EXPECT_EQ(kSize, stats.mem_scaled_shared_bytes);

  // vmar2 will get m[4] and m[5].
  ASSERT_OK(vmar2.map(ZX_VM_PERM_READ | ZX_VM_MAP_RANGE, 0u, m[4], 0, kSize, &vaddr));
  ASSERT_OK(vmar2.map(ZX_VM_PERM_READ | ZX_VM_MAP_RANGE, 0u, m[5], 0, kSize, &vaddr));

  // We've now got 6 pages mapped.  2 are private to proc1, 2 are private to proc2 and 2 are shared
  // between them.  Each is fractionally responsible for 1 of the shared pages.

  ASSERT_OK(proc2.get_info(ZX_INFO_TASK_STATS, &stats, sizeof(stats), &actual, &avail));
  EXPECT_EQ(4 * kSize, stats.mem_mapped_bytes);
  EXPECT_EQ(2 * kSize, stats.mem_private_bytes);
  EXPECT_EQ(2 * kSize, stats.mem_shared_bytes);
  EXPECT_EQ(kSize, stats.mem_scaled_shared_bytes);

  // Now bring proc3 into the mix.

  // vmar3 will get m[1], m[2], and m[4].
  ASSERT_OK(vmar3.map(ZX_VM_PERM_READ | ZX_VM_MAP_RANGE, 0u, m[1], 0, kSize, &vaddr));
  ASSERT_OK(vmar3.map(ZX_VM_PERM_READ | ZX_VM_MAP_RANGE, 0u, m[2], 0, kSize, &vaddr));
  ASSERT_OK(vmar3.map(ZX_VM_PERM_READ | ZX_VM_MAP_RANGE, 0u, m[4], 0, kSize, &vaddr));

  // Still 6 mapped pages.  proc1 and proc2 each have 1 private page and 3 shared pages.  proc3 has
  // 0 private pages and 3 shared pages.  The important thing here is that the
  // mem_scaled_shared_bytes of the three processes plus the private pages sum to 6 pages.
  //
  // See that there are 2 private pages and that proc1 and proc2 are each fractionally-responsible
  // for 1.25 pages, while proc3 is fractionally-responsible for 1.5 pages.
  //
  // 2 + 2 * 1.25 + 1.5 == 6
  ASSERT_OK(proc1.get_info(ZX_INFO_TASK_STATS, &stats, sizeof(stats), &actual, &avail));
  EXPECT_EQ(4 * kSize, stats.mem_mapped_bytes);
  EXPECT_EQ(1 * kSize, stats.mem_private_bytes);
  EXPECT_EQ(3 * kSize, stats.mem_shared_bytes);
  EXPECT_EQ((kSize / 2) + (kSize / 2) + (kSize / 4), stats.mem_scaled_shared_bytes);

  ASSERT_OK(proc2.get_info(ZX_INFO_TASK_STATS, &stats, sizeof(stats), &actual, &avail));
  EXPECT_EQ(4 * kSize, stats.mem_mapped_bytes);
  EXPECT_EQ(1 * kSize, stats.mem_private_bytes);
  EXPECT_EQ(3 * kSize, stats.mem_shared_bytes);
  EXPECT_EQ((kSize / 2) + (kSize / 2) + (kSize / 4), stats.mem_scaled_shared_bytes);

  ASSERT_OK(proc3.get_info(ZX_INFO_TASK_STATS, &stats, sizeof(stats), &actual, &avail));
  EXPECT_EQ(3 * kSize, stats.mem_mapped_bytes);
  EXPECT_EQ(0, stats.mem_private_bytes);
  EXPECT_EQ(3 * kSize, stats.mem_shared_bytes);
  EXPECT_EQ((kSize / 2) + (kSize / 2) + (kSize / 2), stats.mem_scaled_shared_bytes);

  // Kill off proc2.  We're now down to 5 pages.  See that it sums up properly.
  //
  // 2 + 1 + (0.5 + 0.5) + (0.5 + 0.5) == 5
  proc2.reset();
  ASSERT_OK(proc1.get_info(ZX_INFO_TASK_STATS, &stats, sizeof(stats), &actual, &avail));
  EXPECT_EQ(4 * kSize, stats.mem_mapped_bytes);
  EXPECT_EQ(2 * kSize, stats.mem_private_bytes);
  EXPECT_EQ(2 * kSize, stats.mem_shared_bytes);
  EXPECT_EQ((kSize / 2) + (kSize / 2), stats.mem_scaled_shared_bytes);

  ASSERT_OK(proc3.get_info(ZX_INFO_TASK_STATS, &stats, sizeof(stats), &actual, &avail));
  EXPECT_EQ(3 * kSize, stats.mem_mapped_bytes);
  EXPECT_EQ(1 * kSize, stats.mem_private_bytes);
  EXPECT_EQ(2 * kSize, stats.mem_shared_bytes);
  EXPECT_EQ((kSize / 2) + (kSize / 2), stats.mem_scaled_shared_bytes);
}

TEST(ProcessShared, InfoProcessMaps) {
  NEEDS_NEXT_SKIP(zx_process_create_shared);

  // Create a shared process.
  zx::process shared_process;
  zx::vmar shared_vmar;
  static constexpr char kSharedName[] = "shared_process";
  ASSERT_OK(zx::process::create(*zx::job::default_job(), kSharedName, sizeof(kSharedName),
                                ZX_PROCESS_SHARED, &shared_process, &shared_vmar));

  // Create a process that will be used to test restricted mode mapping.
  zx::process restricted_process;
  zx::vmar restricted_vmar;
  static constexpr char kNameRestrictedProc[] = "restricted_process";
  ASSERT_OK(zx_process_create_shared(
      shared_process.get(), 0, kNameRestrictedProc, sizeof(kNameRestrictedProc),
      restricted_process.reset_and_get_address(), restricted_vmar.reset_and_get_address()));

  // Create a VMO and map it into the shared address space.
  zx_vaddr_t vaddr{};
  const size_t kSize = zx_system_get_page_size();
  zx::vmo shared_vmo;
  ASSERT_OK(zx::vmo::create(kSize, 0u, &shared_vmo));
  ASSERT_OK(shared_vmar.map(ZX_VM_PERM_READ | ZX_VM_MAP_RANGE, 0u, shared_vmo, 0, kSize, &vaddr));

  // Create a VMO and map it into the restricted address space.
  zx::vmo restricted_vmo;
  ASSERT_OK(zx::vmo::create(kSize, 0u, &restricted_vmo));
  ASSERT_OK(
      restricted_vmar.map(ZX_VM_PERM_READ | ZX_VM_MAP_RANGE, 0u, restricted_vmo, 0, kSize, &vaddr));

  // Verify that ZX_INFO_PROCESS_MAPS returns the correct mappings in the
  // correct ordering for both the shared process and the restricted process.
  size_t actual;
  size_t avail;
  size_t buffer_size = 6 * sizeof(zx_info_maps_t);
  zx_info_maps_t maps[6]{};
  ASSERT_OK(
      shared_process.get_info(ZX_INFO_PROCESS_MAPS, (void*)maps, buffer_size, &actual, &avail));

  // We expect 3 entries here:
  // 1. The root vmar of the shared process' shared address space.
  // 2. The vm aspace of the shared process' shared address space (identical to the vmar).
  // 3. The VMO we mapped into the shared address space.
  ASSERT_EQ(actual, 3);
  ASSERT_EQ(avail, actual);

  // Assert that the base addresses are in non-descending order for the shared process.
  zx_vaddr_t prev_base = 0;
  for (size_t i = 0; i < actual; i++) {
    EXPECT_GE(maps[i].base, prev_base);
    prev_base = maps[i].base;
  }

  // Assert that the base addresses are in non-descending order for the restricted process.
  ASSERT_OK(
      restricted_process.get_info(ZX_INFO_PROCESS_MAPS, (void*)maps, buffer_size, &actual, &avail));
  // We expect 6 entries here. 3 are identical to the ones in the shared address space, but we have
  // the following additional entries:
  // 1. The root VMAR of the restricted address space.
  // 2. The vm aspace of restricted address space (identical to the vmar).
  // 3. The VM we mapped into the restricted address space.
  ASSERT_EQ(actual, 6);
  ASSERT_EQ(avail, actual);
  prev_base = 0;
  for (size_t i = 0; i < actual; i++) {
    EXPECT_GE(maps[i].base, prev_base);
    prev_base = maps[i].base;
  }
}

// See that a process created via zx_process_create_shared starts off with an independent copy of
// the prototype process's:
//   * ZX_PROP_PROCESS_BREAK_ON_LOAD
//   * ZX_PROP_PROCESS_DEBUG_ADDR
TEST(ProcessShared, PropertiesCopied) {
  NEEDS_NEXT_SKIP(zx_process_create_shared);

  zx::process prototype_process;
  zx::vmar shared_vmar;
  constexpr const char kPrototypeName[] = "prototype_process";
  ASSERT_OK(zx::process::create(*zx::job::default_job(), kPrototypeName, sizeof(kPrototypeName),
                                ZX_PROCESS_SHARED, &prototype_process, &shared_vmar));

  // ZX_PROP_PROCESS_BREAK_ON_LOAD
  const uint64_t break_on_load = 0x10adf00d;
  ASSERT_OK(prototype_process.set_property(ZX_PROP_PROCESS_BREAK_ON_LOAD, &break_on_load,
                                           sizeof(break_on_load)));

  // ZX_PROP_PROCESS_DEBUG_ADDR
  const uint64_t debug_addr = 0xfeedface;
  ASSERT_OK(
      prototype_process.set_property(ZX_PROP_PROCESS_DEBUG_ADDR, &debug_addr, sizeof(debug_addr)));

  zx::process process;
  zx::vmar restricted_vmar;
  constexpr const char kProcessName[] = "process";
  ASSERT_OK(zx_process_create_shared(prototype_process.get(), 0, kProcessName, sizeof(kProcessName),
                                     process.reset_and_get_address(),
                                     restricted_vmar.reset_and_get_address()));

  {
    // ZX_PROP_PROCESS_BREAK_ON_LOAD

    // See that the new processes inherits the prototype's value.
    uint64_t value{};
    ASSERT_OK(process.get_property(ZX_PROP_PROCESS_BREAK_ON_LOAD, &value, sizeof(value)));
    ASSERT_EQ(value, break_on_load);

    // See that the new process's value can diverge from the prototype's.
    const uint64_t new_value = 0;
    ASSERT_OK(process.set_property(ZX_PROP_PROCESS_BREAK_ON_LOAD, &new_value, sizeof(new_value)));
    ASSERT_OK(prototype_process.get_property(ZX_PROP_PROCESS_BREAK_ON_LOAD, &value, sizeof(value)));
    ASSERT_EQ(value, break_on_load);
  }

  {
    // ZX_PROP_PROCESS_DEBUG_ADDR

    // See that the new processes inherits the prototype's value.
    uint64_t value{};
    ASSERT_OK(process.get_property(ZX_PROP_PROCESS_DEBUG_ADDR, &value, sizeof(value)));
    ASSERT_EQ(value, debug_addr);

    // See that the new process's value can diverge from the prototype's.
    const uint64_t new_value = 0;
    ASSERT_OK(process.set_property(ZX_PROP_PROCESS_DEBUG_ADDR, &new_value, sizeof(new_value)));
    ASSERT_OK(prototype_process.get_property(ZX_PROP_PROCESS_DEBUG_ADDR, &value, sizeof(value)));
    ASSERT_EQ(value, debug_addr);
  }
}

// See that a process created via zx_process_create_shared shares the prototype process's:
//   * ZX_PROP_PROCESS_VDSO_BASE_ADDRESS
//   * ZX_PROP_PROCESS_HW_TRACE_CONTEXT_ID
TEST(ProcessShared, PropertiesShared) {
  NEEDS_NEXT_SKIP(zx_process_create_shared);

  zx::process prototype_process;
  zx::vmar shared_vmar;
  constexpr const char kPrototypeName[] = "prototype_process";
  ASSERT_OK(zx::process::create(*zx::job::default_job(), kPrototypeName, sizeof(kPrototypeName),
                                ZX_PROCESS_SHARED, &prototype_process, &shared_vmar));

  // ZX_PROP_PROCESS_VDSO_BASE_ADDRESS
  uint64_t vdso_base_addr;
  ASSERT_OK(prototype_process.get_property(ZX_PROP_PROCESS_VDSO_BASE_ADDRESS, &vdso_base_addr,
                                           sizeof(vdso_base_addr)));

#if defined(__x86_64__)
  // ZX_PROP_PROCESS_HW_TRACE_CONTEXT_ID
  //
  // Only on x86 and only available when kernel.enable-debugging-syscalls=true.
  bool debugging_syscalls_enabled;
  uint64_t hw_trace_context_id;
  zx_status_t status = prototype_process.get_property(
      ZX_PROP_PROCESS_HW_TRACE_CONTEXT_ID, &hw_trace_context_id, sizeof(hw_trace_context_id));
  if (status != ZX_ERR_NOT_SUPPORTED) {
    ASSERT_OK(status);
    ASSERT_NE(0, hw_trace_context_id);
    debugging_syscalls_enabled = true;
  }
#endif

  zx::process process;
  zx::vmar restricted_vmar;
  constexpr const char kProcessName[] = "process";
  ASSERT_OK(zx_process_create_shared(prototype_process.get(), 0, kProcessName, sizeof(kProcessName),
                                     process.reset_and_get_address(),
                                     restricted_vmar.reset_and_get_address()));

  {
    // ZX_PROP_PROCESS_VDSO_BASE_ADDRESS

    // See that the shared process matches the prototype.  I.e. both start at zero.
    uint64_t process_value{};
    ASSERT_OK(process.get_property(ZX_PROP_PROCESS_VDSO_BASE_ADDRESS, &process_value,
                                   sizeof(process_value)));
    ASSERT_EQ(process_value, vdso_base_addr);
    ASSERT_EQ(0, vdso_base_addr);

    // Map the vDSO into the prototype, which will implicitly set the base address property for both
    // processes.
    ASSERT_OK(mini_process_load_vdso(prototype_process.get(), shared_vmar.get(), nullptr, nullptr));

    // See that the property of both processes match.
    ASSERT_OK(process.get_property(ZX_PROP_PROCESS_VDSO_BASE_ADDRESS, &process_value,
                                   sizeof(process_value)));
    ASSERT_NE(0, process_value);
    uint64_t prototype_value{};
    ASSERT_OK(process.get_property(ZX_PROP_PROCESS_VDSO_BASE_ADDRESS, &prototype_value,
                                   sizeof(prototype_value)));
    ASSERT_NE(0, prototype_value);
    ASSERT_EQ(process_value, prototype_value);
  }

#if defined(__x86_64__)
  {
    // ZX_PROP_PROCESS_HW_TRACE_CONTEXT_ID
    if (debugging_syscalls_enabled) {
      uint64_t value{};
      ASSERT_OK(process.get_property(ZX_PROP_PROCESS_HW_TRACE_CONTEXT_ID, &value, sizeof(value)));
      ASSERT_EQ(value, hw_trace_context_id);
    }
  }
#endif
}

// Regression test for https://fxbug.dev/507137840.
// Verify that mapping a page at the very end of a shared process's VMAR boundary
// succeeds and does not panic the kernel due to under-allocated/unpopulated
// top-level page table entries during InitShared.
TEST(ProcessShared, SharedVmarBoundary) {
  zx::process shared_proc;
  zx::vmar shared_vmar;
  constexpr const char kSharedName[] = "shared";
  ASSERT_OK(zx::process::create(*zx::job::default_job(), kSharedName, sizeof(kSharedName),
                                ZX_PROCESS_SHARED, &shared_proc, &shared_vmar));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  ASSERT_OK(vmo.op_range(ZX_VMO_OP_COMMIT, 0, zx_system_get_page_size(), nullptr, 0));

  zx_info_vmar_t vmar_info;
  ASSERT_OK(shared_vmar.get_info(ZX_INFO_VMAR, &vmar_info, sizeof(vmar_info), nullptr, nullptr));

  zx_vaddr_t mapped_addr;
  zx_vaddr_t offset = vmar_info.len - zx_system_get_page_size();

  ASSERT_OK(shared_vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC | ZX_VM_MAP_RANGE,
                            offset, vmo, 0, zx_system_get_page_size(), &mapped_addr));
}

// Regression test for https://fxbug.dev/507137840.
// Verify that process destruction cleans up the mappings at the shared VMAR
// boundary correctly without panicking the kernel (targeting DestroyIndividual).
TEST(ProcessShared, SharedVmarBoundaryDestruction) {
  zx::process shared_proc;
  zx::vmar shared_vmar;
  constexpr const char kSharedName[] = "shared";
  ASSERT_OK(zx::process::create(*zx::job::default_job(), kSharedName, sizeof(kSharedName),
                                ZX_PROCESS_SHARED, &shared_proc, &shared_vmar));

  // Explicitly reset the process handle to trigger process destruction
  // and page table cleanup (DestroyIndividual) within the test scope.
  shared_proc.reset();
  shared_vmar.reset();
}

// Regression test for https://fxbug.dev/507137840.
// Verify that the PML4 prepopulation boundary is calculated correctly in InitUnified
// by mapping at the very end of the shared VMAR, creating a shared child process,
// and verifying the child process can successfully read from the mapped address.
TEST(ProcessShared, UnifiedVmarBoundary) {
  NEEDS_NEXT_SKIP(zx_process_create_shared);

  zx::process prototype_process;
  zx::vmar shared_vmar;
  constexpr const char kPrototypeName[] = "prototype_process";
  ASSERT_OK(zx::process::create(*zx::job::default_job(), kPrototypeName, sizeof(kPrototypeName),
                                ZX_PROCESS_SHARED, &prototype_process, &shared_vmar));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  zx_info_vmar_t vmar_info;
  ASSERT_OK(shared_vmar.get_info(ZX_INFO_VMAR, &vmar_info, sizeof(vmar_info), nullptr, nullptr));
  uintptr_t offset = vmar_info.len - zx_system_get_page_size();

  uintptr_t addr;
  ASSERT_OK(shared_vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, offset, vmo, 0,
                            zx_system_get_page_size(), &addr));

  std::vector<char> shared_data = {'a', 'b', 'c'};
  ASSERT_OK(vmo.write(shared_data.data(), 0, shared_data.size()));

  zx::process process;
  zx::vmar restricted_vmar;
  constexpr const char kProcessName[] = "process";
  ASSERT_OK(zx_process_create_shared(prototype_process.get(), 0, kProcessName, sizeof(kProcessName),
                                     process.reset_and_get_address(),
                                     restricted_vmar.reset_and_get_address()));

  std::vector<char> read_data = {'d', 'e', 'f'};
  size_t actual = 0;
  ASSERT_EQ(prototype_process.read_memory(addr, read_data.data(), read_data.size(), &actual),
            ZX_OK);
  ASSERT_EQ(read_data, shared_data);

  read_data = {'d', 'e', 'f'};
  ASSERT_EQ(process.read_memory(addr, read_data.data(), read_data.size(), &actual), ZX_OK);
  ASSERT_EQ(read_data, shared_data);
}
