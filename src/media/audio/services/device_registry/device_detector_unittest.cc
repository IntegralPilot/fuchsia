// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/device_detector.h"

#include <fidl/fuchsia.audio.device/cpp/natural_types.h>
#include <fidl/fuchsia.hardware.audio/cpp/test_base.h>
#include <lib/async/cpp/task.h>
#include <lib/fdio/namespace.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/internal/transport.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/zx/clock.h>

#include <memory>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/media/audio/services/device_registry/inspector.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/service.h"
#include "src/storage/lib/vfs/cpp/synchronous_vfs.h"

namespace media_audio {
namespace {

namespace fad = fuchsia_audio_device;
namespace fha = fuchsia_hardware_audio;

// Minimal Codec used to emulate a fake devfs directory for tests.
class FakeAudioCodec : public fidl::testing::TestBase<fha::Codec> {
 public:
  FakeAudioCodec(const FakeAudioCodec&) = delete;
  FakeAudioCodec(FakeAudioCodec&&) = delete;
  FakeAudioCodec& operator=(const FakeAudioCodec&) = delete;
  FakeAudioCodec& operator=(FakeAudioCodec&&) = delete;

  explicit FakeAudioCodec(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    ADD_FAILURE() << "Method not implemented: '" << name << "";
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  // Used synchronously, this 2-way call ensures that Connect is complete, before we proceed.
  void GetProperties(GetPropertiesCompleter::Sync& completer) override { completer.Reply({}); }

  fbl::RefPtr<fs::Service> AsService() {
    return fbl::MakeRefCounted<fs::Service>([this](fidl::ServerEnd<fha::CodecConnector> c) {
      connector_binding_ = fidl::BindServer(dispatcher(), std::move(c), &connector_);
      return ZX_OK;
    });
  }

  bool is_bound() const { return binding_.has_value(); }
  async_dispatcher_t* dispatcher() { return dispatcher_; }

 private:
  class Connector : public fidl::Server<fha::CodecConnector> {
   public:
    explicit Connector(FakeAudioCodec* parent) : parent_(parent) {}
    void Connect(ConnectRequest& request, ConnectCompleter::Sync& completer) override {
      parent_->binding_ =
          fidl::BindServer(parent_->dispatcher(), std::move(request.codec_protocol()), parent_);
    }

   private:
    FakeAudioCodec* parent_;
  };

  async_dispatcher_t* dispatcher_;
  Connector connector_{this};
  std::optional<fidl::ServerBindingRef<fha::CodecConnector>> connector_binding_;
  std::optional<fidl::ServerBindingRef<fha::Codec>> binding_;
};

// TODO(https://fxbug.dev/304551042): Convert VirtualAudioComposite to DFv2; remove Connector.
class FakeAudioComposite : public fidl::testing::TestBase<fha::Composite> {
 public:
  FakeAudioComposite(const FakeAudioComposite&) = delete;
  FakeAudioComposite(FakeAudioComposite&&) = delete;
  FakeAudioComposite& operator=(const FakeAudioComposite&) = delete;
  FakeAudioComposite& operator=(FakeAudioComposite&&) = delete;

  explicit FakeAudioComposite(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    ADD_FAILURE() << "Method not implemented: '" << name << "";
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void GetProperties(GetPropertiesCompleter::Sync& completer) override { completer.Reply({}); }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_audio::Composite> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  fbl::RefPtr<fs::Service> AsService() {
    return fbl::MakeRefCounted<fs::Service>([this](fidl::ServerEnd<fha::Composite> c) {
      binding_ = fidl::BindServer(dispatcher(), std::move(c), this);
      return ZX_OK;
    });
  }

  bool is_bound() const { return binding_.has_value(); }
  async_dispatcher_t* dispatcher() { return dispatcher_; }

 private:
  async_dispatcher_t* dispatcher_;
  std::optional<fidl::ServerBindingRef<fha::Composite>> binding_;
};

class DeviceTracker {
 public:
  struct DeviceConnection {
    std::string_view name;
    fad::DeviceType device_type;
    fad::DriverClient client;
  };

  DeviceTracker(async_dispatcher_t* dispatcher, bool detection_is_expected)
      : dispatcher_(dispatcher), detection_is_expected_(detection_is_expected) {}

  virtual ~DeviceTracker() = default;

  size_t size() const { return devices_.size(); }
  const std::vector<DeviceConnection>& devices() const { return devices_; }
  DeviceDetectionHandler handler() { return handler_; }
  DeviceDetectionIdleHandler idle_handler() { return idle_handler_; }
  async_dispatcher_t* dispatcher() { return dispatcher_; }

 private:
  DeviceDetectionHandler handler_ = [this](std::string_view name, fad::DeviceType device_type,
                                           fad::DriverClient driver_client) {
    ASSERT_TRUE(detection_is_expected_) << "Unexpected device detection";

    devices_.emplace_back(DeviceConnection{
        .name = name,
        .device_type = device_type,
        .client = std::move(driver_client),
    });
  };
  DeviceDetectionIdleHandler idle_handler_ = [this]() { detection_idle_received_ = true; };

  async_dispatcher_t* dispatcher_;
  const bool detection_is_expected_;
  bool detection_idle_received_ = false;

  std::vector<DeviceConnection> devices_;
};

class DeviceDetectorTest : public gtest::TestLoopFixture {
 protected:
  void SetUp() override {
    // Use our production Inspector during DeviceDetector unittests.
    media_audio::Inspector::Initialize(dispatcher());

    ASSERT_EQ(fdio_ns_get_installed(&ns_), ZX_OK);
    zx::channel channel0, channel1;

    // Serve up the emulated audio-composite directory
    auto [composite_client, composite_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_EQ(vfs_.ServeDirectory(composite_dir_, std::move(composite_server)), ZX_OK);
    ASSERT_EQ(
        fdio_ns_bind(ns_, "/dev/class/audio-composite", composite_client.TakeChannel().release()),
        ZX_OK);

    // Serve up the emulated codec directory
    auto [codec_client, codec_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_EQ(vfs_.ServeDirectory(codec_dir_, std::move(codec_server)), ZX_OK);
    ASSERT_EQ(fdio_ns_bind(ns_, "/dev/class/codec", codec_client.TakeChannel().release()), ZX_OK);
  }

  void TearDown() override {
    // Scoped directory entries have gone out of scope, but to avoid races we remove all entries.
    bool task_has_run = false;
    async::PostTask(dispatcher(), [this, &task_has_run]() {
      codec_dir_->RemoveAllEntries();
      composite_dir_->RemoveAllEntries();
      task_has_run = true;
    });
    while (!task_has_run) {
      RunLoopUntilIdle();
    }
    ASSERT_TRUE(codec_dir_->IsEmpty() && composite_dir_->IsEmpty())
        << "codec_dir is " << (codec_dir_->IsEmpty() ? "" : "NOT ") << "empty; composite_dir is "
        << (composite_dir_->IsEmpty() ? "" : "NOT ") << "empty";

    ASSERT_NE(ns_, nullptr);
    ASSERT_EQ(fdio_ns_unbind(ns_, "/dev/class/audio-composite"), ZX_OK);
    ASSERT_EQ(fdio_ns_unbind(ns_, "/dev/class/codec"), ZX_OK);
  }

  // Holds a ref to a pseudo dir entry that removes the entry when this object goes out of scope.
  struct ScopedDirent {
    std::string name;
    fbl::RefPtr<fs::PseudoDir> dir;
    async_dispatcher_t* dispatcher;
    ~ScopedDirent() {
      async::PostTask(dispatcher, [n = name, d = dir]() { d->RemoveEntry(n); });
    }
  };

  // Adds a `FakeAudioCodec` to the emulated 'codec' directory that has been installed in
  // the local namespace at /dev/class/codec.
  ScopedDirent AddCodecDevice(std::shared_ptr<FakeAudioCodec> device) {
    auto name = std::to_string(next_device_number_++);
    bool task_has_run = false;
    async::PostTask(dispatcher(), [this, name, device = std::move(device), &task_has_run]() {
      task_has_run = true;
      ASSERT_EQ(ZX_OK, codec_dir_->AddEntry(name, device->AsService()));
    });
    while (!task_has_run) {
      RunLoopUntilIdle();
    }
    return {
        .name = name,
        .dir = codec_dir_,
        .dispatcher = dispatcher(),
    };
  }

  // Adds a `FakeAudioComposite` to the emulated 'composite' directory that has been installed in
  // the local namespace at /dev/class/audio-composite.
  ScopedDirent AddCompositeDevice(std::shared_ptr<FakeAudioComposite> device) {
    auto name = std::to_string(next_device_number_++);
    bool task_has_run = false;
    async::PostTask(dispatcher(), [this, name, device = std::move(device), &task_has_run]() {
      task_has_run = true;
      ASSERT_EQ(ZX_OK, composite_dir_->AddEntry(name, device->AsService()));
    });
    while (!task_has_run) {
      RunLoopUntilIdle();
    }
    return {
        .name = name,
        .dir = composite_dir_,
        .dispatcher = dispatcher(),
    };
  }

 private:
  fdio_ns_t* ns_ = nullptr;
  uint32_t next_device_number_ = 0;

  fs::SynchronousVfs vfs_{dispatcher()};

  // Note these _must_ be RefPtrs since vfs_ will try to AdoptRef on the raw pointer passed to it.
  fbl::RefPtr<fs::PseudoDir> codec_dir_{fbl::MakeRefCounted<fs::PseudoDir>()};
  fbl::RefPtr<fs::PseudoDir> composite_dir_{fbl::MakeRefCounted<fs::PseudoDir>()};
};

// For devices that exist before the plug detector, verify pre-Start, post-Start, post-Stop.
TEST_F(DeviceDetectorTest, DetectExistingDevices) {
  // Add some devices that will exist before the plug detector is created.
  auto composite0 = std::make_shared<FakeAudioComposite>(dispatcher());
  auto codec0 = std::make_shared<FakeAudioCodec>(dispatcher());
  auto composite1 = std::make_shared<FakeAudioComposite>(dispatcher());
  auto codec1 = std::make_shared<FakeAudioCodec>(dispatcher());

  [[maybe_unused]] auto dev0 = AddCompositeDevice(composite0);
  [[maybe_unused]] auto dev1 = AddCodecDevice(codec0);
  [[maybe_unused]] auto dev2 = AddCompositeDevice(composite1);
  [[maybe_unused]] auto dev3 = AddCodecDevice(codec1);

  auto tracker = std::make_shared<DeviceTracker>(dispatcher(), /*detection_is_expected=*/true);
  RunLoopUntilIdle();
  ASSERT_EQ(0u, tracker->size());
  {
    // Create the detector; expect 8 events (1 for each device above);
    auto device_detector = DeviceDetector::Create(tracker->handler(), tracker->idle_handler(),
                                                  dispatcher(), /*ignore_devices=*/false);
    RunLoopUntilIdle();
    ASSERT_TRUE(composite0->is_bound());
    ASSERT_TRUE(composite1->is_bound());
    ASSERT_TRUE(codec0->is_bound());
    ASSERT_TRUE(codec1->is_bound());
    ASSERT_EQ(tracker->size(), 4u) << "Timed out waiting for preexisting devices to be detected";

    int num_composites = 0, num_codecs = 0;
    for (auto dev_num = 0u; dev_num < tracker->size(); ++dev_num) {
      auto& device = tracker->devices()[dev_num];
      if (device.device_type == fad::DeviceType::kComposite) {
        EXPECT_EQ(device.client.Which(), fad::DriverClient::Tag::kComposite);
        EXPECT_TRUE(device.client.composite()->is_valid());
        ++num_composites;
      } else if (device.device_type == fad::DeviceType::kCodec) {
        EXPECT_EQ(device.client.Which(), fad::DriverClient::Tag::kCodec);
        EXPECT_TRUE(device.client.codec()->is_valid());
        ++num_codecs;
      } else {
        ADD_FAILURE() << "Unknown device_type during test";
      }
    }
    EXPECT_EQ(num_composites, 2);
    EXPECT_EQ(num_codecs, 2);
  }

  // After the detector is gone, preexisting devices we detected should still be bound.
  RunLoopUntilIdle();

  std::for_each(tracker->devices().begin(), tracker->devices().end(), [](const auto& device) {
    switch (device.device_type) {
      case fad::DeviceType::kCodec:
        EXPECT_TRUE(device.client.codec()->is_valid());
        break;
      case fad::DeviceType::kComposite:
        EXPECT_TRUE(device.client.composite()->is_valid());
        break;
      default:
        ADD_FAILURE() << "Unknown device_type after test";
        break;
    }
  });

  EXPECT_TRUE(composite0->is_bound());
  EXPECT_TRUE(composite1->is_bound());
  EXPECT_TRUE(codec0->is_bound());
  EXPECT_TRUE(codec1->is_bound());
}

// For devices added after the plug detector, verify detection (and post-detector persistence).
TEST_F(DeviceDetectorTest, DetectHotplugDevices) {
  auto composite = std::make_shared<FakeAudioComposite>(dispatcher());
  auto codec = std::make_shared<FakeAudioCodec>(dispatcher());

  auto tracker = std::make_shared<DeviceTracker>(dispatcher(), /*detection_is_expected=*/true);
  {
    auto device_detector = DeviceDetector::Create(tracker->handler(), tracker->idle_handler(),
                                                  dispatcher(), /*ignore_devices=*/false);

    RunLoopUntilIdle();
    ASSERT_EQ(0u, tracker->size());

    // Hotplug a device of each type.
    [[maybe_unused]] auto dev0 = AddCompositeDevice(composite);
    RunLoopUntilIdle();
    ASSERT_TRUE(composite->is_bound());
    ASSERT_EQ(tracker->size(), 1u) << "Timed out waiting for composite device to be detected";

    [[maybe_unused]] auto dev1 = AddCodecDevice(codec);
    RunLoopUntilIdle();
    ASSERT_TRUE(codec->is_bound());
    ASSERT_EQ(tracker->size(), 2u) << "Timed out waiting for codec device to be detected";

    EXPECT_EQ(tracker->devices()[0].device_type, fad::DeviceType::kComposite);
    EXPECT_EQ(tracker->devices()[1].device_type, fad::DeviceType::kCodec);
  }

  // After the device detector is gone, dynamically-detected devices should still be bound.
  RunLoopUntilIdle();  // Allow any erroneous device unbinds to reveal themselves.

  std::for_each(tracker->devices().begin(), tracker->devices().end(), [](const auto& device) {
    switch (device.device_type) {
      case fad::DeviceType::kCodec:
        EXPECT_TRUE(device.client.codec()->is_valid());
        break;
      case fad::DeviceType::kComposite:
        EXPECT_TRUE(device.client.composite()->is_valid());
        break;
      default:
        ADD_FAILURE() << "Unknown device_type after test";
        break;
    }
  });

  EXPECT_TRUE(composite->is_bound());
  EXPECT_TRUE(codec->is_bound());
}

// Ensure that once the plug detector is destroyed, detection handlers are no longer called.
TEST_F(DeviceDetectorTest, NoDanglingDetectors) {
  auto codec = std::make_shared<FakeAudioCodec>(dispatcher());
  auto composite = std::make_shared<FakeAudioComposite>(dispatcher());
  auto tracker = std::make_shared<DeviceTracker>(dispatcher(), /*detection_is_expected=*/false);

  {
    auto device_detector = DeviceDetector::Create(tracker->handler(), tracker->idle_handler(),
                                                  dispatcher(), /*ignore_devices=*/false);
    RunLoopUntilIdle();  // Allow erroneous device handler callbacks to reveal themselves.
    ASSERT_EQ(0u, tracker->size());
  }
  // After the device detector is gone, additional devices should not be detected.

  // Hotplug an input device, an output device and a codec device.
  // If a device-detection handler is still in place, these will be inserted into tracker's list.
  [[maybe_unused]] auto dev0 = AddCodecDevice(codec);
  [[maybe_unused]] auto dev1 = AddCompositeDevice(composite);
  RunLoopUntilIdle();  // Allow erroneous device handler callbacks to reveal themselves.
  EXPECT_EQ(0u, tracker->size());
  EXPECT_FALSE(codec->is_bound());
  EXPECT_FALSE(composite->is_bound());
}

// Verify that plug detector disregards preexisting audio devices, if `ignore_devices` is set.
TEST_F(DeviceDetectorTest, IgnoreDevicesDoesNotDetectExistingDevices) {
  // Add some devices that will exist before the plug detector is created.
  auto composite0 = std::make_shared<FakeAudioComposite>(dispatcher());
  auto codec0 = std::make_shared<FakeAudioCodec>(dispatcher());
  [[maybe_unused]] auto dev0 = AddCompositeDevice(composite0);
  [[maybe_unused]] auto dev1 = AddCodecDevice(codec0);

  auto tracker = std::make_shared<DeviceTracker>(dispatcher(), /*detection_is_expected=*/false);
  RunLoopUntilIdle();
  ASSERT_EQ(0u, tracker->size());

  {
    // Create the detector, but expect no events at the DeviceTracker;
    auto device_detector = DeviceDetector::Create(tracker->handler(), tracker->idle_handler(),
                                                  dispatcher(), /*ignore_devices=*/true);
    RunLoopUntilIdle();
    ASSERT_EQ(tracker->size(), 0u) << "Pre-existing device detected while 'ignore_devices' set";
  }
}

// Verify that plug detector ignores subsequently-added audio devices, if `ignore_devices` is set.
TEST_F(DeviceDetectorTest, IgnoreDevicesDoesNotDetectHotplugDevices) {
  auto composite = std::make_shared<FakeAudioComposite>(dispatcher());
  auto codec = std::make_shared<FakeAudioCodec>(dispatcher());

  auto tracker = std::make_shared<DeviceTracker>(dispatcher(), /*detection_is_expected=*/false);
  {
    // Create the detector, but expect no events at the DeviceTracker;
    auto device_detector = DeviceDetector::Create(tracker->handler(), tracker->idle_handler(),
                                                  dispatcher(), /*ignore_devices=*/true);
    RunLoopUntilIdle();
    ASSERT_EQ(0u, tracker->size());

    // Hotplug a device of each type.
    [[maybe_unused]] auto dev0 = AddCompositeDevice(composite);
    [[maybe_unused]] auto dev1 = AddCodecDevice(codec);
    RunLoopUntilIdle();
    ASSERT_EQ(tracker->size(), 0u) << "Hot-plugged device detected while 'ignore_devices' set";
  }
}

}  // namespace
}  // namespace media_audio
