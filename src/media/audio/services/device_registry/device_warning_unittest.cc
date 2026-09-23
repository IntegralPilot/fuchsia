// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.audio.device/cpp/common_types.h>
#include <fidl/fuchsia.audio.device/cpp/natural_types.h>
#include <fidl/fuchsia.hardware.audio/cpp/common_types.h>
#include <fidl/fuchsia.hardware.audio/cpp/natural_types.h>

#include <gtest/gtest.h>

#include "src/media/audio/services/device_registry/common_unittest.h"
#include "src/media/audio/services/device_registry/device.h"
#include "src/media/audio/services/device_registry/device_unittest.h"
#include "src/media/audio/services/device_registry/validate.h"

namespace media_audio {
namespace {

namespace fad = fuchsia_audio_device;
namespace fha = fuchsia_hardware_audio;

class CodecWarningTest : public CodecTest {};
class CompositeWarningTest : public CompositeTest {
 protected:
  bool CreateRingBufferForTest(const std::shared_ptr<FakeComposite>& fake_driver,
                               const std::shared_ptr<Device>& device, ElementId rb_element_id) {
    if (!device->is_operational()) {
      return false;
    }
    if (!SetControl(device)) {
      return false;
    }

    auto ring_buffer_format_sets_by_element = ElementDriverRingBufferFormatSets(device);
    if (ring_buffer_format_sets_by_element.empty()) {
      return false;
    }
    if (device->ring_buffer_ids().size() != ring_buffer_format_sets_by_element.size()) {
      return false;
    }
    constexpr uint32_t reserved_ring_buffer_bytes = 8192;
    constexpr uint32_t requested_ring_buffer_bytes = 4000;

    fake_driver->ReserveRingBufferSize(rb_element_id, reserved_ring_buffer_bytes);
    auto safe_format = SafeDriverRingBufferFormatFromElementDriverRingBufferFormatSets(
        rb_element_id, ring_buffer_format_sets_by_element);

    bool callback_received = false;
    if (!device->CreateRingBuffer(
            rb_element_id, safe_format, requested_ring_buffer_bytes,
            [&callback_received](const fit::result<fad::ControlCreateRingBufferError,
                                                   Device::RingBufferInfo>& result) {
              ASSERT_TRUE(result.is_ok());
              callback_received = true;
            })) {
      return false;
    }

    RunLoopUntilIdle();
    return callback_received;
  }

  bool CreatePacketStreamForTest(const std::shared_ptr<Device>& device, ElementId element_id) {
    if (!device->is_operational()) {
      return false;
    }
    if (!IsControlled(device) && !SetControl(device)) {
      return false;
    }

    auto packet_stream_format_sets_by_element = ElementDriverPacketStreamFormatSets(device);
    if (packet_stream_format_sets_by_element.empty()) {
      return false;
    }

    auto safe_format =
        SafeDriverPacketStreamFormats(element_id, packet_stream_format_sets_by_element).front();

    bool callback_received = false;
    if (!device->CreatePacketStream(
            element_id, safe_format,
            [&callback_received](const fit::result<fad::ControlCreatePacketStreamError,
                                                   Device::PacketStreamInfo>& result) {
              ASSERT_TRUE(result.is_ok());
              callback_received = true;
            })) {
      return false;
    }

    RunLoopUntilIdle();
    if (!callback_received) {
      return false;
    }

    return callback_received;
  }

  // Creating a PacketStream should fail with `expected_status`.
  void ExpectCreatePacketStreamError(
      const std::shared_ptr<Device>& device, ElementId element_id,
      fuchsia_audio_device::ControlCreatePacketStreamError expected_status,
      const fuchsia_hardware_audio::Format2& format) {
    auto response_received = false;

    device->CreatePacketStream(
        element_id, format,
        [&response_received, expected_status](const fit::result<fad::ControlCreatePacketStreamError,
                                                                Device::PacketStreamInfo>& result) {
          ASSERT_TRUE(result.is_error());
          EXPECT_EQ(result.error_value(), expected_status);
          response_received = true;
        });

    RunLoopUntilIdle();
    ASSERT_TRUE(response_received);
  }

  // Creating a RingBuffer should fail with `expected_error`.
  void ExpectCreateRingBufferError(const std::shared_ptr<Device>& device, ElementId element_id,
                                   fad::ControlCreateRingBufferError expected_error,
                                   const fha::Format2& format,
                                   uint32_t requested_ring_buffer_bytes = 1024) {
    std::ostringstream stream;
    stream << "Validating CreateRingBuffer on element_id " << element_id << " with format "
           << format.pcm_format().value();
    SCOPED_TRACE(stream.str());

    auto response_received = false;
    auto error_received = fad::ControlCreateRingBufferError(0);

    EXPECT_FALSE(device->CreateRingBuffer(
        element_id, format, requested_ring_buffer_bytes,
        [&response_received, &error_received](
            fit::result<fad::ControlCreateRingBufferError, Device::RingBufferInfo> result) {
          ASSERT_TRUE(result.is_error());
          error_received = result.error_value();
          response_received = true;
        }));

    RunLoopUntilIdle();
    ASSERT_TRUE(response_received);
    EXPECT_EQ(error_received, expected_error);
  }
};

////////////////////
// Codec tests
//
TEST_F(CodecWarningTest, UnhealthyIsError) {
  auto fake_driver = MakeFakeCodecOutput();
  fake_driver->set_health_state(false);
  auto device = InitializeDeviceForFakeCodec(fake_driver);

  EXPECT_TRUE(device->has_error());
  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 1u);

  EXPECT_EQ(device_presence_watcher()->on_ready_count(), 0u);
  EXPECT_EQ(device_presence_watcher()->on_error_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CodecWarningTest, UnhealthyCanBeRemoved) {
  auto fake_driver = MakeFakeCodecInput();
  fake_driver->set_health_state(false);
  auto device = InitializeDeviceForFakeCodec(fake_driver);

  ASSERT_TRUE(device->has_error());
  ASSERT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  ASSERT_EQ(device_presence_watcher()->error_devices().size(), 1u);

  ASSERT_EQ(device_presence_watcher()->on_ready_count(), 0u);
  ASSERT_EQ(device_presence_watcher()->on_error_count(), 1u);
  ASSERT_EQ(device_presence_watcher()->on_removal_count(), 0u);

  fake_driver->DropCodec();
  RunLoopUntilIdle();

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);

  EXPECT_EQ(device_presence_watcher()->on_ready_count(), 0u);
  EXPECT_EQ(device_presence_watcher()->on_error_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_removal_from_error_count(), 1u);
}

TEST_F(CodecWarningTest, UnhealthyFailsSetControl) {
  auto fake_driver = MakeFakeCodecNoDirection();
  fake_driver->set_health_state(false);
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_TRUE(device->has_error());

  EXPECT_FALSE(SetControl(device));
}

TEST_F(CodecWarningTest, UnhealthyFailsAddObserver) {
  auto fake_driver = MakeFakeCodecOutput();
  fake_driver->set_health_state(false);
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_TRUE(device->has_error());

  EXPECT_FALSE(AddObserver(device));
}

TEST_F(CodecWarningTest, AlreadyControlledFailsSetControl) {
  auto fake_driver = MakeFakeCodecInput();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_TRUE(device->is_operational());

  EXPECT_TRUE(SetControl(device));

  EXPECT_FALSE(SetControl(device));
  EXPECT_TRUE(IsControlled(device));

  // Even though SetControl failed, the device should still be healthy and configurable.
  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);

  EXPECT_EQ(device_presence_watcher()->on_ready_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_error_count(), 0u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CodecWarningTest, AlreadyObservedFailsAddObserver) {
  auto fake_driver = MakeFakeCodecNoDirection();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(AddObserver(device));

  EXPECT_FALSE(AddObserver(device));

  // Even though AddObserver failed, the device should still be healthy and configurable.
  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);

  EXPECT_EQ(device_presence_watcher()->on_ready_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_error_count(), 0u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CodecWarningTest, CannotDropUnknownCodecControl) {
  auto fake_driver = MakeFakeCodecOutput();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_TRUE(device->is_operational());

  EXPECT_FALSE(DropControl(device));

  // Even though DropControl failed, the device should still be healthy and configurable.
  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);

  EXPECT_EQ(device_presence_watcher()->on_ready_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_error_count(), 0u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CodecWarningTest, CannotDropCodecControlTwice) {
  auto fake_driver = MakeFakeCodecInput();
  auto device = InitializeDeviceForFakeCodec(fake_driver);

  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));
  ASSERT_TRUE(DropControl(device));

  EXPECT_FALSE(DropControl(device));

  // Even though DropControl failed, the device should still be healthy and configurable.
  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);

  EXPECT_EQ(device_presence_watcher()->on_ready_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_error_count(), 0u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CodecWarningTest, WithoutControlFailsCallsThatRequireControl) {
  auto fake_driver = MakeFakeCodecInput();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_FALSE(notify()->dai_format());
  ASSERT_FALSE(notify()->codec_is_started());
  std::vector<fha::DaiSupportedFormats> dai_formats;
  GetDaiFormatSets(
      device, dai_id(),
      [&dai_formats](ElementId element_id, const std::vector<fha::DaiSupportedFormats>& formats) {
        EXPECT_EQ(element_id, fad::kDefaultDaiInterconnectElementId);
        dai_formats.push_back(formats[0]);
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(device->Reset());
  EXPECT_FALSE(device->CodecStop());
  device->SetDaiFormat(dai_id(), SafeDaiFormatFromDaiFormatSets(dai_formats));
  EXPECT_FALSE(device->CodecStart());

  RunLoopUntilIdle();
  EXPECT_FALSE(notify()->dai_format());
  EXPECT_FALSE(notify()->codec_is_started());
}

// SetDaiFormat with invalid formats: expect a warning.
TEST_F(CodecWarningTest, SetInvalidDaiFormat) {
  auto fake_driver = MakeFakeCodecNoDirection();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  ASSERT_EQ(device_presence_watcher()->error_devices().size(), 0u);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));
  std::vector<fha::DaiSupportedFormats> dai_formats;
  GetDaiFormatSets(
      device, dai_id(),
      [&dai_formats](ElementId element_id, const std::vector<fha::DaiSupportedFormats>& formats) {
        EXPECT_EQ(element_id, fad::kDefaultDaiInterconnectElementId);
        dai_formats.push_back(formats[0]);
      });

  RunLoopUntilIdle();
  auto invalid_dai_format = SecondDaiFormatFromDaiFormatSets(dai_formats);
  invalid_dai_format.bits_per_sample() = invalid_dai_format.bits_per_slot() + 1;

  device->SetDaiFormat(dai_id(), invalid_dai_format);

  RunLoopUntilIdle();
  EXPECT_FALSE(notify()->dai_format());
  auto error_notify = notify()->dai_format_errors().find(fad::kDefaultDaiInterconnectElementId);
  ASSERT_TRUE(error_notify != notify()->dai_format_errors().end());
  EXPECT_EQ(error_notify->second, fad::ControlSetDaiFormatError::kInvalidDaiFormat);

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);
}

TEST_F(CodecWarningTest, SetUnsupportedDaiFormat) {
  auto fake_driver = MakeFakeCodecNoDirection();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  ASSERT_EQ(device_presence_watcher()->error_devices().size(), 0u);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));
  std::vector<fha::DaiSupportedFormats> dai_formats;
  GetDaiFormatSets(device, dai_id(),
                   [&dai_formats](ElementId element_id,
                                  const std::vector<fha::DaiSupportedFormats>& format_sets) {
                     EXPECT_EQ(element_id, fad::kDefaultDaiInterconnectElementId);
                     for (const auto& format_set : format_sets) {
                       dai_formats.push_back(format_set);
                     }
                   });

  RunLoopUntilIdle();

  // Format is valid but unsupported. The call should fail, but the device should remain healthy.
  device->SetDaiFormat(dai_id(), UnsupportedDaiFormatFromDaiFormatSets(dai_formats));

  RunLoopUntilIdle();
  EXPECT_FALSE(notify()->dai_format());
  auto error_notify = notify()->dai_format_errors().find(fad::kDefaultDaiInterconnectElementId);
  ASSERT_TRUE(error_notify != notify()->dai_format_errors().end());
  EXPECT_EQ(error_notify->second, fad::ControlSetDaiFormatError::kFormatMismatch);

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);
}

TEST_F(CodecWarningTest, StartBeforeSetDaiFormat) {
  auto fake_driver = MakeFakeCodecNoDirection();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  ASSERT_EQ(device_presence_watcher()->error_devices().size(), 0u);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  // We expect this to fail, because the DaiFormat has not yet been set.
  EXPECT_FALSE(device->CodecStart());

  RunLoopUntilIdle();
  EXPECT_FALSE(notify()->codec_is_started());
  // We do expect the device to remain healthy and usable.
  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);
}

TEST_F(CodecWarningTest, StopBeforeSetDaiFormat) {
  auto fake_driver = MakeFakeCodecNoDirection();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  ASSERT_EQ(device_presence_watcher()->error_devices().size(), 0u);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  // We expect this to fail, because the DaiFormat has not yet been set.
  EXPECT_FALSE(device->CodecStop());

  RunLoopUntilIdle();
  // We do expect the device to remain healthy and usable.
  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);
}

TEST_F(CodecWarningTest, CreateRingBufferWrongDeviceType) {
  auto fake_driver = MakeFakeCodecNoDirection();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  ASSERT_EQ(device_presence_watcher()->error_devices().size(), 0u);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));
  auto format = fha::Format2::WithPcmFormat(fha::PcmFormat{{
      .number_of_channels = 1,
      .sample_format = fha::SampleFormat::kPcmSigned,
      .bytes_per_sample = 2u,
      .valid_bits_per_sample = 16,
      .frame_rate = 48000,
  }});
  int32_t min_bytes = 100;
  bool callback_received = false;
  auto received_error = fad::ControlCreateRingBufferError(0);

  // We expect this to fail, because a Codec cannot CreateRingBuffer.
  EXPECT_FALSE(device->CreateRingBuffer(
      0, format, min_bytes,
      [&callback_received, &received_error](
          fit::result<fad::ControlCreateRingBufferError, Device::RingBufferInfo> result) {
        ASSERT_TRUE(result.is_error());
        received_error = result.error_value();
        callback_received = true;
      }));

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_received);
  EXPECT_EQ(received_error, fad::ControlCreateRingBufferError::kWrongDeviceType);
  // We do expect the device to remain healthy and usable.
  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);
}

// GetTopologies on error device
// GetTopologies (unsupported by driver)

// GetElements on error device
// GetElements (unsupported by driver)

// WatchTopology on error device
// WatchTopology (unsupported by driver)

// WatchElementState on error device
// WatchElementState (unsupported by driver)

// SetTopology on error device
// SetTopology (unsupported by driver)
// SetTopology without a Control

// SetElementState on error device
// SetElementState (unsupported by driver)
// SetElementState without a Control

////////////////////
// Composite tests
//
TEST_F(CompositeWarningTest, UnhealthyIsError) {
  auto fake_driver = MakeFakeComposite();
  fake_driver->set_health_state(false);
  auto device = InitializeDeviceForFakeComposite(fake_driver);

  EXPECT_TRUE(device->has_error());
  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 1u);

  EXPECT_EQ(device_presence_watcher()->on_ready_count(), 0u);
  EXPECT_EQ(device_presence_watcher()->on_error_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CompositeWarningTest, CanRemoveUnhealthy) {
  auto fake_driver = MakeFakeComposite();
  fake_driver->set_health_state(false);
  auto device = InitializeDeviceForFakeComposite(fake_driver);

  ASSERT_TRUE(device->has_error());
  ASSERT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  ASSERT_EQ(device_presence_watcher()->error_devices().size(), 1u);

  ASSERT_EQ(device_presence_watcher()->on_ready_count(), 0u);
  ASSERT_EQ(device_presence_watcher()->on_error_count(), 1u);
  ASSERT_EQ(device_presence_watcher()->on_removal_count(), 0u);

  fake_driver->DropComposite();
  RunLoopUntilIdle();

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);

  EXPECT_EQ(device_presence_watcher()->on_ready_count(), 0u);
  EXPECT_EQ(device_presence_watcher()->on_error_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_removal_from_error_count(), 1u);
}

TEST_F(CompositeWarningTest, SetControlHealthy) {
  auto fake_driver = MakeFakeComposite();
  fake_driver->set_health_state(false);
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->has_error());

  EXPECT_FALSE(SetControl(device));
}

TEST_F(CompositeWarningTest, AddObserverUnhealthy) {
  auto fake_driver = MakeFakeComposite();
  fake_driver->set_health_state(false);
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->has_error());

  EXPECT_FALSE(AddObserver(device));
}

TEST_F(CompositeWarningTest, AlreadyControlledFailsSetControl) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());

  EXPECT_TRUE(SetControl(device));

  EXPECT_FALSE(SetControl(device));
  EXPECT_TRUE(IsControlled(device));

  // Even though SetControl failed, the device should still be healthy and configurable.
  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);

  EXPECT_EQ(device_presence_watcher()->on_ready_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_error_count(), 0u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CompositeWarningTest, AddObserverAlreadyObserved) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(AddObserver(device));

  EXPECT_FALSE(AddObserver(device));

  // Even though AddObserver failed, the device should still be healthy and configurable.
  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);

  EXPECT_EQ(device_presence_watcher()->on_ready_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_error_count(), 0u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CompositeWarningTest, DropControlUnknown) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());

  EXPECT_FALSE(DropControl(device));

  // Even though DropControl failed, the device should still be healthy and configurable.
  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);

  EXPECT_EQ(device_presence_watcher()->on_ready_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_error_count(), 0u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CompositeWarningTest, DropControlTwice) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);

  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));
  ASSERT_TRUE(DropControl(device));

  EXPECT_FALSE(DropControl(device));

  // Even though DropControl failed, the device should still be healthy and configurable.
  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);

  EXPECT_EQ(device_presence_watcher()->on_ready_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_error_count(), 0u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CompositeWarningTest, UnresponsiveInitializeBecomesError) {
  auto fake_driver = MakeFakeComposite();
  ASSERT_EQ(device_presence_watcher()->on_ready_count(), 0u);
  ASSERT_EQ(device_presence_watcher()->on_error_count(), 0u);
  ASSERT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  ASSERT_EQ(device_presence_watcher()->error_devices().size(), 0u);

  fake_driver->set_unresponsive();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  EXPECT_TRUE(device->has_error());

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 1u);

  EXPECT_EQ(device_presence_watcher()->on_ready_count(), 0u);
  EXPECT_EQ(device_presence_watcher()->on_error_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CompositeWarningTest, UnresponsiveGetHealthStateBecomesError) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  ASSERT_EQ(device_presence_watcher()->on_error_count(), 0u);

  fake_driver->set_unresponsive();
  RetrieveHealthState(device);

  RunLoopFor(ShortCmdTimeout());
  RunLoopUntilIdle();

  // This FIDL call, after the previous one timed out, triggers the device to be marked "in error".
  RetrieveHealthState(device);
  RunLoopUntilIdle();

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 1u);

  EXPECT_EQ(device_presence_watcher()->on_error_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CompositeWarningTest, UnresponsiveSetDaiFormatBecomesError) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto dai_format_sets_by_element = device->dai_format_sets();
  ASSERT_FALSE(dai_format_sets_by_element.empty());
  auto dai_id = *device->dai_ids().begin();
  auto safe_format = SafeDaiFormatFromElementDaiFormatSets(dai_id, dai_format_sets_by_element);
  ASSERT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  ASSERT_EQ(device_presence_watcher()->on_error_count(), 0u);

  fake_driver->set_unresponsive();
  device->SetDaiFormat(dai_id, safe_format);

  RunLoopFor(ShortCmdTimeout());
  RunLoopUntilIdle();

  // This FIDL call, after the previous one timed out, triggers the device to be marked "in error".
  RetrieveHealthState(device);
  RunLoopUntilIdle();

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 1u);

  EXPECT_EQ(device_presence_watcher()->on_error_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CompositeWarningTest, UnresponsiveResetBecomesError) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));
  ASSERT_EQ(device_presence_watcher()->on_ready_count(), 1u);
  ASSERT_FALSE(notify()->device_is_reset());
  ASSERT_EQ(device_presence_watcher()->on_error_count(), 0u);

  fake_driver->set_unresponsive();
  EXPECT_TRUE(device->Reset());

  RunLoopFor(ShortCmdTimeout());
  RunLoopUntilIdle();

  // This FIDL call, after the previous one timed out, triggers the device to be marked "in error".
  RetrieveHealthState(device);
  RunLoopUntilIdle();

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 1u);

  EXPECT_EQ(device_presence_watcher()->on_error_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CompositeWarningTest, UnresponsiveCreateRingBufferBecomesError) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto ring_buffer_format_sets_by_element = ElementDriverRingBufferFormatSets(device);
  ASSERT_FALSE(ring_buffer_format_sets_by_element.empty());
  ASSERT_EQ(device->ring_buffer_ids().size(), ring_buffer_format_sets_by_element.size());
  constexpr uint32_t reserved_ring_buffer_bytes = 8192;
  constexpr uint32_t requested_ring_buffer_bytes = 4000;

  auto element_id = *device->ring_buffer_ids().begin();

  fake_driver->ReserveRingBufferSize(element_id, reserved_ring_buffer_bytes);
  auto safe_format = SafeDriverRingBufferFormatFromElementDriverRingBufferFormatSets(
      element_id, ring_buffer_format_sets_by_element);

  fake_driver->set_unresponsive();
  bool callback_received = false;
  ASSERT_TRUE(device->CreateRingBuffer(
      element_id, safe_format, requested_ring_buffer_bytes,
      [&callback_received](
          const fit::result<fad::ControlCreateRingBufferError, Device::RingBufferInfo>& result) {
        FAIL() << "Unexpected CreateRingBuffer callback when unresponsive";
        callback_received = true;
      }));

  RunLoopFor(LongCmdTimeout());
  RunLoopUntilIdle();

  // This FIDL call, after the previous one timed out, triggers the device to be marked "in error".
  RetrieveHealthState(device);
  RunLoopUntilIdle();

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 1u);

  EXPECT_EQ(device_presence_watcher()->on_error_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CompositeWarningTest, UnresponsiveCreatePacketStreamBecomesError) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto packet_stream_format_sets_by_element = ElementDriverPacketStreamFormatSets(device);
  ASSERT_FALSE(packet_stream_format_sets_by_element.empty());
  ASSERT_EQ(device->packet_stream_ids().size(), packet_stream_format_sets_by_element.size());

  auto element_id = *device->packet_stream_ids().begin();
  auto safe_format =
      SafeDriverPacketStreamFormats(element_id, packet_stream_format_sets_by_element).front();

  fake_driver->set_unresponsive();
  bool callback_received = false;
  device->CreatePacketStream(
      element_id, safe_format,
      [&callback_received](const fit::result<fad::ControlCreatePacketStreamError,
                                             Device::PacketStreamInfo>& result) {
        FAIL() << "Unexpected CreatePacketStream callback when unresponsive";
        callback_received = true;
      });

  RunLoopFor(LongCmdTimeout());
  RunLoopUntilIdle();

  // This FIDL call, after the previous one timed out, triggers the device to be marked "in error".
  RetrieveHealthState(device);
  RunLoopUntilIdle();

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 1u);

  EXPECT_EQ(device_presence_watcher()->on_error_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CompositeWarningTest, UnresponsiveSetElementStateBecomesError) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  RunLoopUntilIdle();
  ASSERT_TRUE(notify()->element_states().contains(FakeComposite::kMuteElementId));
  notify()->clear_element_states();
  fuchsia_hardware_audio_signalprocessing::SettableElementState state{{
      .started = true,
      .bypassed = false,
  }};

  fake_driver->set_unresponsive();
  EXPECT_EQ(device->SetElementState(FakeComposite::kMuteElementId, state), ZX_OK);

  RunLoopFor(ShortCmdTimeout());
  RunLoopUntilIdle();

  // This FIDL call, after the previous one timed out, triggers the device to be marked "in error".
  RetrieveHealthState(device);
  RunLoopUntilIdle();

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 1u);

  EXPECT_EQ(device_presence_watcher()->on_error_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CompositeWarningTest, UnresponsiveSetTopologyBecomesError) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  RunLoopUntilIdle();
  ASSERT_TRUE(notify()->topology_id().has_value());
  TopologyId current_topology_id = *notify()->topology_id();

  TopologyId topology_id_to_set = 0;
  if (device->topology_ids().size() < 2) {
    GTEST_SKIP() << "Not enough topologies to run this test case";
  }
  for (auto t : device->topology_ids()) {
    if (t != current_topology_id) {
      topology_id_to_set = t;
      break;
    }
  }

  fake_driver->set_unresponsive();
  EXPECT_EQ(device->SetTopology(topology_id_to_set), ZX_OK);

  RunLoopFor(ShortCmdTimeout());
  RunLoopUntilIdle();

  // This FIDL call, after the previous one timed out, triggers the device to be marked "in error".
  RetrieveHealthState(device);
  RunLoopUntilIdle();

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 1u);

  EXPECT_EQ(device_presence_watcher()->on_error_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CompositeWarningTest, UnresponsiveSetActiveChannelsBecomesError) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  auto rb_element_id = *device->ring_buffer_ids().begin();
  ASSERT_TRUE(CreateRingBufferForTest(fake_driver, device, rb_element_id));

  bool callback_received = false;
  fake_driver->set_unresponsive();
  device->SetActiveChannels(rb_element_id, 0x0, [&callback_received](zx::result<zx::time> result) {
    EXPECT_TRUE(result.is_error());
    callback_received = true;
  });

  RunLoopFor(ShortCmdTimeout());
  RunLoopUntilIdle();

  // This FIDL call, after the previous one timed out, triggers the device to be marked "in error".
  RetrieveHealthState(device);
  RunLoopUntilIdle();

  EXPECT_FALSE(callback_received);
  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 1u);

  EXPECT_EQ(device_presence_watcher()->on_error_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CompositeWarningTest, UnresponsiveStartRingBufferBecomesError) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  auto rb_element_id = *device->ring_buffer_ids().begin();
  ASSERT_TRUE(CreateRingBufferForTest(fake_driver, device, rb_element_id));

  bool callback_received = false;
  fake_driver->set_unresponsive();
  device->StartRingBuffer(rb_element_id, [&callback_received](zx::result<zx::time> result) {
    EXPECT_TRUE(result.is_error());
    callback_received = true;
  });

  RunLoopFor(ShortCmdTimeout());
  RunLoopUntilIdle();

  // This FIDL call, after the previous one timed out, triggers the device to be marked "in error".
  RetrieveHealthState(device);
  RunLoopUntilIdle();

  EXPECT_FALSE(callback_received);
  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 1u);

  EXPECT_EQ(device_presence_watcher()->on_error_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CompositeWarningTest, UnresponsiveStopRingBufferBecomesError) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  auto rb_element_id = *device->ring_buffer_ids().begin();
  ASSERT_TRUE(CreateRingBufferForTest(fake_driver, device, rb_element_id));

  ASSERT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  ASSERT_EQ(device_presence_watcher()->error_devices().size(), 0u);

  bool callback_received = false;
  fake_driver->set_unresponsive();
  device->StopRingBuffer(rb_element_id, [&callback_received](zx_status_t status) {
    EXPECT_NE(status, ZX_OK);
    callback_received = true;
  });

  RunLoopFor(ShortCmdTimeout());
  RunLoopUntilIdle();

  // This FIDL call, after the previous one timed out, triggers the device to be marked "in error".
  RetrieveHealthState(device);
  RunLoopUntilIdle();

  EXPECT_FALSE(callback_received);
  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 1u);

  EXPECT_EQ(device_presence_watcher()->on_error_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CompositeWarningTest, UnresponsiveStartPacketStreamBecomesError) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  auto element_id = *device->packet_stream_ids().begin();
  fake_driver->InjectPacketStreamBufferTypes(element_id,
                                             fuchsia_hardware_audio::BufferType::kClientOwned |
                                                 fuchsia_hardware_audio::BufferType::kDriverOwned |
                                                 fuchsia_hardware_audio::BufferType::kInline);
  ASSERT_TRUE(CreatePacketStreamForTest(device, element_id));

  bool callback_received = false;
  fake_driver->set_unresponsive();
  device->StartPacketStream(element_id, [&callback_received](zx::result<> result) {
    EXPECT_TRUE(result.is_error());
    callback_received = true;
  });

  RunLoopFor(ShortCmdTimeout());
  RunLoopUntilIdle();

  // This FIDL call, after the previous one timed out, triggers the device to be marked "in error".
  RetrieveHealthState(device);
  RunLoopUntilIdle();

  EXPECT_FALSE(callback_received);
  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 1u);

  EXPECT_EQ(device_presence_watcher()->on_error_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CompositeWarningTest, UnresponsiveStopPacketStreamBecomesError) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  auto element_id = *device->packet_stream_ids().begin();
  ASSERT_TRUE(CreatePacketStreamForTest(device, element_id));

  ASSERT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  ASSERT_EQ(device_presence_watcher()->error_devices().size(), 0u);

  bool callback_received = false;
  fake_driver->set_unresponsive();
  device->StopPacketStream(element_id, [&callback_received](zx_status_t status) {
    EXPECT_NE(status, ZX_OK);
    callback_received = true;
  });

  RunLoopFor(ShortCmdTimeout());
  RunLoopUntilIdle();

  // This FIDL call, after the previous one timed out, triggers the device to be marked "in error".
  RetrieveHealthState(device);
  RunLoopUntilIdle();

  EXPECT_FALSE(callback_received);
  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 1u);

  EXPECT_EQ(device_presence_watcher()->on_error_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);
}

TEST_F(CompositeWarningTest, WithoutControlFailsCallsThatRequireControl) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(notify()->dai_formats().empty());
  ASSERT_TRUE(notify()->codec_format_infos().empty());
  ASSERT_TRUE(notify()->dai_format_errors().empty());
  uint32_t requested_ring_buffer_bytes = 4000;
  ASSERT_TRUE(notify()->dai_formats().empty());
  ASSERT_TRUE(notify()->codec_format_infos().empty());
  ASSERT_TRUE(notify()->dai_format_errors().empty());

  // All three of the primary Control methods (Reset, SetDaiFormat, CreateRingBuffer,
  // CreatePacketStream) should fail, if the Device is not controlled.
  EXPECT_FALSE(device->Reset());

  for (auto dai_id : device->dai_ids()) {
    const auto dai_format =
        SafeDaiFormatFromElementDaiFormatSets(dai_id, device->dai_format_sets());
    device->SetDaiFormat(dai_id, dai_format);
  }
  EXPECT_TRUE(notify()->dai_formats().empty());
  EXPECT_TRUE(notify()->codec_format_infos().empty());
  EXPECT_TRUE(notify()->dai_format_errors().empty());

  for (auto ring_buffer_id : device->ring_buffer_ids()) {
    auto callback_received = false;
    auto received_error = fad::ControlCreateRingBufferError(0);
    EXPECT_FALSE(device->CreateRingBuffer(
        ring_buffer_id,
        SafeDriverRingBufferFormatFromElementDriverRingBufferFormatSets(
            ring_buffer_id, ElementDriverRingBufferFormatSets(device)),
        requested_ring_buffer_bytes,
        [&callback_received, &received_error](
            fit::result<fad::ControlCreateRingBufferError, Device::RingBufferInfo> result) {
          ASSERT_TRUE(result.is_error());
          received_error = result.error_value();
          callback_received = true;
        }));

    RunLoopUntilIdle();
    EXPECT_TRUE(callback_received);
    EXPECT_EQ(received_error, fad::ControlCreateRingBufferError::kOther);
  }

  for (auto packet_stream_id : device->packet_stream_ids()) {
    auto callback_received = false;
    auto received_error = fad::ControlCreatePacketStreamError(0);
    EXPECT_FALSE(device->CreatePacketStream(
        packet_stream_id,
        SafeDriverPacketStreamFormats(packet_stream_id, ElementDriverPacketStreamFormatSets(device))
            .front(),
        [&callback_received, &received_error](
            fit::result<fad::ControlCreatePacketStreamError, Device::PacketStreamInfo> result) {
          ASSERT_TRUE(result.is_error());
          received_error = result.error_value();
          callback_received = true;
        }));

    RunLoopUntilIdle();
    EXPECT_TRUE(callback_received);
    EXPECT_EQ(received_error, fad::ControlCreatePacketStreamError::kOther);
  }

  for (auto packet_stream_id : device->packet_stream_ids()) {
    auto callback_received = false;
    auto received_error = fad::ControlCreatePacketStreamError(0);
    EXPECT_FALSE(device->CreatePacketStream(
        packet_stream_id,
        SafeDriverPacketStreamFormats(packet_stream_id, ElementDriverPacketStreamFormatSets(device))
            .front(),
        [&callback_received, &received_error](
            fit::result<fad::ControlCreatePacketStreamError, Device::PacketStreamInfo> result) {
          ASSERT_TRUE(result.is_error());
          received_error = result.error_value();
          callback_received = true;
        }));

    RunLoopUntilIdle();
    EXPECT_TRUE(callback_received);
    EXPECT_EQ(received_error, fad::ControlCreatePacketStreamError::kOther);
  }
}

TEST_F(CompositeWarningTest, SetDaiFormatWrongElementType) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto dai_id = *device->dai_ids().begin();
  auto safe_format = SafeDaiFormatFromElementDaiFormatSets(dai_id, device->dai_format_sets());
  notify()->clear_dai_formats();
  for (auto ring_buffer_id : device->ring_buffer_ids()) {
    device->SetDaiFormat(ring_buffer_id, safe_format);

    RunLoopUntilIdle();
    EXPECT_TRUE(notify()->dai_formats().empty());
    EXPECT_TRUE(notify()->codec_format_infos().empty());
    EXPECT_TRUE(
        ExpectDaiFormatError(ring_buffer_id, fad::ControlSetDaiFormatError::kInvalidElementId));
  }
}

// SetDaiFormat with invalid formats: expect a warning.
TEST_F(CompositeWarningTest, SetDaiFormatInvalidFormat) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  for (auto dai_id : device->dai_ids()) {
    notify()->clear_dai_formats();
    auto bad_format = SafeDaiFormatFromElementDaiFormatSets(dai_id, device->dai_format_sets());
    bad_format.channels_to_use_bitmask() = (1u << bad_format.number_of_channels());  // too high

    device->SetDaiFormat(dai_id, bad_format);

    RunLoopUntilIdle();
    EXPECT_TRUE(notify()->dai_formats().empty());
    EXPECT_TRUE(notify()->codec_format_infos().empty());
    EXPECT_TRUE(ExpectDaiFormatError(dai_id, fad::ControlSetDaiFormatError::kInvalidDaiFormat));
  }
}

TEST_F(CompositeWarningTest, SetDaiFormatUnsupportedFormat) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  for (auto dai_id : device->dai_ids()) {
    notify()->clear_dai_formats();
    auto unsupported_format =
        UnsupportedDaiFormatFromElementDaiFormatSets(dai_id, device->dai_format_sets());
    ASSERT_TRUE(ValidateDaiFormat(unsupported_format));

    device->SetDaiFormat(dai_id, unsupported_format);

    RunLoopUntilIdle();
    EXPECT_TRUE(notify()->dai_formats().empty());
    EXPECT_TRUE(notify()->codec_format_infos().empty());
    EXPECT_TRUE(ExpectDaiFormatError(dai_id, fad::ControlSetDaiFormatError::kFormatMismatch));
  }
}

TEST_F(CompositeWarningTest, CreateRingBufferInvalidElementId) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto ring_buffer_format_sets_by_element = ElementDriverRingBufferFormatSets(device);
  ASSERT_FALSE(ring_buffer_format_sets_by_element.empty());
  ASSERT_EQ(device->ring_buffer_ids().size(), ring_buffer_format_sets_by_element.size());
  auto ring_buffer_id = *device->ring_buffer_ids().begin();
  fake_driver->ReserveRingBufferSize(ring_buffer_id, 8192);
  auto safe_format = SafeDriverRingBufferFormatFromElementDriverRingBufferFormatSets(
      ring_buffer_id, ring_buffer_format_sets_by_element);

  ExpectCreateRingBufferError(device, -1, fad::ControlCreateRingBufferError::kInvalidElementId,
                              safe_format);
}

TEST_F(CompositeWarningTest, CreateRingBufferWrongElementType) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto ring_buffer_format_sets_by_element = ElementDriverRingBufferFormatSets(device);
  ASSERT_FALSE(ring_buffer_format_sets_by_element.empty());
  ASSERT_EQ(device->ring_buffer_ids().size(), ring_buffer_format_sets_by_element.size());
  auto ring_buffer_id = *device->ring_buffer_ids().begin();
  fake_driver->ReserveRingBufferSize(ring_buffer_id, 8192);
  auto safe_format = SafeDriverRingBufferFormatFromElementDriverRingBufferFormatSets(
      ring_buffer_id, ring_buffer_format_sets_by_element);

  for (auto dai_id : device->dai_ids()) {
    ExpectCreateRingBufferError(device, dai_id,
                                fad::ControlCreateRingBufferError::kInvalidElementId, safe_format);
  }
}

TEST_F(CompositeWarningTest, CreateRingBufferInvalidFormat) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto ring_buffer_format_sets_by_element = ElementDriverRingBufferFormatSets(device);
  ASSERT_FALSE(ring_buffer_format_sets_by_element.empty());
  ASSERT_EQ(device->ring_buffer_ids().size(), ring_buffer_format_sets_by_element.size());

  for (auto ring_buffer_id : device->ring_buffer_ids()) {
    fake_driver->ReserveRingBufferSize(ring_buffer_id, 8192);
    auto invalid_format = SafeDriverRingBufferFormatFromElementDriverRingBufferFormatSets(
        ring_buffer_id, ring_buffer_format_sets_by_element);
    invalid_format.pcm_format().value().number_of_channels(0);

    ExpectCreateRingBufferError(device, ring_buffer_id,
                                fad::ControlCreateRingBufferError::kInvalidFormat, invalid_format);
  }
}

TEST_F(CompositeWarningTest, CreateRingBufferUnsupportedFormat) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto ring_buffer_format_sets_by_element = ElementDriverRingBufferFormatSets(device);
  ASSERT_FALSE(ring_buffer_format_sets_by_element.empty());
  ASSERT_EQ(device->ring_buffer_ids().size(), ring_buffer_format_sets_by_element.size());

  for (auto ring_buffer_id : device->ring_buffer_ids()) {
    fake_driver->ReserveRingBufferSize(ring_buffer_id, 8192);
    auto unsupported_format = SafeDriverRingBufferFormatFromElementDriverRingBufferFormatSets(
        ring_buffer_id, ring_buffer_format_sets_by_element);
    unsupported_format.pcm_format().value().frame_rate(
        unsupported_format.pcm_format().value().frame_rate() - 1);

    ExpectCreateRingBufferError(device, ring_buffer_id,
                                fad::ControlCreateRingBufferError::kFormatMismatch,
                                unsupported_format);
  }
}

// CreateRingBufferSizeTooSmall test?

// CreateRingBufferSizeTooLarge test?

// Negative RingBufferProperties cases?
// Device with error

// Negative GetVmo cases?
// Device with error

// Negative SetActiveChannels cases.
// Device with error

// Negative RingBuffer Start cases.
// Device with error

// Negative RingBuffer Stop  cases.
// Device with error

// Negative WatchDelayInfo cases.
// Device with error

// Negative WatchClockRecoveryPositionInfo cases?
// Device with error

TEST_F(CompositeWarningTest, CreatePacketStreamInvalidElementId) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto packet_stream_format_sets_by_element = ElementDriverPacketStreamFormatSets(device);
  ASSERT_FALSE(packet_stream_format_sets_by_element.empty());
  auto element_id = *device->packet_stream_ids().begin();
  auto safe_format =
      SafeDriverPacketStreamFormats(element_id, packet_stream_format_sets_by_element).front();

  ExpectCreatePacketStreamError(device, -1, fad::ControlCreatePacketStreamError::kInvalidElementId,
                                safe_format);
}

TEST_F(CompositeWarningTest, CreatePacketStreamWrongElementType) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));
  auto packet_stream_format_sets_by_element = ElementDriverPacketStreamFormatSets(device);
  auto element_id = *device->packet_stream_ids().begin();
  auto safe_format =
      SafeDriverPacketStreamFormats(element_id, packet_stream_format_sets_by_element).front();

  for (auto rb_id : device->ring_buffer_ids()) {
    ExpectCreatePacketStreamError(
        device, rb_id, fad::ControlCreatePacketStreamError::kInvalidElementId, safe_format);
  }
  for (auto dai_id : device->dai_ids()) {
    ExpectCreatePacketStreamError(
        device, dai_id, fad::ControlCreatePacketStreamError::kInvalidElementId, safe_format);
  }
}

TEST_F(CompositeWarningTest, CreatePacketStreamInvalidFormat) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto packet_stream_format_sets_by_element = ElementDriverPacketStreamFormatSets(device);
  ASSERT_FALSE(packet_stream_format_sets_by_element.empty());

  for (auto element_id : device->packet_stream_ids()) {
    auto invalid_format =
        SafeDriverPacketStreamFormats(element_id, packet_stream_format_sets_by_element).front();
    if (invalid_format.pcm_format()) {
      invalid_format.pcm_format()->number_of_channels(0);
    } else {
      invalid_format.encoding()->decoded_channel_count(0);
    }

    ExpectCreatePacketStreamError(
        device, element_id, fad::ControlCreatePacketStreamError::kInvalidFormat, invalid_format);
  }
}

TEST_F(CompositeWarningTest, CreatePacketStreamUnsupportedFormat) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto packet_stream_format_sets_by_element = ElementDriverPacketStreamFormatSets(device);
  ASSERT_FALSE(packet_stream_format_sets_by_element.empty());

  for (auto element_id : device->packet_stream_ids()) {
    auto unsupported_format =
        SafeDriverPacketStreamFormats(element_id, packet_stream_format_sets_by_element).front();
    if (unsupported_format.pcm_format()) {
      unsupported_format.pcm_format()->frame_rate(unsupported_format.pcm_format()->frame_rate() -
                                                  1);
    } else {
      unsupported_format.encoding()->decoded_frame_rate(
          *unsupported_format.encoding()->decoded_frame_rate() - 1);
    }

    ExpectCreatePacketStreamError(device, element_id,
                                  fad::ControlCreatePacketStreamError::kFormatMismatch,
                                  unsupported_format);
  }
}

TEST_F(CompositeWarningTest, SetPacketStreamBuffersUnsupportedBufferType) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto element_id = *device->packet_stream_ids().begin();
  fake_driver->InjectPacketStreamBufferTypes(element_id, fha::BufferType::kDriverOwned);
  ASSERT_TRUE(CreatePacketStreamForTest(device, element_id));

  std::vector<fha::VmoInfo> vmo_infos_vec;
  fha::RegisterVmosConfig register_vmos_config{{
      .vmo_infos = std::move(vmo_infos_vec),
  }};

  bool callback_received = false;
  device->SetPacketStreamBuffers(
      element_id, fad::PacketStreamSetupVmoInfo::WithRegisterInfo(std::move(register_vmos_config)),
      [&callback_received](
          fit::result<fad::PacketStreamSetBufferError, fad::PacketStreamBuffers> result) {
        ASSERT_TRUE(result.is_error());
        EXPECT_EQ(result.error_value(), fad::PacketStreamSetBufferError::kBadStrategy);
        callback_received = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_received);
}

TEST_F(CompositeWarningTest, SetPacketStreamBuffersUnsupportedInlineOnly) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto element_id = *device->packet_stream_ids().begin();
  // Set the supported buffer type to inline only.
  fake_driver->InjectPacketStreamBufferTypes(element_id, fha::BufferType::kInline);
  ASSERT_TRUE(CreatePacketStreamForTest(device, element_id));

  fha::AllocateVmosConfig allocate_vmos_config{{
      .min_vmo_size = 8192,
      .vmo_count = 1,
  }};

  bool callback_received = false;
  device->SetPacketStreamBuffers(
      element_id, fad::PacketStreamSetupVmoInfo::WithAllocateInfo(std::move(allocate_vmos_config)),
      [&callback_received](
          fit::result<fad::PacketStreamSetBufferError, fad::PacketStreamBuffers> result) {
        ASSERT_TRUE(result.is_error());
        EXPECT_EQ(result.error_value(), fad::PacketStreamSetBufferError::kBadStrategy);
        callback_received = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_received);
}

TEST_F(CompositeWarningTest, StartPacketStreamUnconfigured) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto element_id = *device->packet_stream_ids().begin();
  // Ensure INLINE is not supported so that buffer configuration is required.
  fake_driver->InjectPacketStreamBufferTypes(element_id, fha::BufferType::kDriverOwned);
  ASSERT_TRUE(CreatePacketStreamForTest(device, element_id));

  bool callback_received = false;
  device->StartPacketStream(element_id, [&callback_received](zx::result<> result) {
    ASSERT_TRUE(result.is_error());
    EXPECT_EQ(result.error_value(), ZX_ERR_BAD_STATE);
    callback_received = true;
  });

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_received);
}

TEST_F(CompositeWarningTest, SetPacketStreamBuffersRegisterVmosInvalidVmoHandle) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  if (device->packet_stream_ids().empty()) {
    GTEST_SKIP() << "No PacketStream elements to test";
  }
  auto element_id = *device->packet_stream_ids().begin();
  ASSERT_TRUE(CreatePacketStreamForTest(device, element_id));

  std::vector<fha::VmoInfo> vmo_infos_vec;
  vmo_infos_vec.emplace_back(fha::VmoInfo{{
      .id = 0,
      .vmo = zx::vmo(),
  }});
  fha::RegisterVmosConfig register_vmos_config{{
      .vmo_infos = std::move(vmo_infos_vec),
  }};

  bool callback_received = false;
  device->SetPacketStreamBuffers(
      element_id, fad::PacketStreamSetupVmoInfo::WithRegisterInfo(std::move(register_vmos_config)),

      [&callback_received](
          fit::result<fad::PacketStreamSetBufferError, fad::PacketStreamBuffers> result) {
        ASSERT_TRUE(result.is_error());
        EXPECT_EQ(result.error_value(), fad::PacketStreamSetBufferError::kBadVmoConfig);
        callback_received = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_received);
}

TEST_F(CompositeWarningTest, SetPacketStreamBuffersRegisterVmosInsufficientRights) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  if (device->packet_stream_ids().empty()) {
    GTEST_SKIP() << "No PacketStream elements to test";
  }
  auto element_id = *device->packet_stream_ids().begin();
  ASSERT_TRUE(CreatePacketStreamForTest(device, element_id));

  zx::vmo vmo, insufficient_rights_vmo;
  ASSERT_EQ(zx::vmo::create(4096, 0, &vmo), ZX_OK);
  ASSERT_EQ(vmo.replace(ZX_RIGHT_READ | ZX_RIGHT_MAP, &insufficient_rights_vmo), ZX_OK);

  std::vector<fha::VmoInfo> vmo_infos;
  vmo_infos.push_back(fha::VmoInfo{{
      .id = 0,
      .vmo = std::move(insufficient_rights_vmo),
  }});

  fha::RegisterVmosConfig register_vmos_config{{
      .vmo_infos = std::move(vmo_infos),
  }};

  bool callback_received = false;
  device->SetPacketStreamBuffers(
      element_id, fad::PacketStreamSetupVmoInfo::WithRegisterInfo(std::move(register_vmos_config)),

      [&callback_received](
          fit::result<fad::PacketStreamSetBufferError, fad::PacketStreamBuffers> result) {
        ASSERT_TRUE(result.is_error());
        EXPECT_EQ(result.error_value(), fad::PacketStreamSetBufferError::kInsufficientRights);
        callback_received = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_received);
}

TEST_F(CompositeWarningTest, SetPacketStreamBuffersAllocateVmosDriverError) {
  auto fake_driver = MakeFakeComposite();
  auto element_id = FakeComposite::kSourcePsElementId;
  fake_driver->InjectPacketStreamBufferTypes(element_id, fha::BufferType::kDriverOwned);
  fake_driver->InjectPacketStreamAllocateVmosError(element_id, ZX_ERR_INTERNAL);
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  ASSERT_TRUE(CreatePacketStreamForTest(device, element_id));

  fha::AllocateVmosConfig allocate_vmos_config{{
      .min_vmo_size = 8192,
      .vmo_count = 1,
  }};

  bool callback_received = false;
  device->SetPacketStreamBuffers(
      element_id, fad::PacketStreamSetupVmoInfo::WithAllocateInfo(std::move(allocate_vmos_config)),

      [&callback_received](
          fit::result<fad::PacketStreamSetBufferError, fad::PacketStreamBuffers> result) {
        ASSERT_TRUE(result.is_error());
        EXPECT_EQ(result.error_value(), fad::PacketStreamSetBufferError::kDeviceError);
        callback_received = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_received);
  EXPECT_FALSE(device->is_operational());
}

TEST_F(CompositeWarningTest, SetPacketStreamBuffersRegisterVmosDriverError) {
  auto fake_driver = MakeFakeComposite();
  auto element_id = FakeComposite::kSourcePsElementId;
  fake_driver->InjectPacketStreamBufferTypes(element_id, fha::BufferType::kClientOwned);
  fake_driver->InjectPacketStreamRegisterVmosError(element_id, ZX_ERR_INTERNAL);
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  ASSERT_TRUE(CreatePacketStreamForTest(device, element_id));

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(8192, 0, &vmo), ZX_OK);

  std::vector<fha::VmoInfo> vmo_infos_vec;
  vmo_infos_vec.emplace_back(fha::VmoInfo{{
      .id = 0,
      .vmo = std::move(vmo),
  }});
  fha::RegisterVmosConfig register_vmos_config{{
      .vmo_infos = std::move(vmo_infos_vec),
  }};

  bool callback_received = false;
  device->SetPacketStreamBuffers(
      element_id, fad::PacketStreamSetupVmoInfo::WithRegisterInfo(std::move(register_vmos_config)),

      [&callback_received](
          fit::result<fad::PacketStreamSetBufferError, fad::PacketStreamBuffers> result) {
        ASSERT_TRUE(result.is_error());
        EXPECT_EQ(result.error_value(), fad::PacketStreamSetBufferError::kDeviceError);
        callback_received = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_received);
  EXPECT_FALSE(device->is_operational());
}

////////////////////////////////////////////////////////
// Signalprocessing test cases
//
// Negative cases for GetTopologies?
// GetTopologies on error device

// Negative cases for GetElements?
// GetElements on error device

// Negative cases for WatchTopology?
// WatchTopology on error device
// WatchTopology while pending

// Negative cases for WatchElementState
// WatchElementState on error device
// WatchElementState with unknown element_id
// WatchElementState while pending

// Negative cases for SetTopology
// SetTopology on error device
// SetTopology without Control.
// SetTopology with unknown topology_id
// Try pipelining a bunch of these calls without waiting and see if errors occur

// Negative cases for SetElementState
// SetElementState on error device
// SetElementState without Control
// SetElementState with unknown element_id
// SetElementState with invalid state
// SetElementState when the state can't be changed
// Try pipelining a bunch of these calls without waiting and see if errors occur

// Move these ideas to device_unittest.cc:
// SetTopology(no-change) should not generate a notification.
// SetElementState(no-change) should not generate a notification.
// Eventually, develop a mechanism in FakeComposite to disable signalprocessing, and test the six
// methods in that mode as well

}  // namespace
}  // namespace media_audio
