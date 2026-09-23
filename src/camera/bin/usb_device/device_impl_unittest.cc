// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/camera/bin/usb_device/device_impl.h"

#include <fuchsia/camera/cpp/fidl.h>
#include <fuchsia/camera3/cpp/fidl.h>
#include <fuchsia/sysmem2/cpp/fidl.h>
#include <lib/async/cpp/executor.h>
#include <lib/sys/cpp/component_context.h>
#include <zircon/errors.h>

#include <limits>

#include "src/camera/bin/usb_device/stream_impl.h"
#include "src/lib/fsl/handles/object_info.h"
#include "src/lib/testing/loop_fixture/real_loop_fixture.h"

namespace camera {

class DeviceImplTest : public gtest::RealLoopFixture {
 protected:
  DeviceImplTest() : context_(sys::ComponentContext::CreateAndServeOutgoingDirectory()) {}

  void SetUp() override {
    fuchsia::sysmem2::AllocatorPtr allocator;
    context_->svc()->Connect(allocator.NewRequest());
    allocator.set_error_handler(MakeErrorHandler("Sysmem Allocator"));

    fuchsia::sysmem2::AllocatorSetDebugClientInfoRequest set_debug_request;
    set_debug_request.set_name(fsl::GetCurrentProcessName());
    set_debug_request.set_id(fsl::GetCurrentProcessKoid());
    allocator->SetDebugClientInfo(std::move(set_debug_request));

    fidl::InterfaceHandle<fuchsia::camera::Control> control_handle;
    control_request_ = control_handle.NewRequest();
    fuchsia::camera::ControlSyncPtr control = control_handle.BindSync();

    zx::event bad_state_event;
    ASSERT_EQ(zx::event::create(0, &bad_state_event), ZX_OK);

    auto device_promise = DeviceImpl::Create(dispatcher(), std::move(control), std::move(allocator),
                                             std::move(bad_state_event));
    bool device_created = false;
    executor_.schedule_task(device_promise.then(
        [this, &device_created](
            fpromise::result<std::unique_ptr<DeviceImpl>, zx_status_t>& device_result) mutable {
          device_created = true;
          ASSERT_TRUE(device_result.is_ok());
          device_ = device_result.take_value();
        }));
    RunLoopUntil([&device_created] { return device_created; });
    ASSERT_NE(device_, nullptr);
  }

  void TearDown() override {
    device_ = nullptr;
    RunLoopUntilIdle();
  }

  static fit::function<void(zx_status_t status)> MakeErrorHandler(std::string server) {
    return [server](zx_status_t status) {
      ADD_FAILURE() << server << " server disconnected - " << status;
    };
  }

  template <class T>
  static void SetFailOnError(fidl::InterfacePtr<T>& ptr, std::string name = T::Name_) {
    ptr.set_error_handler([=](zx_status_t status) {
      ADD_FAILURE() << name << " server disconnected: " << zx_status_get_string(status);
    });
  }

  void RunLoopUntilFailureOr(bool& condition) {
    RunLoopUntil([&]() { return HasFailure() || condition; });
  }

  void Sync(fuchsia::camera3::DevicePtr& device) {
    bool identifier_returned = false;
    device->GetIdentifier([&](fidl::StringPtr identifier) { identifier_returned = true; });
    RunLoopUntilFailureOr(identifier_returned);
  }

  async::Executor executor_{dispatcher()};
  std::unique_ptr<sys::ComponentContext> context_;
  fidl::InterfaceRequest<fuchsia::camera::Control> control_request_;
  std::unique_ptr<DeviceImpl> device_;
};

TEST_F(DeviceImplTest, ConnectToStream) {
  fuchsia::camera3::DevicePtr device;
  SetFailOnError(device, "Device");
  device_->GetHandler()(device.NewRequest());

  fuchsia::camera3::StreamPtr stream;
  SetFailOnError(stream, "Stream");
  device->ConnectToStream(0, stream.NewRequest());
  Sync(device);

  // Verify stream is bound by attempting a second connection which should fail with ALREADY_BOUND.
  fuchsia::camera3::StreamPtr stream2;
  bool error_received = false;
  stream2.set_error_handler([&](zx_status_t status) {
    EXPECT_EQ(status, ZX_ERR_ALREADY_BOUND);
    error_received = true;
  });
  device->ConnectToStream(0, stream2.NewRequest());
  RunLoopUntilFailureOr(error_received);
}

TEST_F(DeviceImplTest, ConnectToStreamInvalidIndex) {
  fuchsia::camera3::DevicePtr device;
  SetFailOnError(device, "Device");
  device_->GetHandler()(device.NewRequest());

  // Dynamically fetch all configurations from the device.
  std::vector<fuchsia::camera3::Configuration2> configurations;
  bool configurations_returned = false;
  device->GetConfigurations2([&](std::vector<fuchsia::camera3::Configuration2> configs) {
    configurations = std::move(configs);
    configurations_returned = true;
  });
  RunLoopUntilFailureOr(configurations_returned);
  ASSERT_FALSE(configurations.empty());

  for (uint32_t config_index = 0; config_index < configurations.size(); ++config_index) {
    device->SetCurrentConfiguration(config_index);

    const auto& config = configurations[config_index];
    uint32_t num_streams = static_cast<uint32_t>(config.streams().size());
    ASSERT_GT(num_streams, 0u);

    // Test index equal to streams_.size() (exact boundary condition).
    {
      fuchsia::camera3::StreamPtr stream;
      bool error_received = false;
      stream.set_error_handler([&](zx_status_t status) {
        EXPECT_EQ(status, ZX_ERR_INVALID_ARGS);
        error_received = true;
      });
      device->ConnectToStream(num_streams, stream.NewRequest());
      RunLoopUntilFailureOr(error_received);
    }

    // Test index greater than streams_.size().
    {
      fuchsia::camera3::StreamPtr stream;
      bool error_received = false;
      stream.set_error_handler([&](zx_status_t status) {
        EXPECT_EQ(status, ZX_ERR_INVALID_ARGS);
        error_received = true;
      });
      device->ConnectToStream(num_streams + 1, stream.NewRequest());
      RunLoopUntilFailureOr(error_received);
    }

    // Test max uint32 index.
    {
      fuchsia::camera3::StreamPtr stream;
      bool error_received = false;
      stream.set_error_handler([&](zx_status_t status) {
        EXPECT_EQ(status, ZX_ERR_INVALID_ARGS);
        error_received = true;
      });
      device->ConnectToStream(std::numeric_limits<uint32_t>::max(), stream.NewRequest());
      RunLoopUntilFailureOr(error_received);
    }

    // Verify all valid stream indices [0, num_streams) connect successfully.
    for (uint32_t stream_index = 0; stream_index < num_streams; ++stream_index) {
      fuchsia::camera3::StreamPtr stream;
      SetFailOnError(stream, "Stream");
      device->ConnectToStream(stream_index, stream.NewRequest());
      Sync(device);
    }
  }
}

TEST_F(DeviceImplTest, StreamClientAlreadyBound) {
  fuchsia::camera3::DevicePtr device;
  SetFailOnError(device, "Device");
  device_->GetHandler()(device.NewRequest());

  fuchsia::camera3::StreamPtr stream1;
  SetFailOnError(stream1, "Stream 1");
  device->ConnectToStream(0, stream1.NewRequest());
  Sync(device);

  // Connecting a second client to stream 0 should fail with ZX_ERR_ALREADY_BOUND.
  fuchsia::camera3::StreamPtr stream2;
  bool error_received = false;
  stream2.set_error_handler([&](zx_status_t status) {
    EXPECT_EQ(status, ZX_ERR_ALREADY_BOUND);
    error_received = true;
  });
  device->ConnectToStream(0, stream2.NewRequest());
  RunLoopUntilFailureOr(error_received);
}

TEST_F(DeviceImplTest, StreamClientDisconnectAndReconnect) {
  fuchsia::camera3::DevicePtr device;
  SetFailOnError(device, "Device");
  device_->GetHandler()(device.NewRequest());

  // Connect the first client.
  fuchsia::camera3::StreamPtr stream1;
  SetFailOnError(stream1, "Stream 1");
  device->ConnectToStream(0, stream1.NewRequest());
  Sync(device);

  // Try to connect a second client, which should fail.
  fuchsia::camera3::StreamPtr stream2;
  bool error_received = false;
  stream2.set_error_handler([&](zx_status_t status) {
    EXPECT_EQ(status, ZX_ERR_ALREADY_BOUND);
    error_received = true;
  });
  device->ConnectToStream(0, stream2.NewRequest());
  RunLoopUntilFailureOr(error_received);

  // Disconnect the first client.
  stream1 = nullptr;

  // Reconnecting should succeed once the first client disconnect has processed.
  bool reconnected = false;
  while (!HasFailure() && !reconnected) {
    error_received = false;
    fuchsia::camera3::StreamPtr stream3;
    stream3.set_error_handler([&](zx_status_t status) { error_received = true; });
    device->ConnectToStream(0, stream3.NewRequest());
    Sync(device);
    if (!error_received) {
      // Successfully reconnected. Verify stream3 is bound by checking already-bound error on
      // another connection.
      fuchsia::camera3::StreamPtr stream4;
      bool conflict_detected = false;
      stream4.set_error_handler([&](zx_status_t status) {
        EXPECT_EQ(status, ZX_ERR_ALREADY_BOUND);
        conflict_detected = true;
      });
      device->ConnectToStream(0, stream4.NewRequest());
      RunLoopUntilFailureOr(conflict_detected);
      reconnected = true;
    }
  }
}

TEST_F(DeviceImplTest, MultipleDeviceClients) {
  fuchsia::camera3::DevicePtr device1;
  SetFailOnError(device1, "Device 1");
  device_->GetHandler()(device1.NewRequest());

  fuchsia::camera3::DevicePtr device2;
  SetFailOnError(device2, "Device 2");
  device_->GetHandler()(device2.NewRequest());

  // Connect stream via device1.
  fuchsia::camera3::StreamPtr stream1;
  SetFailOnError(stream1, "Stream 1");
  device1->ConnectToStream(0, stream1.NewRequest());
  Sync(device1);

  // Attempt to connect stream via device2 should fail with ALREADY_BOUND.
  fuchsia::camera3::StreamPtr stream2;
  bool error_received = false;
  stream2.set_error_handler([&](zx_status_t status) {
    EXPECT_EQ(status, ZX_ERR_ALREADY_BOUND);
    error_received = true;
  });
  device2->ConnectToStream(0, stream2.NewRequest());
  RunLoopUntilFailureOr(error_received);

  // Disconnect stream1, now device2 should be able to connect to stream.
  stream1 = nullptr;

  bool reconnected = false;
  while (!HasFailure() && !reconnected) {
    error_received = false;
    fuchsia::camera3::StreamPtr stream3;
    stream3.set_error_handler([&](zx_status_t status) { error_received = true; });
    device2->ConnectToStream(0, stream3.NewRequest());
    Sync(device2);
    if (!error_received) {
      // Verify stream3 is bound.
      fuchsia::camera3::StreamPtr stream4;
      bool conflict_detected = false;
      stream4.set_error_handler([&](zx_status_t status) {
        EXPECT_EQ(status, ZX_ERR_ALREADY_BOUND);
        conflict_detected = true;
      });
      device1->ConnectToStream(0, stream4.NewRequest());
      RunLoopUntilFailureOr(conflict_detected);
      reconnected = true;
    }
  }
}

TEST_F(DeviceImplTest, Rebind) {
  fuchsia::camera3::DevicePtr device1;
  SetFailOnError(device1, "Device 1");
  device_->GetHandler()(device1.NewRequest());

  fuchsia::camera3::DevicePtr device2;
  SetFailOnError(device2, "Device 2");
  device1->Rebind(device2.NewRequest());

  fuchsia::camera3::StreamPtr stream;
  SetFailOnError(stream, "Stream");
  device2->ConnectToStream(0, stream.NewRequest());
  Sync(device2);

  // Verify stream is bound.
  fuchsia::camera3::StreamPtr stream2;
  bool error_received = false;
  stream2.set_error_handler([&](zx_status_t status) {
    EXPECT_EQ(status, ZX_ERR_ALREADY_BOUND);
    error_received = true;
  });
  device1->ConnectToStream(0, stream2.NewRequest());
  RunLoopUntilFailureOr(error_received);
}

}  // namespace camera
