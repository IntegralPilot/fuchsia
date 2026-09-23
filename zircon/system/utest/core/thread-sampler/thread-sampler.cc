// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/function.h>
#include <lib/fxt/fields.h>
#include <lib/standalone-test/standalone.h>
#include <lib/zx/event.h>
#include <lib/zx/thread.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <zircon/errors.h>
#include <zircon/limits.h>
#include <zircon/process.h>
#include <zircon/rights.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/object.h>
#include <zircon/threads.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <thread>

#include <zxtest/zxtest.h>

#include "../threads/test-thread.h"
#include "../threads/thread-functions/thread-functions.h"

namespace {

zx_koid_t GetTid(zx_handle_t thread) {
  zx_info_handle_basic_t info;
  size_t actual = 0;
  size_t avail = 0;
  if (zx_object_get_info(thread, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), &actual, &avail) !=
      ZX_OK) {
    return ZX_KOID_INVALID;
  }
  ZX_ASSERT(actual == 1);
  ZX_ASSERT(avail == 1);
  return info.koid;
}

void TestFn(zx::unowned_event event) {
  event->signal(0u, ZX_USER_SIGNAL_0);
  for (;;) {
    zx_status_t wait_result = event->wait_one(ZX_USER_SIGNAL_1, zx::time::infinite_past(), nullptr);
    if (wait_result != ZX_ERR_TIMED_OUT) {
      break;
    }
  }
}

// Call f for each record read from the sampler.
zx::result<> ForEachRecord(zx_handle_t sampler, fit::function<void(std::span<uint64_t>)> f) {
  size_t max_size;
  if (zx_status_t status = zx_sampler_read(sampler, nullptr, 0, &max_size); status != ZX_OK) {
    return zx::error(status);
  }

  size_t actual;
  std::vector<uint64_t> data(max_size / 8);
  if (zx_status_t status =
          zx_sampler_read(sampler, data.data(), data.size() * sizeof(uint64_t), &actual);
      status != ZX_OK) {
    return zx::error(status);
  }

  size_t offset = 0;
  const size_t actual_words = std::min(actual / sizeof(uint64_t), data.size());
  while (offset < actual_words) {
    uint64_t* header = data.data() + offset;
    if (*header == 0) {
      break;
    }
    size_t record_words = fxt::RecordFields::RecordSize::Get<size_t>(*header);
    if (record_words == 0 || record_words > actual_words - offset) {
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }
    std::span<uint64_t> record{header, record_words};
    f(record);
    offset += record_words;
  }
  return zx::ok();
}

zx::result<> ReadNRecords(zx_handle_t sampler, size_t N) {
  size_t record_count{0};
  zx::time deadline = zx::deadline_after(zx::sec(30));
  while (zx::clock::get_monotonic() < deadline) {
    if (zx::result res =
            ForEachRecord(sampler, [&record_count](std::span<uint64_t>) { record_count += 1; });
        res.is_error()) {
      return res.take_error();
    }
    if (record_count >= N) {
      return zx::ok();
    }
  }
  return zx::error(ZX_ERR_TIMED_OUT);
}

zx::result<> ReadNRecordsContainingTid(zx_handle_t sampler, zx_koid_t desired_tid, size_t N) {
  size_t record_count{0};
  zx::time deadline = zx::deadline_after(zx::sec(30));
  auto f = [&record_count, desired_tid](std::span<uint64_t> record_data) {
    ZX_ASSERT(record_data.size() >= 4);
    ZX_ASSERT(fxt::RecordFields::Type::Get<size_t>(record_data[0]) ==
              static_cast<size_t>(fxt::RecordType::kProfiler));
    // Record format looks like
    // 0-7  : header
    // 8-15 : ts
    // 16-23: pid
    // 24-31: tid
    zx_koid_t tid = record_data[3];
    if (tid == desired_tid) {
      record_count += 1;
    }
  };
  while (zx::clock::get_monotonic() < deadline) {
    if (zx::result res = ForEachRecord(sampler, f); res.is_error()) {
      return res.take_error();
    }
    if (record_count >= N) {
      return zx::ok();
    }
  }
  return zx::error(ZX_ERR_TIMED_OUT);
}

TEST(ThreadSampler, StartStop) {
  // Start the thread sampler on a thread, wait for some time while taking samples, check to see
  // that samples were written.
  size_t buffer_size = zx_system_get_page_size();
  zx_sampler_config_t config{
      .period = zx::msec(1).get(),
      .buffer_size = buffer_size,
  };
  zx_handle_t sampler;

  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_SAMPLING_BASE);
  ASSERT_OK(result.status_value());
  zx::resource sampling_resource = std::move(result.value());

  ASSERT_OK(zx_sampler_create(sampling_resource.get(), 0, &config, &sampler));

  zx::event event;
  ASSERT_EQ(zx::event::create(0, &event), ZX_OK);

  // Create a thread
  std::thread sample_thread{TestFn, event.borrow()};
  zx_handle_t native_handle = native_thread_get_zx_handle(sample_thread.native_handle());

  ASSERT_OK(event.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite(), nullptr));
  ASSERT_OK(zx_sampler_start(sampler));

  zx_koid_t tid = GetTid(native_handle);
  ASSERT_NE(tid, ZX_KOID_INVALID);

  ASSERT_OK(ReadNRecordsContainingTid(sampler, tid, 10).status_value());
  ASSERT_OK(zx_sampler_stop(sampler));
  ASSERT_OK(event.signal(0, ZX_USER_SIGNAL_1));
  sample_thread.join();
  ASSERT_OK(zx_handle_close(sampler));
}

TEST(ThreadSampler, SamplerLifetime) {
  // Once a sampler is created, another sampler should not be able to be created until the returned
  // buffer is release
  size_t buffer_size = zx_system_get_page_size();
  zx_sampler_config_t config{
      .period = zx::msec(1).get(),
      .buffer_size = buffer_size,
  };

  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_SAMPLING_BASE);
  ASSERT_OK(result.status_value());
  zx::resource sampling_resource = std::move(result.value());

  {
    zx_handle_t sampler;
    ASSERT_OK(zx_sampler_create(sampling_resource.get(), 0, &config, &sampler));

    zx_handle_t new_sampler;
    zx_status_t create_res_bad =
        zx_sampler_create(sampling_resource.get(), 0, &config, &new_sampler);

    EXPECT_EQ(create_res_bad, ZX_ERR_ALREADY_EXISTS);
    ASSERT_OK(zx_handle_close(sampler));
  }

  // Once the buffer is released, a new sampler can now be created
  zx_handle_t sampler;
  ASSERT_OK(zx_sampler_create(sampling_resource.get(), 0, &config, &sampler));
  ASSERT_OK(zx_handle_close(sampler));
}

TEST(ThreadSampler, InvalidOptions) {
  size_t buffer_size = zx_system_get_page_size();
  zx_sampler_config_t config{
      .period = zx::msec(1).get(),
      .buffer_size = buffer_size,
  };
  zx_handle_t sampler;

  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_SAMPLING_BASE);
  ASSERT_OK(result.status_value());
  zx::resource sampling_resource = std::move(result.value());

  zx_status_t create_res = zx_sampler_create(sampling_resource.get(), 1, &config, &sampler);
  ASSERT_EQ(create_res, ZX_ERR_INVALID_ARGS);
}

TEST(ThreadSampler, DroppedSampler) {
  // Ensure we clean up and can create a new sampler if we drop the old one mid session
  size_t buffer_size = zx_system_get_page_size();
  zx_sampler_config_t config{
      .period = zx::msec(1).get(),
      .buffer_size = buffer_size,
  };
  zx_handle_t sampler;

  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_SAMPLING_BASE);
  ASSERT_OK(result.status_value());
  zx::resource sampling_resource = std::move(result.value());

  ASSERT_OK(zx_sampler_create(sampling_resource.get(), 0, &config, &sampler));

  zx::event event;
  ASSERT_EQ(zx::event::create(0, &event), ZX_OK);

  // Create a thread
  std::thread sample_thread{TestFn, event.borrow()};
  zx_handle_t native_handle = native_thread_get_zx_handle(sample_thread.native_handle());
  zx_koid_t tid = GetTid(native_handle);
  ASSERT_NE(tid, ZX_KOID_INVALID);

  ASSERT_OK(event.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite(), nullptr));
  ASSERT_OK(zx_sampler_start(sampler));

  // Drop the sampler mid session
  ASSERT_OK(zx_handle_close(sampler));

  // And create a new one
  ASSERT_OK(zx_sampler_create(sampling_resource.get(), 0, &config, &sampler));
  ASSERT_OK(zx_sampler_start(sampler));
  ASSERT_OK(ReadNRecordsContainingTid(sampler, tid, 10).status_value());
  ASSERT_OK(zx_sampler_stop(sampler));

  ASSERT_OK(event.signal(0, ZX_USER_SIGNAL_1));
  sample_thread.join();

  ASSERT_OK(zx_handle_close(sampler));
}

// We should be able to attach to a started but not running thread. If we do, we should be able to
// get samples from it once it actually starts.
TEST(ThreadSampler, NonRunningThread) {
  size_t buffer_size = zx_system_get_page_size();
  zx_sampler_config_t config{
      .period = zx::msec(1).get(),
      .buffer_size = buffer_size,
  };
  zx_handle_t sampler;

  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_SAMPLING_BASE);
  ASSERT_OK(result.status_value());
  zx::resource sampling_resource = std::move(result.value());

  // Create the thread, but defer starting the thread until after we've attached to it.
  ASSERT_OK(zx_sampler_create(sampling_resource.get(), 0, &config, &sampler));
  TestThread test_thread;
  ASSERT_NO_FATAL_FAILURE(test_thread.Init("NonRunningThread"));
  zx_koid_t tid = GetTid(test_thread.thread().get());
  ASSERT_NE(tid, ZX_KOID_INVALID);

  zx::event event;
  ASSERT_EQ(zx::event::create(0, &event), ZX_OK);
  ASSERT_OK(zx_sampler_start(sampler));

  zx_handle_t event_handle = event.get();

  // Now we actually start the thread. Our request to sample it from earlier should carry over to
  // the now running thread.
  ASSERT_NO_FATAL_FAILURE(test_thread.Start(threads_test_wait_loop, event_handle));
  ASSERT_OK(event.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite(), nullptr));

  ASSERT_OK(ReadNRecordsContainingTid(sampler, tid, 10).status_value());
  ASSERT_OK(zx_sampler_stop(sampler));
  ASSERT_OK(event.signal(0, ZX_USER_SIGNAL_1));

  ASSERT_NO_FATAL_FAILURE(test_thread.Wait());

  ASSERT_OK(zx_handle_close(sampler));
}

TEST(ThreadSampler, HighFrequency) {
  // Start the thread sampler a large number of threads at a high frequency to stress the sampler.

  // We use a larger buffer size than the other tests. We need enough buffer room that the buffers
  // don't immediately fill up and sampling stops.
  size_t buffer_size = size_t{1024} * zx_system_get_page_size();
  zx_sampler_config_t config{
      .period = zx::usec(50).get(),
      .buffer_size = buffer_size,
  };
  zx_handle_t sampler;

  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_SAMPLING_BASE);
  ASSERT_OK(result.status_value());
  zx::resource sampling_resource = std::move(result.value());

  ASSERT_OK(zx_sampler_create(sampling_resource.get(), 0, &config, &sampler));

  std::vector<zx::event> events;
  std::vector<std::thread> threads;
  for (size_t i = 0; i < 4; i++) {
    zx::event& event = events.emplace_back();
    ASSERT_EQ(zx::event::create(0, &event), ZX_OK);

    // Create a thread
    threads.emplace_back(TestFn, event.borrow());
    ASSERT_OK(event.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite(), nullptr));
  }

  ASSERT_OK(zx_sampler_start(sampler));

  ASSERT_OK(ReadNRecords(sampler, 10).status_value());

  ASSERT_OK(zx_sampler_stop(sampler));

  for (auto& event : events) {
    ASSERT_OK(event.signal(0, ZX_USER_SIGNAL_1));
  }
  for (auto& thread : threads) {
    thread.join();
  }

  ASSERT_OK(zx_handle_close(sampler));
}

}  // namespace
