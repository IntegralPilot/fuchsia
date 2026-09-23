// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#define MAGMA_DLOG_ENABLE 1

#include <lib/component/incoming/cpp/directory_watcher.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/magma/magma.h>
#include <lib/magma/util/dlog.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma_client/test_util/magma_map_cpu.h>
#include <lib/magma_client/test_util/test_device_helper.h>
#include <lib/zx/channel.h>
#include <poll.h>

#include <thread>

#include <gtest/gtest.h>

#include "magma_arm_mali_types.h"
#include "mali_utils.h"
#include "src/graphics/drivers/msd-arm-mali/include/magma_vendor_queries.h"

namespace {

class TestConnection : public magma::TestDeviceBase {
 public:
  TestConnection() : magma::TestDeviceBase(MAGMA_VENDOR_ID_MALI) {
    magma_device_create_connection(device(), &connection_);
    DASSERT(connection_);

    magma_connection_create_context(connection_, &context_id_);
    helper_.emplace(connection_, context_id_);
  }

  ~TestConnection() {
    magma_connection_release_context(connection_, context_id_);

    if (connection_)
      magma_connection_release(connection_);
  }

  bool SupportsProtectedMode() {
    uint64_t value_out;
    EXPECT_EQ(MAGMA_STATUS_OK, magma_device_query(device(), kMsdArmVendorQuerySupportsProtectedMode,
                                                  nullptr, &value_out));
    return !!value_out;
  }

  void SubmitCommandBuffer(mali_utils::AtomHelper::How how, uint8_t atom_number,
                           uint8_t atom_dependency, bool protected_mode) {
    helper_->SubmitCommandBuffer(how, atom_number, atom_dependency, protected_mode);
  }

 private:
  magma_connection_t connection_;
  uint32_t context_id_;
  std::optional<mali_utils::AtomHelper> helper_;
};

fidl::ClientEnd<fuchsia_gpu_magma::DebugUtils> GetMaliDebugUtilsClient() {
  zx::result svc_dir = component::OpenServiceRoot();
  EXPECT_FALSE(svc_dir.is_error()) << svc_dir.status_string();
  if (svc_dir.is_error()) {
    return {};
  }

  component::SyncDirectoryWatcher watcher(*svc_dir,
                                          fuchsia_gpu_magma::PowerService::DebugUtils::ServiceName);

  fidl::ClientEnd<fuchsia_gpu_magma::DebugUtils> debug_utils_client;

  while (true) {
    zx::result instance_name = watcher.GetNextEntry(true);
    if (instance_name.is_error()) {
      break;
    }

    zx::result device_client_end =
        component::ConnectAtMember<fuchsia_gpu_magma::PowerService::Device>(*svc_dir,
                                                                            *instance_name);
    EXPECT_FALSE(device_client_end.is_error()) << device_client_end.status_string();
    if (device_client_end.is_error()) {
      continue;
    }

    auto device_client = fidl::WireSyncClient(std::move(*device_client_end));
    auto wire_result = device_client->Query(fuchsia_gpu_magma::wire::QueryId::kVendorId);
    if (wire_result.ok() && wire_result->value()->is_simple_result() &&
        wire_result->value()->simple_result() == MAGMA_VENDOR_ID_MALI) {
      zx::result debug_utils_client_end =
          component::ConnectAtMember<fuchsia_gpu_magma::PowerService::DebugUtils>(*svc_dir,
                                                                                  *instance_name);
      EXPECT_FALSE(debug_utils_client_end.is_error()) << debug_utils_client_end.status_string();
      if (debug_utils_client_end.is_ok()) {
        debug_utils_client = std::move(*debug_utils_client_end);
      }
      break;
    }
  }

  return debug_utils_client;
}

TEST(PowerManagement, SuspendResume) {
  auto debug_utils_client = GetMaliDebugUtilsClient();
  ASSERT_TRUE(debug_utils_client.is_valid()) << "No Mali GPU device found";
  auto client = fidl::WireSyncClient(std::move(debug_utils_client));

  EXPECT_TRUE(client->SetPowerState(0).ok());

  std::unique_ptr<TestConnection> test;
  test.reset(new TestConnection());
  std::atomic_bool submit_returned{false};
  std::thread enable_thread([&] {
    // SubmitCommandBuffer waits 1 second, so delay less than that.
    zx::nanosleep(zx::deadline_after(zx::msec(500)));
    EXPECT_FALSE(submit_returned);
    EXPECT_TRUE(client->SetPowerState(1).ok());
  });
  test->SubmitCommandBuffer(mali_utils::AtomHelper::NORMAL, 1, 0, false);
  submit_returned = true;
  enable_thread.join();
}

// Repeatedly attempt to suspend/resume to GPU to attempt to trigger a soft stop.
TEST(PowerManagement, RepeatedSuspendResume) {
  auto debug_utils_client = GetMaliDebugUtilsClient();
  ASSERT_TRUE(debug_utils_client.is_valid()) << "No Mali GPU device found";
  auto client = fidl::WireSyncClient(std::move(debug_utils_client));

  std::unique_ptr<TestConnection> test;
  test.reset(new TestConnection());
  std::atomic_bool finished_test{false};
  std::thread enable_thread([&] {
    for (uint32_t i = 0; i < 20; i++) {
      constexpr uint32_t kMaxTimeToDelayMs = 10;
      zx::nanosleep(zx::deadline_after(zx::msec(rand() % kMaxTimeToDelayMs)));
      EXPECT_TRUE(client->SetPowerState(0).ok());
      zx::nanosleep(zx::deadline_after(zx::msec(1)));
      EXPECT_TRUE(client->SetPowerState(1).ok());
    }
    finished_test = true;
  });
  uint32_t i = 0;
  while (!finished_test) {
    {
      SCOPED_TRACE(std::to_string(i++));
      test->SubmitCommandBuffer(mali_utils::AtomHelper::NORMAL, 1, 0, false);
    }
  }
  enable_thread.join();
}
}  // namespace
