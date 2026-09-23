// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.gpu.magma/cpp/wire.h>
#include <lib/magma/magma.h>
#include <lib/magma/util/dlog.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma_client/test_util/magma_map_cpu.h>
#include <lib/magma_client/test_util/test_device_helper.h>
#include <lib/zx/channel.h>
#include <lib/zx/clock.h>
#include <poll.h>

#include <gtest/gtest.h>

#include "magma_arm_mali_types.h"
#include "mali_utils.h"
#include "src/graphics/drivers/msd-arm-mali/include/magma_vendor_queries.h"

namespace {

class TestConnection : public magma::TestDeviceBase {
 public:
  TestConnection() : magma::TestDeviceBase(MAGMA_VENDOR_ID_MALI) {
    EXPECT_EQ(MAGMA_STATUS_OK, magma_device_create_connection(device(), &connection_));
    DASSERT(connection_);

    magma_connection_create_context(connection_, &context_id_);
  }

  ~TestConnection() {
    magma_connection_release_context(connection_, context_id_);

    if (connection_)
      magma_connection_release(connection_);
  }

  bool AccessPerfCounters() {
    for (auto& p :
         std::filesystem::directory_iterator("/svc/fuchsia.gpu.magma.PerformanceCounterService")) {
      zx::result client_end = component::Connect<fuchsia_gpu_magma::PerformanceCounterAccess>(
          (static_cast<std::string>(p.path()) + "/access").c_str());
      EXPECT_TRUE(client_end.is_ok()) << client_end.status_string();

      magma_status_t status = magma_connection_enable_performance_counter_access(
          connection_, client_end.value().TakeChannel().release());

      // MAGMA_STATUS_INTERNAL_ERROR occurs because, if the MSD was unbound and rebound prior to
      // this test running, there may be a stale performance counters servic entry.
      // TODO(b/397958414) - remove once services from unbound drivers are reliably removed.
      if (status == MAGMA_STATUS_INTERNAL_ERROR) {
        continue;
      }

      EXPECT_TRUE(status == MAGMA_STATUS_OK || status == MAGMA_STATUS_ACCESS_DENIED);
      if (status == MAGMA_STATUS_OK) {
        return true;
      }
    }
    return false;
  }

  void TestPerfCounters() {
    {
      mali_utils::AtomHelper handler(connection_, context_id_);
      // The hardware can't store performance counters when protected mode is
      // enabled. Submit a non-protected atom to switch to normal mode.
      handler.SubmitCommandBuffer(mali_utils::AtomHelper::NORMAL, 1, 0, false);
    }
    EXPECT_TRUE(AccessPerfCounters());

    magma_buffer_t buffer;
    uint64_t buffer_size;
    magma_buffer_id_t buffer_id;
    constexpr uint32_t kPerfCountBufferSize = 2048;
    EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_create_buffer(connection_, kPerfCountBufferSize * 2,
                                                              &buffer_size, &buffer, &buffer_id));

    magma_perf_count_pool_t pool;
    magma_handle_t notification_handle;

    // Creating and releasing a pool first ensures that the pool will have a non-zero ID (in the
    // current implementation). This could catch some bugs with default values.
    for (uint32_t i = 0; i < 3; i++) {
      EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_create_performance_counter_buffer_pool(
                                     connection_, &pool, &notification_handle));

      EXPECT_EQ(MAGMA_STATUS_OK,
                magma_connection_release_performance_counter_buffer_pool(connection_, pool));
    }

    EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_create_performance_counter_buffer_pool(
                                   connection_, &pool, &notification_handle));
    uint64_t perf_counter_id = 1;
    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_connection_enable_performance_counters(connection_, &perf_counter_id, 1));
    magma_buffer_offset offsets[2];
    offsets[0].buffer_id = buffer_id;
    offsets[0].offset = 0;
    offsets[0].length = kPerfCountBufferSize;
    offsets[1].buffer_id = buffer_id;
    offsets[1].offset = kPerfCountBufferSize;
    offsets[1].length = kPerfCountBufferSize;
    EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_add_performance_counter_buffer_offsets_to_pool(
                                   connection_, pool, offsets, 2));

    uint64_t start_time = zx::clock::get_monotonic().get();

    // Trigger three dumps at once. The last one should be dropped.
    constexpr uint32_t kTriggerId = 5;
    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_connection_dump_performance_counters(connection_, pool, kTriggerId));

    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_connection_dump_performance_counters(connection_, pool, kTriggerId + 1));
    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_connection_dump_performance_counters(connection_, pool, kTriggerId + 2));

    for (uint32_t i = 0; i < 2; i++) {
      magma_poll_item_t poll_item{};
      poll_item.type = MAGMA_POLL_TYPE_HANDLE;
      poll_item.condition = MAGMA_POLL_CONDITION_READABLE;
      poll_item.handle = notification_handle;
      EXPECT_EQ(MAGMA_STATUS_OK, magma_poll(&poll_item, 1, INT64_MAX));

      uint64_t last_possible_time = zx::clock::get_monotonic().get();

      uint32_t trigger_id;
      uint64_t result_buffer_id;
      uint32_t buffer_offset;
      uint64_t time;
      uint32_t result_flags;
      EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_read_performance_counter_completion(
                                     connection_, pool, &trigger_id, &result_buffer_id,
                                     &buffer_offset, &time, &result_flags));

      EXPECT_EQ(buffer_id, result_buffer_id);
      EXPECT_TRUE(trigger_id == kTriggerId || trigger_id == kTriggerId + 1);
      bool expected_discontinuous = i == 0;
      uint32_t expected_result_flags =
          expected_discontinuous ? MAGMA_PERF_COUNTER_RESULT_DISCONTINUITY : 0;
      EXPECT_EQ(expected_result_flags, result_flags);
      EXPECT_LE(start_time, time);
      EXPECT_LE(time, last_possible_time);

      void* data;
      EXPECT_TRUE(magma::MapCpuHelper(buffer, 0 /*offset*/, buffer_size, &data));
      auto data_dwords =
          reinterpret_cast<uint32_t*>(reinterpret_cast<uint8_t*>(data) + buffer_offset);
      constexpr uint32_t kEnableBitsOffset = 2;
      if (i == 0) {
        EXPECT_EQ(0x80ffu, data_dwords[kEnableBitsOffset]);
      }
      EXPECT_TRUE(magma::UnmapCpuHelper(data, buffer_size));
    }

    uint32_t trigger_id;
    uint64_t result_buffer_id;
    uint32_t buffer_offset;
    uint64_t time;
    uint32_t result_flags;
    EXPECT_EQ(MAGMA_STATUS_TIMED_OUT, magma_connection_read_performance_counter_completion(
                                          connection_, pool, &trigger_id, &result_buffer_id,
                                          &buffer_offset, &time, &result_flags));

    magma_connection_release_performance_counter_buffer_pool(connection_, pool);

    magma_connection_release_buffer(connection_, buffer);
  }

 private:
  magma_connection_t connection_;
  uint32_t context_id_;
};

TEST(PerfCounters, Basic) {
  TestConnection connection;
  connection.TestPerfCounters();
}
}  // namespace
