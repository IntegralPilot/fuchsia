// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/defer.h>
#include <lib/standalone-test/standalone.h>
#include <lib/zx/job.h>
#include <lib/zx/pager.h>
#include <lib/zx/profile.h>
#include <lib/zx/resource.h>
#include <lib/zx/result.h>
#include <lib/zx/thread.h>
#include <lib/zx/timer.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/profile.h>
#include <zircon/syscalls/resource.h>
#include <zircon/syscalls/types.h>
#include <zircon/time.h>

#include <atomic>
#include <thread>

#include <zxtest/zxtest.h>

namespace profile {
namespace {

zx::result<zx::resource> GetSystemProfileResource() {
  zx::resource system_profile_resource;
  const zx_status_t status =
      zx::resource::create(*standalone::GetSystemResource(), ZX_RSRC_KIND_SYSTEM,
                           ZX_RSRC_SYSTEM_PROFILE_BASE, 1, nullptr, 0, &system_profile_resource);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(system_profile_resource));
}

zx_profile_info_t MakeSchedulerProfileInfo(int32_t priority) {
  zx_profile_info_t info = {};
  info.flags = ZX_PROFILE_INFO_FLAG_PRIORITY;
  info.priority = priority;
  return info;
}

zx_profile_info_t MakeSchedulerProfileInfo(const zx_sched_deadline_params_t& params) {
  zx_profile_info_t info = {};
  info.flags = ZX_PROFILE_INFO_FLAG_DEADLINE;
  info.deadline_params = params;
  return info;
}

zx_profile_info_t MakeCpuMaskProfile(uint64_t mask) {
  zx_profile_info_t info = {};
  info.flags = ZX_PROFILE_INFO_FLAG_CPU_MASK;
  info.cpu_affinity_mask.mask[0] = mask;
  return info;
}

zx_profile_info_t MakeMemoryPriorityProfile(int32_t priority) {
  zx_profile_info_t info = {};
  info.flags = ZX_PROFILE_INFO_FLAG_MEMORY_PRIORITY;
  info.priority = priority;
  return info;
}

size_t GetCpuCount() {
  size_t actual, available;
  auto system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_INFO_BASE);
  ZX_ASSERT(result.status_value() == ZX_OK);
  zx::resource info_resource = std::move(result.value());

  zx_status_t status = info_resource.get_info(ZX_INFO_CPU_STATS, nullptr, 0, &actual, &available);
  ZX_ASSERT(status == ZX_OK);
  return available;
}

uint64_t GetAffinityMask(const zx::thread& thread) {
  zx_info_thread_t info;
  zx_status_t status = thread.get_info(ZX_INFO_THREAD, &info, sizeof(info), nullptr, nullptr);
  ZX_ASSERT(status == ZX_OK);
  return info.cpu_affinity_mask.mask[0];
}

uint32_t GetLastScheduledCpu(const zx::thread& thread) {
  zx_info_thread_stats_t info;
  zx_status_t status = thread.get_info(ZX_INFO_THREAD_STATS, &info, sizeof(info), nullptr, nullptr);
  ZX_ASSERT(status == ZX_OK);
  return info.last_scheduled_cpu;
}

// Tests in this file rely that the default job is the root job.
TEST(SchedulerProfileTest, CreateProfileWithDefaultPriorityIsOk) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  zx_profile_info_t profile_info = MakeSchedulerProfileInfo(ZX_PRIORITY_DEFAULT);
  zx::profile profile;

  ASSERT_OK(zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
}

TEST(SchedulerProfileTest, CreateProfileWithLowestPriorityIsOk) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  zx_profile_info_t profile_info = MakeSchedulerProfileInfo(ZX_PRIORITY_LOWEST);
  zx::profile profile;

  ASSERT_OK(zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
}

TEST(SchedulerProfileTest, CreateProfileWithLowPriorityIsOk) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  zx_profile_info_t profile_info = MakeSchedulerProfileInfo(ZX_PRIORITY_LOW);
  zx::profile profile;

  ASSERT_OK(zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
}

TEST(SchedulerProfileTest, CreateProfileWithHighPriorityIsOk) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  zx_profile_info_t profile_info = MakeSchedulerProfileInfo(ZX_PRIORITY_HIGH);
  zx::profile profile;

  ASSERT_OK(zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
}

TEST(SchedulerProfileTest, CreateProfileWithHighestPriorityIsOk) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  zx_profile_info_t profile_info = MakeSchedulerProfileInfo(ZX_PRIORITY_HIGHEST);
  zx::profile profile;

  ASSERT_OK(zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
}

TEST(SchedulerProfileTest, CreateFairProfileWithNoInheritIsOk) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  zx_profile_info_t profile_info = MakeSchedulerProfileInfo(ZX_PRIORITY_DEFAULT);
  zx::profile profile;

  profile_info.flags |= ZX_PROFILE_INFO_FLAG_NO_INHERIT;

  ASSERT_OK(zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
}

TEST(SchedulerProfileTest, CreateProfileWithPriorityExceedingHighestIsInvalidArgs) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  zx_profile_info_t profile_info = MakeSchedulerProfileInfo(ZX_PRIORITY_HIGHEST + 1);
  zx::profile profile;

  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
}

TEST(SchedulerProfileTest, CreateProfileWithPriorityBelowLowestIsInvalidArgs) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  zx_profile_info_t profile_info = MakeSchedulerProfileInfo(ZX_PRIORITY_LOWEST - 1);
  zx::profile profile;

  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
}

TEST(SchedulerProfileTest, CreateProfileWithDeadlineIsOk) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  zx_profile_info_t profile_info = MakeSchedulerProfileInfo({ZX_MSEC(1), ZX_MSEC(8), ZX_MSEC(10)});
  zx::profile profile;

  ASSERT_OK(zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
}

TEST(SchedulerProfileTest, CreateProfileWithCriticalDeadlineIsOk) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  zx_profile_info_t profile_info = MakeSchedulerProfileInfo({ZX_MSEC(1), ZX_MSEC(8), ZX_MSEC(10)});
  profile_info.flags |= ZX_PROFILE_INFO_FLAG_CRITICAL;
  zx::profile profile;

  ASSERT_OK(zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
}

TEST(SchedulerProfileTest, CreateProfileWithCriticalWithoutDeadlineIsInvalidArgs) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  zx_profile_info_t profile_info = MakeSchedulerProfileInfo(ZX_PRIORITY_DEFAULT);
  profile_info.flags |= ZX_PROFILE_INFO_FLAG_CRITICAL;
  zx::profile profile;

  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
}

TEST(SchedulerProfileTest, CreateProfileWithZeroCapacityIsInvalidArgs) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  zx_profile_info_t profile_info = MakeSchedulerProfileInfo({ZX_MSEC(0), ZX_MSEC(8), ZX_MSEC(10)});
  zx::profile profile;

  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
}

TEST(SchedulerProfileTest, CreateProfileWithDeadlineBelowCapacityIsInvalidArgs) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  zx_profile_info_t profile_info = MakeSchedulerProfileInfo({ZX_MSEC(8), ZX_MSEC(1), ZX_MSEC(10)});
  zx::profile profile;

  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
}

TEST(SchedulerProfileTest, CreateProfileWithPeriodBelowDeadlineIsInvalidArgs) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  zx_profile_info_t profile_info = MakeSchedulerProfileInfo({ZX_MSEC(8), ZX_MSEC(10), ZX_MSEC(1)});
  zx::profile profile;

  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
}

TEST(SchedulerProfileTest, CreateProfileOnNonProfileResourceIsAccessDenied) {
  zx_profile_info_t profile_info = MakeSchedulerProfileInfo(ZX_PRIORITY_DEFAULT);
  zx::profile profile;
  zx::resource power_resource;
  zx::resource::create(*standalone::GetSystemResource(), ZX_RSRC_KIND_SYSTEM,
                       ZX_RSRC_SYSTEM_POWER_BASE, 1, nullptr, 0, &power_resource);

  ASSERT_EQ(ZX_ERR_ACCESS_DENIED, zx::profile::create(power_resource, 0u, &profile_info, &profile));
}

TEST(SchedulerProfileTest, CreateProfileWithNonZeroOptionsIsInvalidArgs) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  zx_profile_info_t profile_info = MakeSchedulerProfileInfo(ZX_PRIORITY_DEFAULT);
  zx::profile profile;

  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx::profile::create(maybe_profile_rsrc.value(), 1u, &profile_info, &profile));
}

TEST(SchedulerProfileTest, CreateProfileWithDeadlineAndNoInheritIsInvalidArgs) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  zx_profile_info_t profile_info = MakeSchedulerProfileInfo({ZX_MSEC(1), ZX_MSEC(8), ZX_MSEC(10)});
  zx::profile profile;

  profile_info.flags |= ZX_PROFILE_INFO_FLAG_NO_INHERIT;

  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
}

TEST(SchedulerProfileTest, SetThreadPriorityIsOk) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());

  std::atomic<const char*> error = nullptr;
  std::atomic<zx_status_t> result = ZX_OK;

  zx::profile profile_1;
  zx_profile_info_t info_1 = MakeSchedulerProfileInfo(ZX_PRIORITY_LOWEST);
  ASSERT_OK(zx::profile::create(maybe_profile_rsrc.value(), 0u, &info_1, &profile_1));

  zx::profile profile_2;
  zx_profile_info_t info_2 = MakeSchedulerProfileInfo(ZX_PRIORITY_HIGH);
  ASSERT_OK(zx::profile::create(maybe_profile_rsrc.value(), 0u, &info_2, &profile_2));

  zx::profile profile_3;
  zx_profile_info_t info_3 = MakeSchedulerProfileInfo({ZX_MSEC(8), ZX_MSEC(16), ZX_MSEC(16)});
  ASSERT_OK(zx::profile::create(maybe_profile_rsrc.value(), 0u, &info_3, &profile_3));

  // Operate on a background thread, just in case a failure changes the priority of the main
  // thread.
  std::thread worker(
      [](zx::profile first, zx::profile second, zx::profile third, std::atomic<const char*>* error,
         std::atomic<zx_status_t>* result) {
        *result = zx::thread::self()->set_profile(first, 0);
        if (*result != ZX_OK) {
          *error = "Failed to set first profile on thread";
          return;
        }
        std::this_thread::yield();

        *result = zx::thread::self()->set_profile(second, 0);
        if (*result != ZX_OK) {
          *error = "Failed to set second profile on thread";
          return;
        }
        std::this_thread::yield();

        *result = zx::thread::self()->set_profile(third, 0);
        if (*result != ZX_OK) {
          *error = "Failed to set third profile on thread";
          return;
        }
      },
      std::move(profile_1), std::move(profile_2), std::move(profile_3), &error, &result);

  // Wait until is completed.
  worker.join();

  ASSERT_OK(result.load(), "%s", error.load());
}

TEST(SchedulerProfileTest, SetProfileWithNonZeroOptionsIsInvalidArgs) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());

  zx::profile profile;
  zx_profile_info_t info = MakeSchedulerProfileInfo(ZX_PRIORITY_LOWEST);
  ASSERT_OK(zx::profile::create(maybe_profile_rsrc.value(), 0u, &info, &profile));

  ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx::thread::self()->set_profile(profile, 1u));
}

TEST(ProfileTest, CreateProfileWithDefaultInitializedProfileInfoIsError) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  zx_profile_info_t profile_info = {};
  zx::profile profile;

  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
}

TEST(ProfileTest, CreateProfileWithMutuallyExclusiveFlagsIsInvalidArgs) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  zx_profile_info_t profile_info = {};
  profile_info.flags = ZX_PROFILE_INFO_FLAG_PRIORITY | ZX_PROFILE_INFO_FLAG_DEADLINE;
  zx::profile profile;

  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
}

TEST(ProfileTest, CreateProfileWithNoProfileInfoIsInvalidArgs) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  zx::profile profile;

  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx::profile::create(maybe_profile_rsrc.value(), 0u, nullptr, &profile));
}

TEST(ProfileTest, CreateProfileWithInvalidHandleIsBadHandle) {
  zx::profile profile;

  ASSERT_EQ(ZX_ERR_BAD_HANDLE,
            zx::profile::create(zx::resource(ZX_HANDLE_INVALID), 0u, nullptr, &profile));
}

zx_status_t RunThreadWithProfile(const zx::profile& profile,
                                 const std::function<zx_status_t()>& body) {
  zx_status_t result;
  std::thread worker([&body, &result, &profile]() {
    result = zx::thread::self()->set_profile(profile, 0);
    if (result != ZX_OK) {
      return;
    }
    result = body();
  });
  worker.join();
  return result;
}

TEST(CpuMaskProfile, EmptyMaskIsValid) {
  zx::profile profile;
  zx_profile_info_t profile_info = MakeCpuMaskProfile(0);
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  ASSERT_OK(zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));

  // Ensure that the thread can still run, despite the affinity mask
  // having no valid CPUs in it. (The kernel will just fall back to
  // its own choice of CPUs if this mask can't be respected.)
  ASSERT_OK(RunThreadWithProfile(profile, []() {
    EXPECT_EQ(GetAffinityMask(*zx::thread::self()), 0);
    EXPECT_NE(GetLastScheduledCpu(*zx::thread::self()), ZX_INFO_INVALID_CPU);
    return ZX_OK;
  }));
}

TEST(CpuMaskProfile, ApplyProfile) {
  const size_t num_cpus = GetCpuCount();
  ASSERT_LT(num_cpus, ZX_CPU_SET_BITS_PER_WORD,
            "Test assumes system running with less than %d cores.", ZX_CPU_SET_BITS_PER_WORD);
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  for (size_t i = 0; i < num_cpus; i++) {
    zx_profile_info_t profile_info = MakeCpuMaskProfile(1 << i);
    zx::profile profile;
    ASSERT_OK(zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));

    // Ensure that the correct mask was applied.
    ASSERT_OK(RunThreadWithProfile(profile, [i]() {
      EXPECT_EQ(GetAffinityMask(*zx::thread::self()), (1 << i));
      EXPECT_EQ(GetLastScheduledCpu(*zx::thread::self()), i);
      return ZX_OK;
    }));
  }
}

TEST(MemoryPriorityProfile, InvalidPriorities) {
  constexpr int32_t kBadPriorities[] = {ZX_PRIORITY_LOWEST, ZX_PRIORITY_LOW, ZX_PRIORITY_HIGHEST};
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  for (const int32_t prio : kBadPriorities) {
    zx::profile profile;

    zx_profile_info_t profile_info = MakeMemoryPriorityProfile(prio);
    EXPECT_EQ(ZX_ERR_INVALID_ARGS,
              zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
  }
}

TEST(MemoryPriorityProfile, MemoryOrThread) {
  const uint32_t kInvalidWith[] = {ZX_PROFILE_INFO_FLAG_PRIORITY, ZX_PROFILE_INFO_FLAG_CPU_MASK,
                                   ZX_PROFILE_INFO_FLAG_DEADLINE};
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());

  for (const uint32_t invalid_with : kInvalidWith) {
    zx_profile_info_t profile_info = {};
    profile_info.flags = ZX_PROFILE_INFO_FLAG_MEMORY_PRIORITY | invalid_with;
    zx::profile profile;

    ASSERT_EQ(ZX_ERR_INVALID_ARGS,
              zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
  }
}

TEST(MemoryPriorityProfile, ApplyProfile) {
  // Create the two profiles we will need.
  zx::profile profile_high, profile_default;
  zx_profile_info_t profile_info = MakeMemoryPriorityProfile(ZX_PRIORITY_HIGH);
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  ASSERT_OK(zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile_high));
  profile_info = MakeMemoryPriorityProfile(ZX_PRIORITY_DEFAULT);
  ASSERT_OK(zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile_default));

  // To ensure there are some candidate reclaimable pages, create and map in a pager backed VMO.
  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));
  zx::pager pager;
  ASSERT_OK(zx::pager::create(0, &pager));
  zx::vmo pager_vmo;
  ASSERT_OK(pager.create_vmo(0, port, 0, zx_system_get_page_size(), &pager_vmo));
  zx_vaddr_t addr;
  ASSERT_OK(zx::vmar::root_self()->map(0, 0, pager_vmo, 0, zx_system_get_page_size(), &addr));
  auto unmap = fit::defer([&addr]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(addr, zx_system_get_page_size());
  });

  // Helper to supply pages to the VMO. Since these pages are reclaimable there is a small chance
  // that they get evicted during the execution of the test so we re-supply at a few different
  // points.
  auto supply = [&]() {
    zx::vmo aux_vmo;
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &aux_vmo));
    uint64_t val = 42;
    EXPECT_OK(aux_vmo.write(&val, 0, sizeof(val)));
    EXPECT_OK(pager.supply_pages(pager_vmo, 0, zx_system_get_page_size(), aux_vmo, 0));
  };

  // Start with the pages supplied so that they are there for the initial query.
  supply();

  zx_info_kmem_stats_extended_t stats;
  auto system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_INFO_BASE);
  ASSERT_OK(result.status_value());
  zx::resource info_resource = std::move(result.value());

  EXPECT_OK(
      info_resource.get_info(ZX_INFO_KMEM_STATS_EXTENDED, &stats, sizeof(stats), nullptr, nullptr));

  const uint64_t prev = stats.vmo_reclaim_disabled_bytes;

  EXPECT_OK(zx::vmar::root_self()->set_profile(profile_high, 0));

  // Applying the profile should have caused our pages to no longer be reclaimable. The pager backed
  // VMO we mapped means we know it's definitely non-zero, but we do not know if there are more.
  // In between the previous supply and setting the profile, the pages could have been evicted, so
  // re-supply them.
  supply();
  EXPECT_OK(
      info_resource.get_info(ZX_INFO_KMEM_STATS_EXTENDED, &stats, sizeof(stats), nullptr, nullptr));
  EXPECT_GT(stats.vmo_reclaim_disabled_bytes, prev);

  // Applying the default priority should undo the reclamation change.
  EXPECT_OK(zx::vmar::root_self()->set_profile(profile_default, 0));

  EXPECT_OK(
      info_resource.get_info(ZX_INFO_KMEM_STATS_EXTENDED, &stats, sizeof(stats), nullptr, nullptr));
  EXPECT_EQ(stats.vmo_reclaim_disabled_bytes, prev);
}

TEST(MemoryPriorityProfile, Rights) {
  zx::profile profile;
  zx_profile_info_t profile_info = MakeMemoryPriorityProfile(ZX_PRIORITY_DEFAULT);
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());
  ASSERT_OK(zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));

  // Duplicate the vmar handle to have valid and invalid permissions.
  zx::vmar vmar_valid, vmar_invalid;
  ASSERT_OK(zx::vmar::root_self()->duplicate(ZX_RIGHT_OP_CHILDREN, &vmar_valid));
  ASSERT_OK(zx::vmar::root_self()->duplicate(0, &vmar_invalid));

  EXPECT_OK(vmar_valid.set_profile(profile, 0));
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, vmar_invalid.set_profile(profile, 0));
}

TEST(SchedulerProfileTest, CreateProfileWithDeadlineCapacityOutOfRangeIsOutOfRange) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());

  constexpr int64_t kOutOfRangeDuration = static_cast<int64_t>(INT32_MAX) + 1;

  zx_profile_info_t profile_info = {};
  profile_info.flags = ZX_PROFILE_INFO_FLAG_DEADLINE;
  profile_info.deadline_params.capacity = kOutOfRangeDuration;
  profile_info.deadline_params.relative_deadline = kOutOfRangeDuration + 1;
  profile_info.deadline_params.period = kOutOfRangeDuration + 2;
  zx::profile profile;
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
}

TEST(SchedulerProfileTest, CreateProfileWithDeadlineRelativeDeadlineOutOfRangeIsOutOfRange) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());

  constexpr int64_t kOutOfRangeDuration = static_cast<int64_t>(INT32_MAX) + 1;

  zx_profile_info_t profile_info = {};
  profile_info.flags = ZX_PROFILE_INFO_FLAG_DEADLINE;
  profile_info.deadline_params.capacity = 10;
  profile_info.deadline_params.relative_deadline = kOutOfRangeDuration;
  profile_info.deadline_params.period = kOutOfRangeDuration + 1;
  zx::profile profile;
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
}

TEST(SchedulerProfileTest, CreateProfileWithDeadlinePeriodOutOfRangeIsOutOfRange) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());

  constexpr int64_t kOutOfRangeDuration = static_cast<int64_t>(INT32_MAX) + 1;

  zx_profile_info_t profile_info = {};
  profile_info.flags = ZX_PROFILE_INFO_FLAG_DEADLINE;
  profile_info.deadline_params.capacity = 10;
  profile_info.deadline_params.relative_deadline = 20;
  profile_info.deadline_params.period = kOutOfRangeDuration;
  zx::profile profile;
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));
}

TEST(ProfileTest, ApplySchedulingProfileToVmarIsOk) {
  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());

  zx_profile_info_t profile_info = {};
  profile_info.flags = ZX_PROFILE_INFO_FLAG_PRIORITY;
  profile_info.priority = 10;
  zx::profile profile;
  ASSERT_OK(zx::profile::create(maybe_profile_rsrc.value(), 0u, &profile_info, &profile));

  // Applying scheduling profile to VMAR should succeed (doing nothing)
  EXPECT_OK(zx::vmar::root_self()->set_profile(profile, 0));
}

// Regression test for b/511528549: verifies that remote priority inheritance propagation on an
// idle CPU does not cause an assertion failure in energy statistics calculation due to mismatched
// per-CPU idle time attribution.
TEST(SchedulerProfileTest, DeadlinePiUtilizationPanicRepro) {
  if (GetCpuCount() < 2) {
    ZXTEST_SKIP("At least 2 CPUs are required for this test");
  }

  zx::result<zx::resource> maybe_profile_rsrc = GetSystemProfileResource();
  ASSERT_OK(maybe_profile_rsrc.status_value());

  zx::profile prof_dl;
  zx_profile_info_t pinfo = MakeSchedulerProfileInfo({ZX_MSEC(2), ZX_MSEC(3), ZX_MSEC(3)});
  ASSERT_OK(zx::profile::create(maybe_profile_rsrc.value(), 0u, &pinfo, &prof_dl));

  zx::channel ch1, ch2;
  ASSERT_OK(zx::channel::create(0, &ch1, &ch2));

  zx_futex_t futex1 = 1;
  std::atomic<bool> ready = false;
  std::atomic<bool> done = false;
  std::atomic<bool> failed = false;

  zx::thread main_thread_handle;
  ASSERT_OK(zx::thread::self()->duplicate(ZX_RIGHT_SAME_RIGHTS, &main_thread_handle));

  auto t1_func = [&]() {
    if (zx_status_t status = zx::thread::self()->set_profile(prof_dl, 0); status != ZX_OK) {
      failed = true;
      EXPECT_OK(status);
      return;
    }
    while (!ready && !failed) {
      zx::nanosleep(zx::deadline_after(zx::usec(1)));
    }
    while (!done && !failed) {
      zx_futex_wait(&futex1, 1, main_thread_handle.get(), zx_deadline_after(ZX_USEC(10)));
    }
  };

  std::thread t1(t1_func);
  std::thread t3(t1_func);

  std::thread t2([&]() {
    while (!ready && !failed) {
      zx::nanosleep(zx::deadline_after(zx::usec(1)));
    }
    char msg[16];
    uint32_t act_bytes, act_handles;
    while (!done && !failed) {
      if (ch2.wait_one(ZX_CHANNEL_READABLE, zx::deadline_after(zx::msec(10)), nullptr) == ZX_OK) {
        if (ch2.read(0, msg, nullptr, sizeof(msg), 0, &act_bytes, &act_handles) == ZX_OK) {
          ch2.write(0, msg, 8, nullptr, 0);
        }
      }
    }
  });

  zx::nanosleep(zx::deadline_after(zx::msec(10)));
  ready = true;

  char tx_msg[8] = "msg";
  char rx_msg[8] = {0};
  zx_channel_call_args_t args = {
      .wr_bytes = tx_msg,
      .wr_handles = nullptr,
      .rd_bytes = rx_msg,
      .rd_handles = nullptr,
      .wr_num_bytes = 8,
      .wr_num_handles = 0,
      .rd_num_bytes = 8,
      .rd_num_handles = 0,
  };

  uint32_t act_bytes, act_handles;
  for (int i = 0; i < 50000 && !done && !failed; i++) {
    ch1.call(0, zx::deadline_after(zx::msec(50)), &args, &act_bytes, &act_handles);
    if (i % 50 == 0) {
      zx::nanosleep(zx::deadline_after(zx::usec(1)));
    }
  }

  done = true;
  zx_futex_wake(&futex1, 0xFFFFFFFF);
  t1.join();
  t3.join();
  t2.join();

  EXPECT_FALSE(failed.load());
}

}  // namespace
}  // namespace profile
