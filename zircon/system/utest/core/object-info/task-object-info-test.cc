// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/maybe-standalone-test/maybe-standalone.h>
#include <lib/zx/event.h>
#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <lib/zx/profile.h>
#include <lib/zx/thread.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <zircon/syscalls/object.h>

#include <zxtest/zxtest.h>

#include "helper.h"

namespace object_info_test {
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

TEST(TaskGetInfoTest, InfoStatsUnstartedSucceeds) {
  static constexpr char kProcessName[] = "object-info-unstarted";

  zx::vmar vmar;
  zx::process process;

  ASSERT_OK(zx::process::create(*zx::job::default_job(), kProcessName, sizeof(kProcessName), 0u,
                                &process, &vmar));

  zx_info_task_stats_t info;
  ASSERT_OK(process.get_info(ZX_INFO_TASK_STATS, &info, sizeof(info), nullptr, nullptr));
}

template <typename InfoT>
static void TestProcessGetInfoStatsSmokeTest(const uint32_t topic) {
  InfoT info;
  ASSERT_OK(zx::process::self()->get_info(topic, &info, sizeof(info), nullptr, nullptr));

  EXPECT_GT(info.mem_private_bytes, 0u);
  EXPECT_GE(info.mem_shared_bytes, 0u);
  EXPECT_GE(info.mem_mapped_bytes, info.mem_private_bytes + info.mem_shared_bytes);
  EXPECT_GE(info.mem_scaled_shared_bytes, 0u);
  EXPECT_GE(info.mem_shared_bytes, info.mem_scaled_shared_bytes);
}

TEST(TaskGetInfoTest, InfoStatsSmokeTest) {
  TestProcessGetInfoStatsSmokeTest<zx_info_task_stats_t>(ZX_INFO_TASK_STATS);
}

TEST(TaskGetInfoTest, InfoStatsSmokeTestV1) {
  TestProcessGetInfoStatsSmokeTest<zx_info_task_stats_v1_t>(ZX_INFO_TASK_STATS_V1);
}

constexpr auto handle_provider = []() -> const zx::process& {
  const static zx::unowned_process process = zx::process::self();
  return *process;
};

TEST(TaskGetInfoTest, InfoTaskStatsInvalidHandleFails) {
  ASSERT_NO_FATAL_FAILURE(
      CheckInvalidHandleFails<zx_info_task_stats_t>(ZX_INFO_TASK_STATS, handle_provider));
}

TEST(TaskGetInfoTest, InfoTaskStatsNullAvailSucceeds) {
  ASSERT_TRUE(handle_provider().is_valid());
  ASSERT_NO_FATAL_FAILURE(
      CheckNullAvailSucceeds<zx_info_task_stats_t>(ZX_INFO_TASK_STATS, handle_provider));
}

TEST(TaskGetInfoTest, InfoTaskStatsNullActualSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      CheckNullActualSucceeds<zx_info_task_stats_t>(ZX_INFO_TASK_STATS, handle_provider));
}

TEST(TaskGetInfoTest, InfoTaskStatsNullActualAndAvailSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      CheckNullActualAndAvailSucceeds<zx_info_task_stats_t>(ZX_INFO_TASK_STATS, handle_provider));
}

TEST(TaskGetInfoTest, InfoTaskStatsInvalidBufferPointerFails) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckInvalidBufferPointerFails<zx_info_task_stats_t>(ZX_INFO_TASK_STATS, handle_provider)));
}

TEST(TaskGetInfoTest, InfoTaskStatsBadActualIsInvalidArg) {
  ASSERT_NO_FATAL_FAILURE(
      BadActualIsInvalidArgs<zx_info_task_stats_t>(ZX_INFO_TASK_STATS, handle_provider));
}

TEST(TaskGetInfoTest, InfoTaskStatsBadAvailIsInvalidArg) {
  ASSERT_NO_FATAL_FAILURE(
      BadAvailIsInvalidArgs<zx_info_task_stats_t>(ZX_INFO_TASK_STATS, handle_provider));
}

TEST(TaskGetInfoTest, InfoTaskStatsZeroSizedBufferIsTooSmall) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckZeroSizeBufferFails<zx_info_task_stats_t>(ZX_INFO_TASK_STATS, handle_provider)));
}

constexpr auto job_provider = []() -> const zx::job& {
  const static zx::unowned_job job = zx::job::default_job();
  return *job;
};

TEST(TaskGetInfoTest, InfoTaskStatsJobHandleIsBadHandle) {
  ASSERT_NO_FATAL_FAILURE(
      CheckWrongHandleTypeFails<zx_info_task_stats_t>(ZX_INFO_TASK_STATS, job_provider));
}

constexpr auto thread_provider = []() -> const zx::thread& {
  const static zx::unowned_thread thread = zx::thread::self();
  return *thread;
};

TEST(TaskGetInfoTest, InfoTaskStatsThreadHandleIsBadHandle) {
  ASSERT_NO_FATAL_FAILURE(
      CheckWrongHandleTypeFails<zx_info_task_stats_t>(ZX_INFO_TASK_STATS, thread_provider));
}

TEST(TaskGetInfoTest, InfoTaskRuntimeWrongType) {
  zx::event event;
  zx::event::create(0, &event);

  auto event_provider = [&]() -> const zx::event& { return event; };

  ASSERT_NO_FATAL_FAILURE(
      CheckWrongHandleTypeFails<zx_info_task_runtime_t>(ZX_INFO_TASK_RUNTIME, event_provider));
}

TEST(TaskGetInfoTest, InfoTaskRuntimeInvalidHandle) {
  ASSERT_NO_FATAL_FAILURE(
      CheckInvalidHandleFails<zx_info_task_runtime_t>(ZX_INFO_TASK_RUNTIME, thread_provider));
}

TEST(TaskGetInfoTest, SharedMemAccounting) {
  // First, verify we have access to the system resource to run this test.
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available, skipping\n");
  }
  // Create some some processes and VMOs with

  zx::process proc0;
  zx::vmar vmar0;
  static constexpr char kNameProc0[] = "proc0";
  ASSERT_OK(zx::process::create(*zx::job::default_job(), kNameProc0, sizeof(kNameProc0), 0, &proc0,
                                &vmar0));
  zx::process proc1;
  zx::vmar vmar1;
  static constexpr char kNameProc1[] = "proc1";
  ASSERT_OK(zx::process::create(*zx::job::default_job(), kNameProc1, sizeof(kNameProc1), 0, &proc1,
                                &vmar1));

  // With all the processes created apply a deadline memory priority to them all so that our memory
  // stats are predictable and will not change due to compression.
  ASSERT_NO_FATAL_FAILURE(SetDeadlineMemoryPriority(system_resource, vmar0));
  ASSERT_NO_FATAL_FAILURE(SetDeadlineMemoryPriority(system_resource, vmar1));

  const size_t kAnonSize = zx_system_get_page_size();
  zx::vmo private_anon;
  const char data = 'A';
  ASSERT_OK(zx::vmo::create(kAnonSize, 0, &private_anon));
  EXPECT_OK(private_anon.write(&data, 0, sizeof(data)));

  zx::vmo shared_anon;
  ASSERT_OK(zx::vmo::create(kAnonSize, 0, &shared_anon));
  EXPECT_OK(shared_anon.write(&data, 0, sizeof(data)));

  const size_t kCowSize = zx_system_get_page_size() * 3ul;
  zx::vmo shared_cow;
  ASSERT_OK(zx::vmo::create(kCowSize, 0, &shared_cow));
  EXPECT_OK(shared_cow.write(&data, 0, sizeof(data)));
  EXPECT_OK(shared_cow.write(&data, zx_system_get_page_size(), sizeof(data)));
  EXPECT_OK(shared_cow.write(&data, zx_system_get_page_size() * 2ul, sizeof(data)));
  zx::vmo private_cow;
  ASSERT_OK(shared_cow.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, kCowSize, &private_cow));
  // Create a private page in both the shared and private cow. Use distinct offsets to effectively
  // cause each to gain TWO private pages, one that they forked themselves, and one as a result of
  // the other forking.
  const size_t kCowSharedBytes = zx_system_get_page_size();
  const size_t kCowPrivateBytes = kCowSize - kCowSharedBytes;
  EXPECT_OK(shared_cow.write(&data, 0, sizeof(data)));
  EXPECT_OK(private_cow.write(&data, zx_system_get_page_size(), sizeof(data)));

  // Map the private anonymous and private cow into just proc0 and proc1 respectively
  zx_vaddr_t vaddr;
  EXPECT_OK(vmar0.map(ZX_VM_PERM_READ, 0u, private_anon, 0, kAnonSize, &vaddr));
  EXPECT_OK(vmar1.map(ZX_VM_PERM_READ, 0u, private_cow, 0, kCowSize, &vaddr));

  // Map the shared anonymous and cow into both proc0 and proc1
  EXPECT_OK(vmar0.map(ZX_VM_PERM_READ, 0u, shared_anon, 0, kAnonSize, &vaddr));
  EXPECT_OK(vmar1.map(ZX_VM_PERM_READ, 0u, shared_anon, 0, kAnonSize, &vaddr));
  EXPECT_OK(vmar0.map(ZX_VM_PERM_READ, 0u, shared_cow, 0, kCowSize, &vaddr));
  EXPECT_OK(vmar1.map(ZX_VM_PERM_READ, 0u, shared_cow, 0, kCowSize, &vaddr));

  // Query the stats and validate what we expect.
  size_t actual{};
  size_t avail{};
  zx_info_task_stats_t stats0{};
  EXPECT_OK(proc0.get_info(ZX_INFO_TASK_STATS, &stats0, sizeof(stats0), &actual, &avail));
  EXPECT_EQ(1, actual);
  EXPECT_EQ(1, avail);
  zx_info_task_stats_t stats1{};
  EXPECT_OK(proc1.get_info(ZX_INFO_TASK_STATS, &stats1, sizeof(stats1), &actual, &avail));
  EXPECT_EQ(1, actual);
  EXPECT_EQ(1, avail);

  // For mem_mapped_bytes it's just the total size of every mapping, and we have 2 anon and 1 cow
  // sized mappings.
  const size_t kProc0MappedBytes = kAnonSize * 2 + kCowSize;
  EXPECT_EQ(kProc0MappedBytes, stats0.mem_mapped_bytes);
  // Private bytes are only counted from private bytes of the non-shared mappings, which is just the
  // private anon, as private pages in the shared_cow are shared.
  const size_t kProc0PrivateBytes = kAnonSize;
  EXPECT_EQ(kProc0PrivateBytes, stats0.mem_private_bytes);
  // Shared bytes count from any shared mapping (regardless of whether those bytes are VMO private
  // or shared), as well as VMO shared pages from a private mapping.
  const size_t kProc0SharedBytes = kAnonSize + kCowSize;
  EXPECT_EQ(kProc0SharedBytes, stats0.mem_shared_bytes);
  // As our mappings are from VMOs that are fully populated, the shared and private bytes should
  // sum to the mapped bytes.
  EXPECT_EQ(kProc0MappedBytes, kProc0PrivateBytes + kProc0SharedBytes);
  // When scaling the shared bytes the VMO private pages are shared equally, however the VMO shared
  // pages (of which we can see 1) are first shared between the two VMOs, then shared between the
  // two processes.
  const size_t kProc0ScaledSharedBytes =
      (kAnonSize + kCowPrivateBytes) / 2 + kCowSharedBytes / 2 / 2;
  EXPECT_EQ(kProc0ScaledSharedBytes, stats0.mem_scaled_shared_bytes);

  // Proc1 has 1 anon mapping and 2 cow sized mappings.
  const size_t kProc1MappedBytes = kAnonSize + kCowSize * 2;
  EXPECT_EQ(kProc1MappedBytes, stats1.mem_mapped_bytes);
  // Only gain the private bytes of our private cow mapping.
  const size_t kProc1PrivateBytes = kCowPrivateBytes;
  EXPECT_EQ(kProc1PrivateBytes, stats1.mem_private_bytes);
  // Sharing the full shared anon and cow mappings, as well as the VMO shared page from our private
  // cow.
  const size_t kProc1SharedBytes = kAnonSize + kCowSize + kCowSharedBytes;
  EXPECT_EQ(kProc1SharedBytes, stats1.mem_shared_bytes);
  EXPECT_EQ(kProc1MappedBytes, kProc1PrivateBytes + kProc1SharedBytes);
  // Similar to proc1 starts with the same scaled shared bytes contribution as proc0 due to having
  // equivalent mappings, but then additionally gains the portions of the VMO shared bytes from
  // private_cow.
  const size_t kProc1ScaledSharedBytes = kProc0ScaledSharedBytes + kCowSharedBytes / 2;
  EXPECT_EQ(kProc1ScaledSharedBytes, stats1.mem_scaled_shared_bytes);

  // The sum of all private and scaled shared bytes should equal the actual memory usage of the
  // system.
  const size_t kTotalCommitted = kAnonSize * 2 + kCowSize + (zx_system_get_page_size() * 2ul);
  const size_t kTotalFromProcs =
      kProc0PrivateBytes + kProc0ScaledSharedBytes + kProc1PrivateBytes + kProc1ScaledSharedBytes;
  EXPECT_EQ(kTotalCommitted, kTotalFromProcs);
}

}  // namespace
}  // namespace object_info_test
