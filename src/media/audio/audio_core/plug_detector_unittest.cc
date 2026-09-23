// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/plug_detector.h"

#include <fuchsia/hardware/audio/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/fdio/namespace.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/syslog/cpp/macros.h>

#include <future>

#include "src/lib/testing/loop_fixture/real_loop_fixture.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/service.h"
#include "src/storage/lib/vfs/cpp/synchronous_vfs.h"

namespace media::audio {
namespace {

// A minimal |fuchsia::hardware::audio::Device| that we can use to emulate a fake devfs directory
// for testing.
class FakeAudioDevice : public fuchsia::hardware::audio::StreamConfigConnector,
                        public fuchsia::hardware::audio::StreamConfig {
 public:
  FakeAudioDevice() : loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {
    loop_.StartThread("fake-audio-device-loop");
  }
  ~FakeAudioDevice() override { loop_.Shutdown(); }

  fbl::RefPtr<fs::Service> AsService() {
    return fbl::MakeRefCounted<fs::Service>([this](zx::channel c) {
      connector_binding_.Bind(std::move(c), loop_.dispatcher());
      return ZX_OK;
    });
  }

  bool is_bound() const { return stream_config_binding_->is_bound(); }

 private:
  // FIDL method for fuchsia.hardware.audio.StreamConfigConnector.
  void Connect(fidl::InterfaceRequest<fuchsia::hardware::audio::StreamConfig> server) override {
    stream_config_binding_.emplace(this, std::move(server), loop_.dispatcher());
  }

  // FIDL methods for fuchsia.hardware.audio.StreamConfig.
  void GetProperties(GetPropertiesCallback callback) override { callback({}); }
  void GetSupportedFormats(GetSupportedFormatsCallback callback) override { callback({}); }
  void CreateRingBuffer(
      ::fuchsia::hardware::audio::Format format,
      ::fidl::InterfaceRequest<::fuchsia::hardware::audio::RingBuffer> intf) override {}
  void WatchGainState(WatchGainStateCallback callback) override { callback({}); }
  void SetGain(::fuchsia::hardware::audio::GainState target_state) override {}
  void WatchPlugState(WatchPlugStateCallback callback) override { callback({}); }
  void GetHealthState(GetHealthStateCallback callback) override { callback({}); }
  void SignalProcessingConnect(
      fidl::InterfaceRequest<fuchsia::hardware::audio::signalprocessing::SignalProcessing>
          signal_processing) override {
    signal_processing.Close(ZX_ERR_NOT_SUPPORTED);
  }

  async::Loop loop_;
  std::optional<fidl::Binding<fuchsia::hardware::audio::StreamConfig>> stream_config_binding_;
  fidl::Binding<fuchsia::hardware::audio::StreamConfigConnector> connector_binding_{this};
};

class DeviceTracker {
 public:
  struct DeviceConnection {
    std::string name;
    bool is_input;
    fidl::InterfaceHandle<fuchsia::hardware::audio::StreamConfig> stream_config;
  };

  fit::function<void(const std::string&, bool,
                     fidl::InterfaceHandle<fuchsia::hardware::audio::StreamConfig>)>
  GetHandler() {
    return [this](auto name, auto is_input, auto stream_config) {
      // To make sure the 1-way Connect call is completed in the StreamConfigConnector server, make
      // a 2-way call.
      fidl::InterfacePtr client = stream_config.Bind();
      client.set_error_handler([](zx_status_t status) { FAIL() << zx_status_get_string(status); });
      client->GetProperties([=, name = std::move(name), client = std::move(client)](
                                const fuchsia::hardware::audio::StreamProperties&) mutable {
        devices_.emplace_back(DeviceConnection{std::move(name), is_input, client.Unbind()});
      });
    };
  }

  size_t size() const { return devices_.size(); }

  std::vector<DeviceConnection> take_devices() { return std::move(devices_); }

 private:
  std::vector<DeviceConnection> devices_;
};

class PlugDetectorTest : public gtest::RealLoopFixture,
                         public ::testing::WithParamInterface<const char*> {
 protected:
  void SetUp() override {
    // Setup the emulated svc directory containing both services.
    svc_dir_->AddEntry("fuchsia.hardware.audio.StreamConfigConnectorInputService", input_dir_);
    svc_dir_->AddEntry("fuchsia.hardware.audio.StreamConfigConnectorOutputService", output_dir_);

    ASSERT_EQ(vfs_loop_.StartThread("vfs-loop"), ZX_OK);
  }

  bool IsEmpty(fbl::RefPtr<fs::PseudoDir> dir) {
    std::promise<bool> promise;
    auto future = promise.get_future();
    async::PostTask(vfs_loop_.dispatcher(), [promise = std::move(promise), dir]() mutable {
      promise.set_value(dir->IsEmpty());
    });
    return future.get();
  }

  void TearDown() override {
    EXPECT_TRUE(IsEmpty(input_dir_));
    EXPECT_TRUE(IsEmpty(output_dir_));
    vfs_loop_.Shutdown();
  }

  fidl::ClientEnd<fuchsia_io::Directory> GetSvcClient() {
    auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    std::promise<zx_status_t> promise;
    auto future = promise.get_future();
    async::PostTask(vfs_loop_.dispatcher(),
                    [this, promise = std::move(promise), server = std::move(server)]() mutable {
                      promise.set_value(vfs_.ServeDirectory(svc_dir_, std::move(server)));
                    });
    zx_status_t status = future.get();
    ZX_ASSERT(status == ZX_OK);
    return std::move(client);
  }

  // Holds a reference to a pseudo dir entry that removes the entry when this object goes out of
  // scope.
  struct ScopedDirent {
    std::string name;
    fbl::RefPtr<fs::PseudoDir> dir;
    async_dispatcher_t* dispatcher;

    ScopedDirent(std::string n, fbl::RefPtr<fs::PseudoDir> d, async_dispatcher_t* disp)
        : name(std::move(n)), dir(std::move(d)), dispatcher(disp) {}

    ScopedDirent(const ScopedDirent&) = delete;
    ScopedDirent& operator=(const ScopedDirent&) = delete;
    ScopedDirent(ScopedDirent&&) = default;
    ScopedDirent& operator=(ScopedDirent&&) = delete;

    ~ScopedDirent() {
      if (dir) {
        async::PostTask(dispatcher, [n = name, d = dir]() { d->RemoveEntry(n); });
      }
    }
  };

  // Adds a |FakeAudioDevice| to the emulated 'audio-input' service directory.
  ScopedDirent AddInputDevice(FakeAudioDevice* device) {
    auto instance_name = std::to_string(next_input_device_number_++);
    std::promise<zx_status_t> promise;
    auto future = promise.get_future();
    async::PostTask(vfs_loop_.dispatcher(), [this, instance_name, device,
                                             promise = std::move(promise)]() mutable {
      auto instance_dir = fbl::MakeRefCounted<fs::PseudoDir>();
      zx_status_t status = instance_dir->AddEntry("stream_config_connector", device->AsService());
      if (status != ZX_OK) {
        promise.set_value(status);
        return;
      }
      promise.set_value(input_dir_->AddEntry(instance_name, instance_dir));
    });
    EXPECT_EQ(ZX_OK, future.get());
    return ScopedDirent(instance_name, input_dir_, vfs_loop_.dispatcher());
  }

  // Adds a |FakeAudioDevice| to the emulated 'audio-output' service directory.
  ScopedDirent AddOutputDevice(FakeAudioDevice* device) {
    auto instance_name = std::to_string(next_output_device_number_++);
    std::promise<zx_status_t> promise;
    auto future = promise.get_future();
    async::PostTask(vfs_loop_.dispatcher(), [this, instance_name, device,
                                             promise = std::move(promise)]() mutable {
      auto instance_dir = fbl::MakeRefCounted<fs::PseudoDir>();
      zx_status_t status = instance_dir->AddEntry("stream_config_connector", device->AsService());
      if (status != ZX_OK) {
        promise.set_value(status);
        return;
      }
      promise.set_value(output_dir_->AddEntry(instance_name, instance_dir));
    });
    EXPECT_EQ(ZX_OK, future.get());
    return ScopedDirent(instance_name, output_dir_, vfs_loop_.dispatcher());
  }

 private:
  uint32_t next_input_device_number_ = 0;
  uint32_t next_output_device_number_ = 0;

  async::Loop vfs_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  fs::SynchronousVfs vfs_{vfs_loop_.dispatcher()};
  // Note these _must_ be RefPtrs since the vfs_ will attempt to AdoptRef on a raw pointer passed
  // to it.
  fbl::RefPtr<fs::PseudoDir> input_dir_{fbl::MakeRefCounted<fs::PseudoDir>()};
  fbl::RefPtr<fs::PseudoDir> output_dir_{fbl::MakeRefCounted<fs::PseudoDir>()};
  fbl::RefPtr<fs::PseudoDir> svc_dir_{fbl::MakeRefCounted<fs::PseudoDir>()};
};

TEST_F(PlugDetectorTest, DetectExistingDevices) {
  // Add some devices that will exist before the plug detector starts.
  FakeAudioDevice input0, input1;
  auto d1 = AddInputDevice(&input0);
  auto d2 = AddInputDevice(&input1);
  FakeAudioDevice output0, output1;
  auto d3 = AddOutputDevice(&output0);
  auto d4 = AddOutputDevice(&output1);

  // Create the plug detector; no events should be sent until |Start|.
  DeviceTracker tracker;
  auto plug_detector = PlugDetector::Create(GetSvcClient());
  RunLoopUntilIdle();
  EXPECT_EQ(0u, tracker.size());

  // Start the detector; expect 4 events (1 for each device above);
  ASSERT_EQ(ZX_OK, plug_detector->Start(tracker.GetHandler()));
  RunLoopUntil([&tracker] { return tracker.size() == 4; });
  EXPECT_EQ(4u, tracker.size());
  EXPECT_TRUE(input0.is_bound());
  EXPECT_TRUE(input1.is_bound());
  EXPECT_TRUE(output0.is_bound());
  EXPECT_TRUE(output1.is_bound());

  plug_detector->Stop();
}

TEST_F(PlugDetectorTest, DetectHotplugDevices) {
  DeviceTracker tracker;
  auto plug_detector = PlugDetector::Create(GetSvcClient());
  ASSERT_EQ(ZX_OK, plug_detector->Start(tracker.GetHandler()));
  RunLoopUntilIdle();
  EXPECT_EQ(0u, tracker.size());

  // Hotplug a device.
  FakeAudioDevice input0;
  auto d1 = AddInputDevice(&input0);
  RunLoopUntil([&tracker] { return tracker.size() == 1; });
  ASSERT_EQ(1u, tracker.size());
  auto device = std::move(*tracker.take_devices().begin());
  EXPECT_TRUE(device.is_input);
  EXPECT_TRUE(input0.is_bound());

  plug_detector->Stop();
}

}  // namespace
}  // namespace media::audio
