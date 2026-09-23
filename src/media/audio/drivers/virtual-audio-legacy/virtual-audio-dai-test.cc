// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/drivers/virtual-audio-legacy/virtual-audio-dai.h"

#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <fidl/fuchsia.virtualaudio/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/fit/defer.h>
#include <lib/sync/completion.h>

#include <zxtest/zxtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/media/audio/drivers/virtual-audio-legacy/virtual-audio-device.h"

namespace virtual_audio {
namespace {

class VirtualAudioDaiTest : public zxtest::Test {
 public:
  void SetUp() override {
    fake_parent_ = MockDevice::FakeRootParent();
    runtime_ = fdf_testing::DriverRuntime::GetInstance();
    ASSERT_NOT_NULL(runtime_);
    dispatcher_ = runtime_->StartBackgroundDispatcher();
    ASSERT_NOT_NULL(dispatcher_->async_dispatcher());
  }

  void TearDown() override {
    if (device_) {
      libsync::Completion shutdown_started;
      async::PostTask(dispatcher_->async_dispatcher(), [&]() {
        device_->ShutdownAsync();
        shutdown_started.Signal();
      });
      shutdown_started.Wait();
      mock_ddk::ReleaseFlaggedDevices(fake_parent_.get());
      shutdown_done_.Wait();
      device_.reset();
    }
  }

 protected:
  std::shared_ptr<MockDevice> fake_parent_;
  fdf_testing::DriverRuntime* runtime_ = nullptr;
  fdf::UnownedSynchronizedDispatcher dispatcher_;
  std::shared_ptr<VirtualAudioDevice> device_;
  libsync::Completion shutdown_done_;
};

// Verify that multiple VirtualAudioDai clients can connect concurrently, querying props/formats.
TEST_F(VirtualAudioDaiTest, MultipleClientsConnectAndQuery) {
  fidl::ClientEnd<fuchsia_hardware_audio::DaiConnector> conn_client_end;
  libsync::Completion done;
  async::PostTask(dispatcher_->async_dispatcher(), [&]() {
    auto signal_done = fit::defer([&done]() { done.Signal(); });

    auto config = VirtualAudioDai::GetDefaultConfig(true);
    auto [control_client_end, control_server_end] =
        fidl::Endpoints<fuchsia_virtualaudio::Device>::Create();
    auto create_res =
        VirtualAudioDevice::Create(config, std::move(control_server_end), fake_parent_.get(),
                                   [&]() { shutdown_done_.Signal(); });
    ASSERT_TRUE(create_res.is_ok());
    device_ = create_res.value();

    auto* child = fake_parent_->GetLatestChild();
    ASSERT_NOT_NULL(child);
    auto* dai = child->GetDeviceContext<VirtualAudioDai>();
    ASSERT_NOT_NULL(dai);

    auto [client_end, server_end] = fidl::Endpoints<fuchsia_hardware_audio::DaiConnector>::Create();
    fidl::BindServer(dispatcher_->async_dispatcher(), std::move(server_end), dai);
    conn_client_end = std::move(client_end);
  });
  done.Wait();

  fidl::SyncClient conn_client(std::move(conn_client_end));

  // Connect Client 1.
  auto [dai1_client_end, dai1_server_end] = fidl::Endpoints<fuchsia_hardware_audio::Dai>::Create();
  auto connect_res1 = conn_client->Connect(std::move(dai1_server_end));
  ASSERT_TRUE(connect_res1.is_ok());

  // Connect Client 2 concurrently.
  auto [dai2_client_end, dai2_server_end] = fidl::Endpoints<fuchsia_hardware_audio::Dai>::Create();
  auto connect_res2 = conn_client->Connect(std::move(dai2_server_end));
  ASSERT_TRUE(connect_res2.is_ok());

  fidl::SyncClient dai_client1(std::move(dai1_client_end));
  fidl::SyncClient dai_client2(std::move(dai2_client_end));

  // Client 1 queries properties.
  auto props1 = dai_client1->GetProperties();
  ASSERT_TRUE(props1.is_ok());
  EXPECT_EQ(props1->properties().is_input(), true);
  EXPECT_EQ(props1->properties().manufacturer(), "Fuchsia Virtual Audio Group");

  // Client 2 queries properties concurrently without being rejected.
  auto props2 = dai_client2->GetProperties();
  ASSERT_TRUE(props2.is_ok());
  EXPECT_EQ(props2->properties().is_input(), true);
  EXPECT_EQ(props2->properties().manufacturer(), "Fuchsia Virtual Audio Group");

  // Client 1 queries DAI formats.
  auto formats1 = dai_client1->GetDaiFormats();
  ASSERT_TRUE(formats1.is_ok());
  EXPECT_FALSE(formats1->dai_formats().empty());

  // Client 2 queries RingBuffer formats.
  auto rb_formats2 = dai_client2->GetRingBufferFormats();
  ASSERT_TRUE(rb_formats2.is_ok());
  EXPECT_FALSE(rb_formats2->ring_buffer_formats().empty());

  // Client 1 disconnects.
  dai_client1 = {};

  // Client 2 should continue to function normally.
  auto formats2 = dai_client2->GetDaiFormats();
  ASSERT_TRUE(formats2.is_ok());
  EXPECT_FALSE(formats2->dai_formats().empty());
}

}  // namespace
}  // namespace virtual_audio
