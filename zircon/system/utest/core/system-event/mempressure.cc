// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/maybe-standalone-test/maybe-standalone.h>
#include <lib/zx/event.h>
#include <lib/zx/job.h>

#include <zxtest/zxtest.h>

// Tests in this file rely that the default job is the root job.

static void retrieve_mem_event(zx_system_event_type_t event_type) {
  zx::event mem_event;

  ASSERT_EQ(zx_system_get_event(ZX_HANDLE_INVALID, event_type, mem_event.reset_and_get_address()),
            ZX_ERR_BAD_HANDLE, "cannot get with invalid root job");

  ASSERT_EQ(zx_system_get_event(zx_process_self(), event_type, mem_event.reset_and_get_address()),
            ZX_ERR_WRONG_TYPE, "cannot get without a job handle");

  zx::job tmp_job;
  ASSERT_EQ(zx::job::create(*zx::job::default_job(), 0u, &tmp_job), ZX_OK, "helper sub job");

  ASSERT_EQ(zx_system_get_event(tmp_job.get(), event_type, mem_event.reset_and_get_address()),
            ZX_ERR_ACCESS_DENIED, "cannot get without correct root job");

  zx::unowned_job root_job(zx_job_default());
  if (!root_job->is_valid()) {
    printf("no root job. skipping part of test\n");
  } else {
    ASSERT_EQ(zx_system_get_event(root_job->get(), ~0u, mem_event.reset_and_get_address()),
              ZX_ERR_INVALID_ARGS, "incorrect kind value does not retrieve");

    ASSERT_EQ(zx_system_get_event(root_job->get(), event_type, mem_event.reset_and_get_address()),
              ZX_OK, "can get if root provided");

    // Confirm we at least got an event.
    zx_info_handle_basic_t info;
    ASSERT_EQ(zx_object_get_info(mem_event.get(), ZX_INFO_HANDLE_BASIC, &info, sizeof(info),
                                 nullptr, nullptr),
              ZX_OK, "object_get_info");
    EXPECT_NE(info.koid, 0, "no koid");
    EXPECT_EQ(info.type, ZX_OBJ_TYPE_EVENT, "incorrect type");
    EXPECT_EQ(info.rights, ZX_DEFAULT_SYSTEM_EVENT_LOW_MEMORY_RIGHTS, "incorrect rights");
  }
}

static void signal_mem_event_from_userspace(zx_system_event_type_t event_type) {
  zx::unowned_job root_job(zx_job_default());
  if (!root_job->is_valid()) {
    printf("no root job. skipping test\n");
  } else {
    zx::event mem_event;
    ASSERT_EQ(zx_system_get_event(root_job->get(), event_type, mem_event.reset_and_get_address()),
              ZX_OK);

    ASSERT_EQ(mem_event.signal(0, 1), ZX_ERR_ACCESS_DENIED, "shouldn't be able to signal");
  }
}

TEST(SystemEvent, RetrieveOom) { retrieve_mem_event(ZX_SYSTEM_EVENT_OUT_OF_MEMORY); }

TEST(SystemEvent, CannotSignalOomFromUserspace) {
  signal_mem_event_from_userspace(ZX_SYSTEM_EVENT_OUT_OF_MEMORY);
}

TEST(SystemEvent, RetrieveImminentOom) {
  retrieve_mem_event(ZX_SYSTEM_EVENT_IMMINENT_OUT_OF_MEMORY);
}

TEST(SystemEvent, CannotSignalImminentOomFromUserspace) {
  signal_mem_event_from_userspace(ZX_SYSTEM_EVENT_IMMINENT_OUT_OF_MEMORY);
}

TEST(SystemEvent, RetrieveMempressureCritical) {
  retrieve_mem_event(ZX_SYSTEM_EVENT_MEMORY_PRESSURE_CRITICAL);
}

TEST(SystemEvent, CannotSignalMempressureCriticalFromUserspace) {
  signal_mem_event_from_userspace(ZX_SYSTEM_EVENT_MEMORY_PRESSURE_CRITICAL);
}

TEST(SystemEvent, RetreiveMempressureWarning) {
  retrieve_mem_event(ZX_SYSTEM_EVENT_MEMORY_PRESSURE_WARNING);
}

TEST(SystemEvent, CannotSignalMempressureWarningFromUserspace) {
  signal_mem_event_from_userspace(ZX_SYSTEM_EVENT_MEMORY_PRESSURE_WARNING);
}

TEST(SystemEvent, RetrieveMempressureNormal) {
  retrieve_mem_event(ZX_SYSTEM_EVENT_MEMORY_PRESSURE_NORMAL);
}

TEST(SystemEvent, CannotSignalMempressureNormalFromUserspace) {
  signal_mem_event_from_userspace(ZX_SYSTEM_EVENT_MEMORY_PRESSURE_NORMAL);
}

TEST(SystemEvent, ExactlyOneMemoryEventSignaled) {
  zx::unowned_job root_job(zx_job_default());
  if (!root_job->is_valid()) {
    printf("no root job. skipping part of test\n");
    return;
  }

  const int kNumEvents = 5;
  zx::event mem_events[kNumEvents];
  zx_wait_item_t wait_items[kNumEvents];
  zx_system_event_type_t types[kNumEvents] = {
      ZX_SYSTEM_EVENT_OUT_OF_MEMORY, ZX_SYSTEM_EVENT_IMMINENT_OUT_OF_MEMORY,
      ZX_SYSTEM_EVENT_MEMORY_PRESSURE_CRITICAL, ZX_SYSTEM_EVENT_MEMORY_PRESSURE_WARNING,
      ZX_SYSTEM_EVENT_MEMORY_PRESSURE_NORMAL};

  for (int i = 0; i < kNumEvents; i++) {
    ASSERT_EQ(zx_system_get_event(root_job->get(), types[i], mem_events[i].reset_and_get_address()),
              ZX_OK, "can get if root provided");
    wait_items[i].handle = mem_events[i].get();
    wait_items[i].waitfor = ZX_EVENT_SIGNALED;
    wait_items[i].pending = 0;
  }

  // This wait should return immediately.
  ASSERT_EQ(zx_object_wait_many(wait_items, kNumEvents, ZX_TIME_INFINITE), ZX_OK,
            "wait on memory events");

  int events_signaled = 0;
  for (auto& wait_item : wait_items) {
    if (wait_item.pending) {
      events_signaled++;
    }
  }

  ASSERT_EQ(events_signaled, 1, "exactly one memory event signaled");
}

// Creates a 1 MiB discardable VMO filled with random data.
static void CreateDiscardableVmo(zx::vmo& vmo) {
  ASSERT_OK(zx::vmo::create(1024ul * 1024, ZX_VMO_DISCARDABLE, &vmo));

  uint64_t size;
  ASSERT_OK(vmo.get_size(&size));

  zx_vmo_lock_state_t lock_state;
  ASSERT_OK(vmo.op_range(ZX_VMO_OP_LOCK, 0, size, &lock_state, sizeof(lock_state)));

  zx_vaddr_t addr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_ALLOW_FAULTS, 0,
                                       vmo, 0, size, &addr));
  zx_cprng_draw(reinterpret_cast<void*>(addr), size);
  ASSERT_OK(zx::vmar::root_self()->unmap(addr, size));

  ASSERT_OK(vmo.op_range(ZX_VMO_OP_UNLOCK, 0, size, nullptr, 0));
}

// TODO(https://fxbug.dev/513577268): Test is disabled.  See bug for details.
TEST(SystemEvent, DISABLED_MemoryStallEvent) {
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    printf("System resource not available, skipping\n");
    return;
  }

  // Retrieve the total amount of memory in the system.
  zx::result<zx::resource> info_resource =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_INFO_BASE);
  ASSERT_OK(info_resource.status_value());
  zx_info_kmem_stats_t kmem_stats;
  ASSERT_OK(info_resource->get_info(ZX_INFO_KMEM_STATS, &kmem_stats, sizeof(kmem_stats), nullptr,
                                    nullptr));
  size_t total_bytes_mb = kmem_stats.total_bytes / (1024ul * 1024);

  zx::result<zx::resource> stall_resource =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_STALL_BASE);
  ASSERT_OK(stall_resource.status_value());

  // Start observing memory pressure with a very low threshold.
  zx::event stall_event;
  zx_status_t status =
      zx_system_watch_memory_stall(stall_resource->get(), ZX_SYSTEM_MEMORY_STALL_SOME, 1,
                                   ZX_MSEC(100), stall_event.reset_and_get_address());
  ASSERT_OK(status);

  // Keep hoarding discardable VMOs until a stall is reported (=success) or we hit a safety limit
  // (=failure).
  size_t safety_limit_vmo_count =
      total_bytes_mb + total_bytes_mb / 20;  // 105% the total amount of memory
  std::vector<zx::vmo> vmos;
  zx_signals_t pending = 0;
  while ((pending & ZX_EVENT_SIGNALED) == 0) {
    status = stall_event.wait_one(0, zx::time::infinite_past(), &pending);
    ASSERT_EQ(ZX_ERR_TIMED_OUT, status);

    ASSERT_LE(vmos.size(), safety_limit_vmo_count, "Safety limit was hit, giving up");

    zx::vmo vmo;
    ASSERT_NO_FATAL_FAILURE(CreateDiscardableVmo(vmo));
    vmos.push_back(std::move(vmo));
  }

  printf("Got stall signal with VMO # %zu\n", vmos.size());
}

TEST(SystemEvent, CannotSignalMemoryStallEventFromUserspace) {
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    printf("System resource not available, skipping\n");
    return;
  }

  zx::result<zx::resource> stall_resource =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_STALL_BASE);
  ASSERT_OK(stall_resource.status_value());

  // Obtain a memory stall event.
  zx::event stall_event;
  zx_status_t status =
      zx_system_watch_memory_stall(stall_resource->get(), ZX_SYSTEM_MEMORY_STALL_SOME, 1,
                                   ZX_MSEC(100), stall_event.reset_and_get_address());
  ASSERT_OK(status);

  ASSERT_EQ(ZX_ERR_ACCESS_DENIED, stall_event.signal(0, ZX_EVENT_SIGNALED),
            "shouldn't be able to signal");
}

TEST(SystemEvent, MemoryStallThresholds) {
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    printf("System resource not available, skipping\n");
    return;
  }

  zx::result<zx::resource> stall_resource =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_STALL_BASE);
  ASSERT_OK(stall_resource.status_value());

  zx::event stall_event;

  // Verify we can request a FULL stall event.
  ASSERT_OK(zx_system_watch_memory_stall(stall_resource->get(), ZX_SYSTEM_MEMORY_STALL_FULL,
                                         ZX_MSEC(100), ZX_MSEC(100),
                                         stall_event.reset_and_get_address()));

  // Verify we cannot request an invalid stall kind.
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_system_watch_memory_stall(
                stall_resource->get(), static_cast<zx_system_memory_stall_type_t>(999),
                ZX_MSEC(100), ZX_MSEC(100), stall_event.reset_and_get_address()));

  // Verify we can request a threshold that is equal to the window, but not greater than that.
  ASSERT_OK(zx_system_watch_memory_stall(stall_resource->get(), ZX_SYSTEM_MEMORY_STALL_SOME,
                                         ZX_MSEC(100), ZX_MSEC(100),
                                         stall_event.reset_and_get_address()));
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_system_watch_memory_stall(stall_resource->get(), ZX_SYSTEM_MEMORY_STALL_SOME,
                                         ZX_MSEC(100) + 1, ZX_MSEC(100),
                                         stall_event.reset_and_get_address()));

  // Verify that we cannot request a threshold less than or equal to zero.
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_system_watch_memory_stall(stall_resource->get(), ZX_SYSTEM_MEMORY_STALL_SOME, 0,
                                         ZX_MSEC(100), stall_event.reset_and_get_address()));
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_system_watch_memory_stall(stall_resource->get(), ZX_SYSTEM_MEMORY_STALL_SOME, -1,
                                         ZX_MSEC(100), stall_event.reset_and_get_address()));

  // Verify we cannot request a zero or negative window.
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_system_watch_memory_stall(stall_resource->get(), ZX_SYSTEM_MEMORY_STALL_SOME, 0, 0,
                                         stall_event.reset_and_get_address()));
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_system_watch_memory_stall(stall_resource->get(), ZX_SYSTEM_MEMORY_STALL_SOME, -1, -1,
                                         stall_event.reset_and_get_address()));

  // Verify that we can request a window of 10 seconds, but no more.
  ASSERT_OK(zx_system_watch_memory_stall(stall_resource->get(), ZX_SYSTEM_MEMORY_STALL_SOME,
                                         ZX_MSEC(100), ZX_SEC(10),
                                         stall_event.reset_and_get_address()));
  ASSERT_EQ(
      ZX_ERR_INVALID_ARGS,
      zx_system_watch_memory_stall(stall_resource->get(), ZX_SYSTEM_MEMORY_STALL_SOME, ZX_MSEC(100),
                                   ZX_SEC(10) + 1, stall_event.reset_and_get_address()));
}
