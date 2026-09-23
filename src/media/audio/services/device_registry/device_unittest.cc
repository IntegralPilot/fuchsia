// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/device.h"

#include <fidl/fuchsia.audio.device/cpp/common_types.h>
#include <fidl/fuchsia.audio/cpp/common_types.h>
#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/common_types.h>
#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/natural_types.h>
#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <lib/zx/clock.h>
#include <zircon/errors.h>

#include <optional>
#include <sstream>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/media/audio/services/device_registry/common_unittest.h"
#include "src/media/audio/services/device_registry/device_unittest.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_codec.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_composite.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_composite_packet_stream.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_composite_ring_buffer.h"
#include "src/media/audio/services/device_registry/validate.h"

namespace media_audio {

namespace fad = fuchsia_audio_device;
namespace fha = fuchsia_hardware_audio;
namespace fhasp = fuchsia_hardware_audio_signalprocessing;
namespace {

/////////////////////
// Codec tests
//
// Verify that a fake codec is initialized successfully.
TEST_F(CodecTest, Initialization) {
  auto fake_driver = MakeFakeCodecOutput();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  EXPECT_TRUE(device->is_operational());

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);

  EXPECT_EQ(device_presence_watcher()->on_ready_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_error_count(), 0u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);

  EXPECT_TRUE(device->is_codec());

  EXPECT_TRUE(device->has_codec_properties());
  EXPECT_TRUE(device->has_health_state());
  EXPECT_TRUE(device->dai_format_sets_retrieved());
  EXPECT_TRUE(device->has_plug_state());

  EXPECT_FALSE(device->checked_for_signalprocessing());
  EXPECT_FALSE(device->supports_signalprocessing());
  EXPECT_TRUE(device->info().has_value());
}

TEST_F(CodecTest, InitializationNoDirection) {
  auto fake_driver = MakeFakeCodecNoDirection();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  EXPECT_TRUE(device->is_operational());

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);

  EXPECT_EQ(device_presence_watcher()->on_ready_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_error_count(), 0u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);

  ASSERT_TRUE(device->info().has_value());
  EXPECT_FALSE(device->info()->is_input().has_value());
}

// Verify that a fake codec is initialized to the expected default values.
TEST_F(CodecTest, DeviceInfo) {
  auto fake_driver = MakeFakeCodecOutput();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_TRUE(device->is_operational());

  ASSERT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  ASSERT_EQ(device_presence_watcher()->on_ready_count(), 1u);

  ASSERT_TRUE(device->info().has_value());
  auto info = *device->info();

  EXPECT_TRUE(info.token_id().has_value());

  ASSERT_TRUE(info.device_type().has_value());
  EXPECT_EQ(*info.device_type(), fad::DeviceType::kCodec);

  ASSERT_TRUE(info.device_name().has_value());
  EXPECT_TRUE(!info.device_name()->empty());

  ASSERT_TRUE(info.manufacturer().has_value());
  EXPECT_EQ(*info.manufacturer(), FakeCodec::kDefaultManufacturer);

  ASSERT_TRUE(info.product().has_value());
  EXPECT_EQ(*info.product(), FakeCodec::kDefaultProduct);

  ASSERT_TRUE(info.unique_instance_id().has_value());
  EXPECT_EQ(*info.unique_instance_id(), FakeCodec::kDefaultUniqueInstanceId);

  ASSERT_TRUE(info.is_input().has_value());
  EXPECT_FALSE(*info.is_input());

  EXPECT_FALSE(info.ring_buffer_format_sets().has_value());

  ASSERT_TRUE(info.dai_format_sets().has_value());
  ASSERT_FALSE(info.dai_format_sets()->empty());
  ASSERT_TRUE(info.dai_format_sets()->at(0).format_sets().has_value());
  EXPECT_EQ(*info.dai_format_sets()->at(0).format_sets(), FakeCodec::kDefaultDaiFormatSets);

  ASSERT_TRUE(info.plug_detect_caps().has_value());
  EXPECT_EQ(*info.plug_detect_caps(), FakeCodec::kDefaultPlugCaps);

  EXPECT_FALSE(info.clock_domain().has_value());

  // signal_processing_elements is optional, but it can't be empty
  EXPECT_FALSE(info.signal_processing_elements().has_value() &&
               info.signal_processing_elements()->empty());

  // signal_processing_topologies is optional, but it can't be empty
  EXPECT_FALSE(info.signal_processing_topologies().has_value() &&
               info.signal_processing_topologies()->empty());
}

TEST_F(CodecTest, DistinctTokenIds) {
  auto fake_driver = MakeFakeCodecOutput();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_TRUE(device->is_operational());

  // Set up a second, entirely distinct fake device.
  auto [client, server] = fidl::Endpoints<fha::Codec>::Create();

  auto fake_driver2 =
      std::make_shared<FakeCodec>(server.TakeChannel(), client.TakeChannel(), dispatcher());
  fake_driver2->set_is_input(true);

  auto device2 = InitializeDeviceForFakeCodec(fake_driver2);
  EXPECT_TRUE(device2->is_operational());

  EXPECT_NE(device->token_id(), device2->token_id());

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 2u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);
}

// Verify that a driver's dropping the Codec causes a DeviceIsRemoved notification.
TEST_F(CodecTest, Disconnect) {
  auto fake_driver = MakeFakeCodecNoDirection();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  ASSERT_EQ(device_presence_watcher()->error_devices().size(), 0u);

  ASSERT_EQ(device_presence_watcher()->on_ready_count(), 1u);
  ASSERT_EQ(device_presence_watcher()->on_error_count(), 0u);
  ASSERT_EQ(device_presence_watcher()->on_removal_count(), 0u);

  fake_driver->DropCodec();

  RunLoopUntilIdle();
  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);

  EXPECT_EQ(device_presence_watcher()->on_ready_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_error_count(), 0u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_removal_from_ready_count(), 1u);
}

// Verify that a GetHealthState response of an empty struct is considered "healthy".
TEST_F(CodecTest, EmptyHealthResponse) {
  auto fake_driver = MakeFakeCodecOutput();
  fake_driver->set_health_state(std::nullopt);
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  EXPECT_TRUE(device->is_operational());

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);
}

TEST_F(CodecTest, GetClock) {
  auto fake_driver = MakeFakeCodecNoDirection();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_TRUE(device->is_operational());

  auto clock = device->GetReadOnlyClock();
  ASSERT_TRUE(clock.is_error());
  EXPECT_EQ(clock.error_value(), ZX_ERR_NOT_SUPPORTED);
}

TEST_F(CodecTest, Observer) {
  auto fake_driver = MakeFakeCodecNoDirection();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_TRUE(device->is_operational());

  EXPECT_TRUE(AddObserver(device));
}

TEST_F(CodecTest, Control) {
  auto fake_driver = MakeFakeCodecOutput();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  EXPECT_TRUE(DropControl(device));
}

// This tests the driver's ability to inform its ObserverNotify of initial plug state.
TEST_F(CodecTest, InitialPlugState) {
  auto fake_driver = MakeFakeCodecNoDirection();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(AddObserver(device));

  RunLoopUntilIdle();
  EXPECT_TRUE(device_plugged_state(device));
  ASSERT_TRUE(notify()->plug_state());
  EXPECT_EQ(notify()->plug_state()->first, fad::PlugState::kPlugged);
  EXPECT_EQ(notify()->plug_state()->second.get(), zx::time::infinite_past().get());
}

// This tests the driver's ability to originate plug changes, such as from jack detection. It also
// validates that plug notifications are delivered through ControlNotify (not just ObserverNotify).
TEST_F(CodecTest, DynamicPlugUpdate) {
  auto fake_driver = MakeFakeCodecOutput();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  RunLoopUntilIdle();
  EXPECT_TRUE(device_plugged_state(device));
  ASSERT_TRUE(notify()->plug_state());
  EXPECT_EQ(notify()->plug_state()->first, fad::PlugState::kPlugged);
  EXPECT_EQ(notify()->plug_state()->second.get(), zx::time::infinite_past().get());
  notify()->plug_state().reset();

  auto unplug_time = zx::clock::get_monotonic();
  fake_driver->InjectUnpluggedAt(unplug_time);
  RunLoopUntilIdle();
  EXPECT_FALSE(device_plugged_state(device));
  ASSERT_TRUE(notify()->plug_state());
  EXPECT_EQ(notify()->plug_state()->first, fad::PlugState::kUnplugged);
  EXPECT_EQ(notify()->plug_state()->second.get(), unplug_time.get());
}

TEST_F(CodecTest, GetDaiFormats) {
  auto fake_driver = MakeFakeCodecInput();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_FALSE(IsControlled(device));

  ASSERT_TRUE(AddObserver(device));

  bool received_get_dai_formats_callback = false;
  std::vector<fha::DaiSupportedFormats> dai_formats;
  GetDaiFormatSets(device, dai_id(),
                   [&received_get_dai_formats_callback, &dai_formats](
                       ElementId element_id, const std::vector<fha::DaiSupportedFormats>& formats) {
                     EXPECT_EQ(element_id, fad::kDefaultDaiInterconnectElementId);
                     received_get_dai_formats_callback = true;
                     for (auto& dai_format_set : formats) {
                       dai_formats.push_back(dai_format_set);
                     }
                   });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_get_dai_formats_callback);
  EXPECT_TRUE(ValidateDaiFormatSets(dai_formats));
}

// SetDaiFormat is tested (here and in CodecWarningTest) against all states and error cases:
// States: First set, format-change, no-change. ControlNotify::DaiFormatIsChanged received.
// SetDaiFormat stops Codec; ControlNotify::CodecIsStopped received if was started.
// Errors: wrong type; Device has error, not controlled; invalid format; unsupported format.
//
// SetDaiFormat - use the first supported format
TEST_F(CodecTest, SetDaiFormat) {
  auto fake_driver = MakeFakeCodecNoDirection();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_TRUE(device->is_operational());

  ASSERT_TRUE(SetControl(device));
  ASSERT_TRUE(IsControlled(device));

  bool received_get_dai_formats_callback = false;
  std::vector<fha::DaiSupportedFormats> dai_formats;
  GetDaiFormatSets(device, dai_id(),
                   [&received_get_dai_formats_callback, &dai_formats](
                       ElementId element_id, const std::vector<fha::DaiSupportedFormats>& formats) {
                     EXPECT_EQ(element_id, fad::kDefaultDaiInterconnectElementId);
                     received_get_dai_formats_callback = true;
                     for (auto& dai_format_set : formats) {
                       dai_formats.push_back(dai_format_set);
                     }
                   });
  RunLoopUntilIdle();
  ASSERT_TRUE(received_get_dai_formats_callback);
  ASSERT_FALSE(notify()->dai_format());
  device->SetDaiFormat(dai_id(), SafeDaiFormatFromDaiFormatSets(dai_formats));

  // check that notify received dai_format and codec_format_info
  RunLoopUntilIdle();
  EXPECT_TRUE(notify()->dai_format());
  EXPECT_TRUE(ValidateDaiFormat(*notify()->dai_format()));
  EXPECT_TRUE(notify()->codec_format_info(dai_id()));
  EXPECT_TRUE(ValidateCodecFormatInfo(*notify()->codec_format_info(dai_id())));
}

// Start and Stop
TEST_F(CodecTest, InitiallyStopped) {
  auto fake_driver = MakeFakeCodecOutput();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
  ASSERT_TRUE(device->is_operational());

  ASSERT_TRUE(SetControl(device));
  ASSERT_TRUE(IsControlled(device));

  bool received_get_dai_formats_callback = false;
  std::vector<fha::DaiSupportedFormats> dai_formats;
  GetDaiFormatSets(device, dai_id(),
                   [&received_get_dai_formats_callback, &dai_formats](
                       ElementId element_id, const std::vector<fha::DaiSupportedFormats>& formats) {
                     EXPECT_EQ(element_id, fad::kDefaultDaiInterconnectElementId);
                     received_get_dai_formats_callback = true;
                     for (auto& dai_format_set : formats) {
                       dai_formats.push_back(dai_format_set);
                     }
                   });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_get_dai_formats_callback);
  device->SetDaiFormat(dai_id(), SafeDaiFormatFromDaiFormatSets(dai_formats));

  RunLoopUntilIdle();
  ASSERT_TRUE(notify()->dai_format());
  ASSERT_TRUE(notify()->codec_is_stopped());

  // This should do nothing since we are already stopped.
  EXPECT_TRUE(device->CodecStop());

  RunLoopUntilIdle();
  EXPECT_TRUE(notify()->codec_is_stopped());
  // This demonstrates that there was no change as a result of the Stop call.
  EXPECT_EQ(notify()->codec_stop_time()->get(), zx::time::infinite_past().get());
}

TEST_F(CodecTest, Start) {
  auto fake_driver = MakeFakeCodecInput();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
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
  device->SetDaiFormat(dai_id(), SafeDaiFormatFromDaiFormatSets(dai_formats));

  RunLoopUntilIdle();
  ASSERT_TRUE(notify()->dai_format());
  ASSERT_TRUE(notify()->codec_is_stopped());
  auto time_before_start = zx::clock::get_monotonic();

  EXPECT_TRUE(device->CodecStart());

  RunLoopUntilIdle();
  EXPECT_TRUE(notify()->codec_is_started());
  EXPECT_GT(notify()->codec_start_time()->get(), time_before_start.get());
}

TEST_F(CodecTest, SetDaiFormatChange) {
  auto fake_driver = MakeFakeCodecNoDirection();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
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
  device->SetDaiFormat(dai_id(), SafeDaiFormatFromDaiFormatSets(dai_formats));

  RunLoopUntilIdle();
  ASSERT_TRUE(notify()->dai_format());
  ASSERT_TRUE(device->CodecStart());

  RunLoopUntilIdle();
  ASSERT_TRUE(notify()->codec_is_started());
  auto dai_format2 = SecondDaiFormatFromDaiFormatSets(dai_formats);
  auto time_before_format_change = zx::clock::get_monotonic();

  // Change the DaiFormat
  device->SetDaiFormat(dai_id(), dai_format2);

  // Expect codec to be stopped, and format to be changed.
  RunLoopUntilIdle();
  EXPECT_TRUE(notify()->codec_is_stopped());
  EXPECT_GT(notify()->codec_stop_time()->get(), time_before_format_change.get());
  EXPECT_TRUE(notify()->dai_format());
  EXPECT_EQ(*notify()->dai_format(), dai_format2);
  EXPECT_TRUE(notify()->codec_format_info(dai_id()));
}

TEST_F(CodecTest, SetDaiFormatNoChange) {
  auto fake_driver = MakeFakeCodecNoDirection();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
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
  auto safe_format = SafeDaiFormatFromDaiFormatSets(dai_formats);
  device->SetDaiFormat(dai_id(), safe_format);

  RunLoopUntilIdle();
  ASSERT_TRUE(notify()->dai_format());
  ASSERT_TRUE(device->CodecStart());

  RunLoopUntilIdle();
  ASSERT_TRUE(notify()->codec_is_started());
  auto time_after_started = zx::clock::get_monotonic();
  // Clear out our notify's DaiFormat so we can detect a new notification of the same DaiFormat.
  notify()->clear_dai_format(dai_id());

  device->SetDaiFormat(dai_id(), safe_format);

  RunLoopUntilIdle();
  // We do not expect this to reset our Start state.
  EXPECT_TRUE(notify()->codec_is_started());
  EXPECT_LT(notify()->codec_start_time()->get(), time_after_started.get());
  // We do not expect to be notified of a format change -- even a "change" to the same format.
  EXPECT_FALSE(notify()->dai_format());
}

TEST_F(CodecTest, StartStop) {
  auto fake_driver = MakeFakeCodecNoDirection();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
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
  device->SetDaiFormat(dai_id(), SafeDaiFormatFromDaiFormatSets(dai_formats));

  RunLoopUntilIdle();
  ASSERT_TRUE(notify()->dai_format());
  ASSERT_TRUE(notify()->codec_is_stopped());
  ASSERT_TRUE(device->CodecStart());

  RunLoopUntilIdle();
  auto time_before_stop = zx::clock::get_monotonic();
  ASSERT_TRUE(notify()->codec_is_started());
  ASSERT_TRUE(device->CodecStop());

  RunLoopUntilIdle();
  EXPECT_TRUE(notify()->codec_is_stopped());
  EXPECT_GT(notify()->codec_stop_time()->get(), time_before_stop.get());
}

// Start when already started: no notification; old start_time.
TEST_F(CodecTest, StartStart) {
  auto fake_driver = MakeFakeCodecOutput();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
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
  device->SetDaiFormat(dai_id(), SafeDaiFormatFromDaiFormatSets(dai_formats));

  RunLoopUntilIdle();
  ASSERT_TRUE(notify()->dai_format());
  ASSERT_TRUE(notify()->codec_is_stopped());
  ASSERT_TRUE(device->CodecStart());

  RunLoopUntilIdle();
  ASSERT_TRUE(notify()->codec_is_started());
  auto previous_start_time = *notify()->codec_start_time();
  ASSERT_TRUE(device->CodecStart());

  // Should not get a new notification here.
  RunLoopUntilIdle();
  EXPECT_TRUE(notify()->codec_is_started());
  EXPECT_EQ(notify()->codec_start_time()->get(), previous_start_time.get());
}

// Stop when already stopped: no notification; old stop_time.
TEST_F(CodecTest, StopStop) {
  auto fake_driver = MakeFakeCodecInput();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
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
  device->SetDaiFormat(dai_id(), SafeDaiFormatFromDaiFormatSets(dai_formats));

  RunLoopUntilIdle();
  ASSERT_TRUE(notify()->dai_format());
  ASSERT_TRUE(notify()->codec_is_stopped());
  // First Start the Codec so we can explicitly transition into the Stop state.
  ASSERT_TRUE(device->CodecStart());

  RunLoopUntilIdle();
  ASSERT_TRUE(notify()->codec_is_started());
  ASSERT_TRUE(device->CodecStop());

  RunLoopUntilIdle();
  ASSERT_TRUE(notify()->codec_is_stopped());
  auto previous_stop_time = *notify()->codec_stop_time();

  // Since we are already stopped, this should have no effect.
  EXPECT_TRUE(device->CodecStop());

  RunLoopUntilIdle();
  EXPECT_TRUE(notify()->codec_is_stopped());
  // This demonstrates that there was no change as a result of the Stop call.
  EXPECT_EQ(notify()->codec_stop_time()->get(), previous_stop_time.get());
}

// Reset stops the Codec and resets DaiFormat, Elements and Topology
TEST_F(CodecTest, Reset) {
  auto fake_driver = MakeFakeCodecNoDirection();
  auto device = InitializeDeviceForFakeCodec(fake_driver);
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
  device->SetDaiFormat(dai_id(), SafeDaiFormatFromDaiFormatSets(dai_formats));

  RunLoopUntilIdle();
  ASSERT_TRUE(notify()->dai_format());
  ASSERT_TRUE(notify()->codec_format_info(dai_id()));
  ASSERT_TRUE(notify()->codec_is_stopped());
  ASSERT_TRUE(device->CodecStart());

  // TODO(https://fxbug.dev/323270827): implement signalprocessing for Codec (topology, gain).
  // When implemented in FakeCodec, set a signalprocessing Topology, and change a signalprocessing
  // Element, and observe that both of these changes are reverted by the call to Reset.

  RunLoopUntilIdle();
  ASSERT_FALSE(notify()->device_is_reset());
  EXPECT_TRUE(notify()->codec_is_started());
  // Verify that the signalprocessing Topology has in fact changed.
  // Verify that the signalprocessing Element has in fact changed.
  auto time_before_reset = zx::clock::get_monotonic();

  EXPECT_TRUE(device->Reset());

  RunLoopUntilIdle();
  EXPECT_TRUE(notify()->codec_is_stopped());
  // We were notified that the Codec stopped.
  EXPECT_GT(notify()->codec_stop_time()->get(), time_before_reset.get());
  // We were notified that the Codec reset its DaiFormat (none is now set).
  EXPECT_FALSE(notify()->dai_format());
  EXPECT_FALSE(notify()->codec_format_info(dai_id()));

  // Observe the signalprocessing Elements - were they reset?
  // Observe the signalprocessing Topology - was it reset?

  EXPECT_TRUE(notify()->device_is_reset());
}

/////////////////////
// Composite tests
//
// Verify that a fake composite is initialized successfully.
TEST_F(CompositeTest, Initialization) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  EXPECT_TRUE(device->is_operational());

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);

  EXPECT_EQ(device_presence_watcher()->on_ready_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_error_count(), 0u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 0u);

  EXPECT_TRUE(device->is_composite());

  ASSERT_TRUE(device->info().has_value());
}

// Verify that a fake composite is initialized to the expected default values,
// for the most basic device info fields.
TEST_F(CompositeTest, DeviceInfo) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  ASSERT_EQ(device_presence_watcher()->on_ready_count(), 1u);

  ASSERT_TRUE(device->info().has_value());
  auto info = *device->info();

  EXPECT_TRUE(info.token_id().has_value());

  ASSERT_TRUE(info.device_type().has_value());
  EXPECT_EQ(*info.device_type(), fad::DeviceType::kComposite);

  ASSERT_TRUE(info.device_name().has_value());
  EXPECT_FALSE(info.device_name()->empty());

  ASSERT_TRUE(info.manufacturer().has_value());
  EXPECT_EQ(*info.manufacturer(), FakeComposite::kDefaultManufacturer);

  ASSERT_TRUE(info.product().has_value());
  EXPECT_EQ(*info.product(), FakeComposite::kDefaultProduct);

  ASSERT_TRUE(info.unique_instance_id().has_value());
  EXPECT_EQ(*info.unique_instance_id(), FakeComposite::kDefaultUniqueInstanceId);

  EXPECT_FALSE(info.is_input().has_value());

  EXPECT_FALSE(info.plug_detect_caps().has_value());

  ASSERT_TRUE(info.clock_domain().has_value());
  EXPECT_EQ(*info.clock_domain(), FakeComposite::kDefaultClockDomain);
}

// Verify that a fake composite is initialized to the expected default RingBuffer format sets.
TEST_F(CompositeTest, DeviceInfoRingBufferFormats) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  ASSERT_EQ(device_presence_watcher()->on_ready_count(), 1u);

  ASSERT_TRUE(device->info().has_value());
  auto info = *device->info();

  // Validate ring buffer format sets
  ASSERT_TRUE(info.ring_buffer_format_sets().has_value());
  ASSERT_FALSE(info.ring_buffer_format_sets()->empty());
  ASSERT_EQ(info.ring_buffer_format_sets()->size(), 2u);
  ASSERT_TRUE(info.ring_buffer_format_sets()->at(0).element_id().has_value());
  ASSERT_TRUE(info.ring_buffer_format_sets()->at(1).element_id().has_value());
  ASSERT_TRUE(
      *info.ring_buffer_format_sets()->at(0).element_id() == FakeComposite::kSourceRbElementId ||
      *info.ring_buffer_format_sets()->at(0).element_id() == FakeComposite::kDestRbElementId);
  ASSERT_TRUE(
      *info.ring_buffer_format_sets()->at(1).element_id() == FakeComposite::kSourceRbElementId ||
      *info.ring_buffer_format_sets()->at(1).element_id() == FakeComposite::kDestRbElementId);

  auto& source_rb_format_sets =
      *info.ring_buffer_format_sets()->at(0).element_id() == FakeComposite::kSourceRbElementId
          ? info.ring_buffer_format_sets()->at(0).format_sets()
          : info.ring_buffer_format_sets()->at(1).format_sets();
  ASSERT_TRUE(source_rb_format_sets.has_value());
  ASSERT_FALSE(source_rb_format_sets->empty());
  EXPECT_EQ(source_rb_format_sets->size(), 1u);
  ASSERT_TRUE(source_rb_format_sets->at(0).frame_rates().has_value());
  EXPECT_THAT(*source_rb_format_sets->at(0).frame_rates(),
              testing::ElementsAreArray(FakeComposite::kDefaultRbFrameRates2));
  ASSERT_TRUE(source_rb_format_sets->at(0).channel_sets().has_value());
  ASSERT_FALSE(source_rb_format_sets->at(0).channel_sets()->empty());
  EXPECT_EQ(source_rb_format_sets->at(0).channel_sets()->size(), 1u);
  ASSERT_TRUE(source_rb_format_sets->at(0).channel_sets()->at(0).attributes().has_value());
  ASSERT_FALSE(source_rb_format_sets->at(0).channel_sets()->at(0).attributes()->empty());
  EXPECT_EQ(source_rb_format_sets->at(0).channel_sets()->at(0).attributes()->size(), 1u);
  ASSERT_FALSE(source_rb_format_sets->at(0)
                   .channel_sets()
                   ->at(0)
                   .attributes()
                   ->at(0)
                   .max_frequency()
                   .has_value());
  ASSERT_TRUE(source_rb_format_sets->at(0)
                  .channel_sets()
                  ->at(0)
                  .attributes()
                  ->at(0)
                  .min_frequency()
                  .has_value());
  EXPECT_EQ(*source_rb_format_sets->at(0).channel_sets()->at(0).attributes()->at(0).min_frequency(),
            FakeComposite::kDefaultChannelAttributes2MinFrequency);

  auto& dest_rb_format_sets =
      *info.ring_buffer_format_sets()->at(1).element_id() == FakeComposite::kDestRbElementId
          ? info.ring_buffer_format_sets()->at(1).format_sets()
          : info.ring_buffer_format_sets()->at(0).format_sets();
  ASSERT_TRUE(dest_rb_format_sets.has_value());
  ASSERT_FALSE(dest_rb_format_sets->empty());
  EXPECT_EQ(dest_rb_format_sets->size(), 1u);
  ASSERT_TRUE(dest_rb_format_sets->at(0).frame_rates().has_value());
  EXPECT_THAT(*dest_rb_format_sets->at(0).frame_rates(),
              testing::ElementsAreArray(FakeComposite::kDefaultRbFrameRates1));
  ASSERT_TRUE(dest_rb_format_sets->at(0).channel_sets().has_value());
  ASSERT_FALSE(dest_rb_format_sets->at(0).channel_sets()->empty());
  EXPECT_EQ(dest_rb_format_sets->at(0).channel_sets()->size(), 1u);
  ASSERT_TRUE(dest_rb_format_sets->at(0).channel_sets()->at(0).attributes().has_value());
  ASSERT_FALSE(dest_rb_format_sets->at(0).channel_sets()->at(0).attributes()->empty());
  EXPECT_EQ(dest_rb_format_sets->at(0).channel_sets()->at(0).attributes()->size(), 1u);
  ASSERT_TRUE(dest_rb_format_sets->at(0)
                  .channel_sets()
                  ->at(0)
                  .attributes()
                  ->at(0)
                  .max_frequency()
                  .has_value());
  EXPECT_EQ(*dest_rb_format_sets->at(0).channel_sets()->at(0).attributes()->at(0).max_frequency(),
            FakeComposite::kDefaultChannelAttributes1MaxFrequency);
  ASSERT_TRUE(dest_rb_format_sets->at(0)
                  .channel_sets()
                  ->at(0)
                  .attributes()
                  ->at(0)
                  .min_frequency()
                  .has_value());
  EXPECT_EQ(*dest_rb_format_sets->at(0).channel_sets()->at(0).attributes()->at(0).min_frequency(),
            FakeComposite::kDefaultChannelAttributes1MinFrequency);
}

// Verify that a fake composite is initialized to the expected default PacketStream format sets.
TEST_F(CompositeTest, DeviceInfoPacketStreamFormats) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  ASSERT_EQ(device_presence_watcher()->on_ready_count(), 1u);

  ASSERT_TRUE(device->info().has_value());
  auto info = *device->info();

  // Validate packet stream format sets
  ASSERT_TRUE(info.packet_stream_format_sets().has_value());
  ASSERT_FALSE(info.packet_stream_format_sets()->empty());
  ASSERT_EQ(info.packet_stream_format_sets()->size(), 3u);
  ASSERT_TRUE(info.packet_stream_format_sets()->at(0).element_id().has_value());
  ASSERT_TRUE(info.packet_stream_format_sets()->at(1).element_id().has_value());
  ASSERT_TRUE(info.packet_stream_format_sets()->at(2).element_id().has_value());

  std::optional<std::vector<fuchsia_audio_device::PacketStreamSupportedFormats>>
      source_ps_format_sets;
  std::optional<std::vector<fuchsia_audio_device::PacketStreamSupportedFormats>>
      dest_ps_format_sets;
  std::optional<std::vector<fuchsia_audio_device::PacketStreamSupportedFormats>>
      both_ps_format_sets;

  for (const auto& format_set : *info.packet_stream_format_sets()) {
    ASSERT_TRUE(format_set.element_id().has_value());
    if (*format_set.element_id() == FakeComposite::kSourcePsElementId) {
      source_ps_format_sets = format_set.format_sets();
    } else if (*format_set.element_id() == FakeComposite::kDestPsElementId) {
      dest_ps_format_sets = format_set.format_sets();
    } else if (*format_set.element_id() == FakeComposite::kSourceDualSupportPsElementId) {
      both_ps_format_sets = format_set.format_sets();
    }
  }

  auto verify_pcm = [](const fad::PcmFormatSet& pcm_format,
                       const std::vector<uint32_t>& expected_frame_rates) {
    ASSERT_TRUE(pcm_format.frame_rates().has_value());
    EXPECT_THAT(*pcm_format.frame_rates(), testing::ElementsAreArray(expected_frame_rates));
  };

  auto verify_encoding = [](const fha::SupportedEncodings& encodings,
                            const fha::SupportedEncodings& expected_encodings) {
    ASSERT_TRUE(encodings.encoding_types().has_value());
    EXPECT_THAT(*encodings.encoding_types(),
                testing::ElementsAreArray(*expected_encodings.encoding_types()));
    ASSERT_TRUE(encodings.decoded_channel_sets().has_value());
    EXPECT_EQ(encodings.decoded_channel_sets()->size(),
              expected_encodings.decoded_channel_sets()->size());
  };

  ASSERT_TRUE(source_ps_format_sets.has_value());
  ASSERT_FALSE(source_ps_format_sets->empty());
  EXPECT_EQ(source_ps_format_sets->size(), 1u);
  ASSERT_TRUE(source_ps_format_sets->at(0).supported_encodings().has_value());
  verify_encoding(source_ps_format_sets->at(0).supported_encodings().value(),
                  FakeComposite::kDefaultEncodingSet2);

  ASSERT_TRUE(dest_ps_format_sets.has_value());
  ASSERT_FALSE(dest_ps_format_sets->empty());
  EXPECT_EQ(dest_ps_format_sets->size(), 2u);
  bool dest_pcm1_found = false;
  bool dest_pcm2_found = false;
  for (const auto& format_set : *dest_ps_format_sets) {
    ASSERT_TRUE(format_set.pcm_format().has_value());
    if (format_set.pcm_format()->frame_rates() == FakeComposite::kDefaultRbFrameRates1) {
      dest_pcm1_found = true;
    } else if (format_set.pcm_format()->frame_rates() == FakeComposite::kDefaultRbFrameRates3) {
      dest_pcm2_found = true;
      EXPECT_THAT(*format_set.pcm_format()->sample_types(),
                  testing::ElementsAre(fuchsia_audio::SampleType::kFloat32));
    }
  }
  EXPECT_TRUE(dest_pcm1_found);
  EXPECT_TRUE(dest_pcm2_found);

  ASSERT_TRUE(both_ps_format_sets.has_value());
  ASSERT_FALSE(both_ps_format_sets->empty());
  EXPECT_EQ(both_ps_format_sets->size(), 2u);
  bool pcm_found = false;
  bool encoding_found = false;
  for (const auto& format_set : *both_ps_format_sets) {
    if (format_set.pcm_format().has_value()) {
      verify_pcm(format_set.pcm_format().value(), FakeComposite::kDefaultPsFrameRates1);
      pcm_found = true;
    } else if (format_set.supported_encodings().has_value()) {
      verify_encoding(format_set.supported_encodings().value(),
                      FakeComposite::kDefaultEncodingSet2);
      encoding_found = true;
    }
  }
  EXPECT_TRUE(pcm_found);
  EXPECT_TRUE(encoding_found);
}

// Verify that a fake composite is initialized to the expected default Dai format sets.
TEST_F(CompositeTest, DeviceInfoDaiFormats) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  ASSERT_EQ(device_presence_watcher()->on_ready_count(), 1u);

  ASSERT_TRUE(device->info().has_value());
  auto info = *device->info();

  // Validate DAI format sets
  ASSERT_TRUE(info.dai_format_sets().has_value());
  ASSERT_FALSE(info.dai_format_sets()->empty());
  ASSERT_EQ(info.dai_format_sets()->size(), 2u);
  ASSERT_TRUE(info.dai_format_sets()->at(0).element_id().has_value());
  ASSERT_TRUE(info.dai_format_sets()->at(1).element_id().has_value());
  ASSERT_TRUE(*info.dai_format_sets()->at(0).element_id() == FakeComposite::kSourceDaiElementId ||
              *info.dai_format_sets()->at(0).element_id() == FakeComposite::kDestDaiElementId);
  ASSERT_TRUE(*info.dai_format_sets()->at(1).element_id() == FakeComposite::kSourceDaiElementId ||
              *info.dai_format_sets()->at(1).element_id() == FakeComposite::kDestDaiElementId);

  auto& source_dai_format_sets =
      *info.dai_format_sets()->at(0).element_id() == FakeComposite::kSourceDaiElementId
          ? info.dai_format_sets()->at(0).format_sets()
          : info.dai_format_sets()->at(1).format_sets();
  ASSERT_TRUE(source_dai_format_sets.has_value());
  ASSERT_EQ(source_dai_format_sets->size(), 1u);
  EXPECT_THAT(source_dai_format_sets->at(0).number_of_channels(),
              testing::ElementsAreArray(FakeComposite::kDefaultDaiNumberOfChannelsSet));
  EXPECT_THAT(source_dai_format_sets->at(0).frame_rates(),
              testing::ElementsAreArray(FakeComposite::kDefaultDaiFrameRateSet));
  EXPECT_THAT(source_dai_format_sets->at(0).bits_per_slot(),
              testing::ElementsAreArray(FakeComposite::kDefaultDaiBitsPerSlotSet));
  EXPECT_THAT(source_dai_format_sets->at(0).bits_per_sample(),
              testing::ElementsAreArray(FakeComposite::kDefaultDaiBitsPerSampleSet));
  EXPECT_THAT(source_dai_format_sets->at(0).frame_formats(),
              testing::ElementsAreArray(FakeComposite::kDefaultDaiFrameFormatSet));
  EXPECT_THAT(source_dai_format_sets->at(0).sample_formats(),
              testing::ElementsAreArray(FakeComposite::kDefaultDaiSampleFormatSet));

  auto& dest_dai_format_sets =
      *info.dai_format_sets()->at(1).element_id() == FakeComposite::kDestDaiElementId
          ? info.dai_format_sets()->at(1).format_sets()
          : info.dai_format_sets()->at(0).format_sets();
  ASSERT_TRUE(dest_dai_format_sets.has_value());
  ASSERT_EQ(dest_dai_format_sets->size(), 1u);
  EXPECT_THAT(dest_dai_format_sets->at(0).number_of_channels(),
              testing::ElementsAreArray(FakeComposite::kDefaultDaiNumberOfChannelsSet2));
  EXPECT_THAT(dest_dai_format_sets->at(0).frame_rates(),
              testing::ElementsAreArray(FakeComposite::kDefaultDaiFrameRateSet2));
  EXPECT_THAT(dest_dai_format_sets->at(0).bits_per_slot(),
              testing::ElementsAreArray(FakeComposite::kDefaultDaiBitsPerSlotSet2));
  EXPECT_THAT(dest_dai_format_sets->at(0).bits_per_sample(),
              testing::ElementsAreArray(FakeComposite::kDefaultDaiBitsPerSampleSet2));
  EXPECT_THAT(dest_dai_format_sets->at(0).frame_formats(),
              testing::ElementsAreArray(FakeComposite::kDefaultDaiFrameFormatSet2));
  EXPECT_THAT(dest_dai_format_sets->at(0).sample_formats(),
              testing::ElementsAreArray(FakeComposite::kDefaultDaiSampleFormatSet2));
}

// Verify that a fake composite is initialized to the expected default signalprocessing elements.
TEST_F(CompositeTest, DeviceInfoSignalProcessingElements) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  ASSERT_EQ(device_presence_watcher()->on_ready_count(), 1u);

  ASSERT_TRUE(device->info().has_value());
  auto info = *device->info();

  ASSERT_TRUE(info.signal_processing_elements().has_value());
  ASSERT_FALSE(info.signal_processing_elements()->empty());
  EXPECT_EQ(info.signal_processing_elements()->size(), FakeComposite::kElements.size());

  // info.signal_processing_elements() is a vector of Elements.
  // We expect them in the same order as FakeComposite::kElements.
  for (size_t i = 0; i < FakeComposite::kElements.size(); ++i) {
    SCOPED_TRACE(std::string("Testing element index ") + std::to_string(i));
    auto& element = info.signal_processing_elements()->at(i);
    auto& expected_element = FakeComposite::kElements.at(i);

    ASSERT_TRUE(element.id().has_value());
    EXPECT_EQ(*element.id(), *expected_element.id());

    ASSERT_TRUE(element.type().has_value());
    EXPECT_EQ(*element.type(), *expected_element.type());

    if (expected_element.type_specific().has_value()) {
      ASSERT_TRUE(element.type_specific().has_value());
      EXPECT_EQ(*element.type_specific(), *expected_element.type_specific());
    } else {
      EXPECT_FALSE(element.type_specific().has_value());
    }

    if (expected_element.description().has_value()) {
      ASSERT_TRUE(element.description().has_value());
      EXPECT_EQ(*element.description(), *expected_element.description());
    } else {
      EXPECT_FALSE(element.description().has_value());
    }

    if (expected_element.can_stop().has_value()) {
      ASSERT_TRUE(element.can_stop().has_value());
      EXPECT_EQ(*element.can_stop(), *expected_element.can_stop());
    } else {
      EXPECT_FALSE(element.can_stop().has_value());
    }

    if (expected_element.can_bypass().has_value()) {
      ASSERT_TRUE(element.can_bypass().has_value());
      EXPECT_EQ(*element.can_bypass(), *expected_element.can_bypass());
    } else {
      EXPECT_FALSE(element.can_bypass().has_value());
    }
  }
}

// Verify that a fake composite is initialized to the expected default signalprocessing topologies.
TEST_F(CompositeTest, DeviceInfoSignalProcessingTopologies) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  ASSERT_EQ(device_presence_watcher()->on_ready_count(), 1u);

  ASSERT_TRUE(device->info().has_value());
  auto info = *device->info();

  ASSERT_TRUE(info.signal_processing_topologies().has_value());
  ASSERT_FALSE(info.signal_processing_topologies()->empty());
  EXPECT_EQ(info.signal_processing_topologies()->size(), FakeComposite::kTopologies.size());
  ASSERT_TRUE(info.signal_processing_topologies()->at(0).id().has_value());
  EXPECT_EQ(*info.signal_processing_topologies()->at(0).id(), FakeComposite::kInputOnlyTopologyId);
  ASSERT_TRUE(
      info.signal_processing_topologies()->at(0).processing_elements_edge_pairs().has_value());
  EXPECT_THAT(*info.signal_processing_topologies()->at(0).processing_elements_edge_pairs(),
              testing::ElementsAreArray(
                  *FakeComposite::kInputOnlyTopology.processing_elements_edge_pairs()));
}

TEST_F(CompositeTest, DistinctTokenIds) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());

  // Set up a second, entirely distinct fake device.
  auto [client, server] = fidl::Endpoints<fha::Composite>::Create();

  auto fake_driver2 =
      std::make_shared<FakeComposite>(server.TakeChannel(), client.TakeChannel(), dispatcher());

  auto device2 = InitializeDeviceForFakeComposite(fake_driver2);
  EXPECT_TRUE(device2->is_operational());

  EXPECT_NE(device->token_id(), device2->token_id());

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 2u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);
}

// Verify that a driver's dropping the Composite causes a DeviceIsRemoved notification.
TEST_F(CompositeTest, Disconnect) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  ASSERT_EQ(device_presence_watcher()->error_devices().size(), 0u);

  ASSERT_EQ(device_presence_watcher()->on_ready_count(), 1u);
  ASSERT_EQ(device_presence_watcher()->on_error_count(), 0u);
  ASSERT_EQ(device_presence_watcher()->on_removal_count(), 0u);

  fake_driver->DropComposite();
  RunLoopUntilIdle();

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 0u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);

  EXPECT_EQ(device_presence_watcher()->on_ready_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_error_count(), 0u);
  EXPECT_EQ(device_presence_watcher()->on_removal_count(), 1u);
  EXPECT_EQ(device_presence_watcher()->on_removal_from_ready_count(), 1u);
}

// Verify that a GetHealthState response of an empty struct is considered "healthy".
TEST_F(CompositeTest, EmptyHealthResponse) {
  auto fake_driver = MakeFakeComposite();
  fake_driver->set_health_state(std::nullopt);
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  EXPECT_TRUE(device->is_operational());

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);
}

TEST_F(CompositeTest, DefaultClock) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());

  EXPECT_EQ(device_clock(device)->domain(), fha::kClockDomainMonotonic);
  EXPECT_TRUE(device_clock(device)->IdenticalToMonotonicClock());
  EXPECT_FALSE(device_clock(device)->adjustable());

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);
}

TEST_F(CompositeTest, ClockInOtherDomain) {
  const uint32_t kNonMonotonicClockDomain = fha::kClockDomainMonotonic + 1;
  auto fake_driver = MakeFakeComposite();
  fake_driver->set_clock_domain(kNonMonotonicClockDomain);
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());

  EXPECT_EQ(device_clock(device)->domain(), kNonMonotonicClockDomain);
  EXPECT_TRUE(device_clock(device)->IdenticalToMonotonicClock());
  EXPECT_TRUE(device_clock(device)->adjustable());

  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);
}

TEST_F(CompositeTest, Observer) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());

  EXPECT_TRUE(AddObserver(device));
}

// This tests the driver's ability to inform its ObserverNotify of initial signalprocessing state.
TEST_F(CompositeTest, InitialSignalProcessingForObserver) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(AddObserver(device));

  RunLoopUntilIdle();
  EXPECT_TRUE(notify()->topology_id().has_value())
      << "ObserverNotify was not notified of initial TopologyId";
  EXPECT_FALSE(notify()->element_states().empty());

  ASSERT_TRUE(device->info().has_value());
  ASSERT_TRUE(device->info()->signal_processing_elements().has_value());
  EXPECT_FALSE(device->info()->signal_processing_elements()->empty());
  EXPECT_EQ(notify()->element_states().size(),
            device->info()->signal_processing_elements()->size());
}

TEST_F(CompositeTest, Control) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  EXPECT_TRUE(DropControl(device));
}

// This tests the driver's ability to inform its ControlNotify of initial signalprocessing state.
TEST_F(CompositeTest, InitialSignalProcessingForControl) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  RunLoopUntilIdle();
  EXPECT_TRUE(notify()->topology_id().has_value())
      << "ObserverNotify was not notified of initial TopologyId";
  EXPECT_FALSE(notify()->element_states().empty());

  ASSERT_TRUE(device->info().has_value());
  ASSERT_TRUE(device->info()->signal_processing_elements().has_value());
  EXPECT_FALSE(device->info()->signal_processing_elements()->empty());
  EXPECT_EQ(notify()->element_states().size(),
            device->info()->signal_processing_elements()->size());
}

// This tests whether a Composite driver notifies Observers of initial plug state. (It shouldn't.)
TEST_F(CompositeTest, NoInitialPlugState) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(AddObserver(device));

  RunLoopUntilIdle();
  EXPECT_FALSE(notify()->plug_state().has_value())
      << "ObserverNotify was notified of initial PlugState";
}

// Verify setting initial DAI format, with ControlNotify receiving appropriate notification.
TEST_F(CompositeTest, SetDaiFormat) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto dai_format_sets_by_element = device->dai_format_sets();
  ASSERT_FALSE(dai_format_sets_by_element.empty());
  for (auto dai_id : device->dai_ids()) {
    notify()->clear_dai_formats();
    auto safe_format = SafeDaiFormatFromElementDaiFormatSets(dai_id, dai_format_sets_by_element);

    device->SetDaiFormat(dai_id, safe_format);

    RunLoopUntilIdle();
    // check that notify received dai_format for this element_id
    EXPECT_TRUE(ExpectDaiFormatMatches(dai_id, safe_format));
    EXPECT_TRUE(notify()->codec_format_infos().empty());
    EXPECT_TRUE(notify()->dai_format_errors().empty());
  }
}

// After a DAI format is set, setting it to the SAME format should have no effect.
TEST_F(CompositeTest, SetDaiFormatNoChange) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto dai_format_sets_by_element = device->dai_format_sets();
  ASSERT_FALSE(dai_format_sets_by_element.empty());

  for (auto dai_id : device->dai_ids()) {
    notify()->clear_dai_formats();
    auto safe_format = SafeDaiFormatFromElementDaiFormatSets(dai_id, dai_format_sets_by_element);
    device->SetDaiFormat(dai_id, safe_format);

    RunLoopUntilIdle();
    auto format_match = notify()->dai_formats().find(dai_id);
    ASSERT_NE(format_match, notify()->dai_formats().end());
    ASSERT_TRUE(format_match->second.has_value());
    ASSERT_EQ(format_match->second, safe_format);
    ASSERT_TRUE(notify()->codec_format_infos().empty());
    ASSERT_TRUE(notify()->dai_format_errors().empty());
    notify()->clear_dai_format(dai_id);

    device->SetDaiFormat(dai_id, safe_format);

    RunLoopUntilIdle();
    ASSERT_FALSE(notify()->dai_format_errors().empty());
    auto error_match = notify()->dai_format_errors().find(dai_id);
    ASSERT_NE(error_match, notify()->dai_format_errors().end());
    EXPECT_EQ(error_match->second, fad::ControlSetDaiFormatError(0));
    format_match = notify()->dai_formats().find(dai_id);
    EXPECT_EQ(format_match, notify()->dai_formats().end());
    EXPECT_TRUE(notify()->codec_format_infos().empty());
  }
}

// After a DAI format is set, CHANGING it should generate the appropriate notifications.
TEST_F(CompositeTest, SetDaiFormatChange) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto dai_format_sets_by_element = device->dai_format_sets();
  ASSERT_FALSE(dai_format_sets_by_element.empty());

  for (auto dai_id : device->dai_ids()) {
    notify()->clear_dai_formats();
    auto safe_format = SafeDaiFormatFromElementDaiFormatSets(dai_id, dai_format_sets_by_element);
    auto safe_format2 = SecondDaiFormatFromElementDaiFormatSets(dai_id, dai_format_sets_by_element);
    device->SetDaiFormat(dai_id, safe_format);

    RunLoopUntilIdle();
    auto format_match = notify()->dai_formats().find(dai_id);
    ASSERT_NE(format_match, notify()->dai_formats().end());
    ASSERT_TRUE(format_match->second.has_value());
    EXPECT_TRUE(ValidateDaiFormat(*format_match->second));
    EXPECT_TRUE(notify()->codec_format_infos().empty());
    EXPECT_TRUE(notify()->dai_format_errors().empty());
    notify()->clear_dai_format(dai_id);

    device->SetDaiFormat(dai_id, safe_format2);

    RunLoopUntilIdle();
    // SetDaiFormat with no-change should cause a DAI element to emit a not-set notification with 0.
    format_match = notify()->dai_formats().find(dai_id);
    EXPECT_TRUE(ExpectDaiFormatMatches(dai_id, safe_format2));
    EXPECT_TRUE(notify()->codec_format_infos().empty());
    EXPECT_TRUE(notify()->dai_format_errors().empty());
  }
}

// Reset stops/releases all RingBuffers and resets all Dais / Elements / Topology.
TEST_F(CompositeTest, Reset) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  // Set DAI formats on every DAI element.
  auto dai_format_sets_by_element = device->dai_format_sets();
  ASSERT_EQ(device->dai_ids().size(), dai_format_sets_by_element.size());
  for (auto dai_id : device->dai_ids()) {
    auto safe_format = SafeDaiFormatFromElementDaiFormatSets(dai_id, dai_format_sets_by_element);
    device->SetDaiFormat(dai_id, safe_format);

    RunLoopUntilIdle();
    ASSERT_TRUE(ExpectDaiFormatMatches(dai_id, safe_format));
  }

  // Create RingBuffers on every RingBuffer element.
  auto ring_buffer_format_sets_by_element = ElementDriverRingBufferFormatSets(device);
  ASSERT_EQ(device->ring_buffer_ids().size(), ring_buffer_format_sets_by_element.size());
  for (auto element_id : device->ring_buffer_ids()) {
    fake_driver->ReserveRingBufferSize(element_id, 4096);
    auto safe_format = SafeDriverRingBufferFormatFromElementDriverRingBufferFormatSets(
        element_id, ring_buffer_format_sets_by_element);

    std::ostringstream stream;
    stream << "Validating CreateRingBuffer on element_id " << element_id << " with format "
           << safe_format.pcm_format().value();
    SCOPED_TRACE(stream.str());

    uint32_t requested_ring_buffer_bytes = 2000;
    auto callback_received = false;
    Device::RingBufferInfo rb_info;

    ASSERT_TRUE(device->CreateRingBuffer(
        element_id, safe_format, requested_ring_buffer_bytes,
        [&callback_received](
            const fit::result<fad::ControlCreateRingBufferError, Device::RingBufferInfo>& result) {
          EXPECT_TRUE(result.is_ok());
          callback_received = true;
        }));

    RunLoopUntilIdle();
    ASSERT_TRUE(callback_received);
  }
  EXPECT_EQ(FakeCompositeRingBuffer::count(), device->ring_buffer_ids().size());

  // Create PacketStreams on every PacketStream element.
  auto packet_stream_format_sets_by_element = ElementDriverPacketStreamFormatSets(device);
  ASSERT_EQ(device->packet_stream_ids().size(), packet_stream_format_sets_by_element.size());
  for (auto element_id : device->packet_stream_ids()) {
    for (auto safe_format :
         SafeDriverPacketStreamFormats(element_id, packet_stream_format_sets_by_element)) {
      std::ostringstream stream;
      stream << "Validating CreatePacketStream on element_id " << element_id << " with format "
             << (safe_format.pcm_format() ? "PCM" : "Encoding");
      SCOPED_TRACE(stream.str());

      TestCreatePacketStream(device, element_id, safe_format);
      TestAllocatePacketStreamBuffers(device, element_id);
    }
  }
  EXPECT_EQ(FakeCompositePacketStream::count(), device->packet_stream_ids().size());

  ASSERT_FALSE(notify()->device_is_reset());

  EXPECT_TRUE(device->Reset());

  RunLoopUntilIdle();
  // Reset should cause every DAI element to emit DaiFormatIsChanged(id, std::nullopt, std::nullopt)
  // So notify()->dai_formats() should contain an entry for each Dai element, of value std::nullopt.
  EXPECT_EQ(notify()->dai_formats().size(), device->dai_ids().size());
  for (const auto& [element_id, format] : notify()->dai_formats()) {
    EXPECT_FALSE(format.has_value()) << "{element_id " << element_id << "}";
  }
  EXPECT_TRUE(notify()->codec_format_infos().empty());
  EXPECT_TRUE(notify()->dai_format_errors().empty());

  // Expect any RingBuffers to drop, eventually. Our Control should still be valid though.
  EXPECT_EQ(FakeCompositeRingBuffer::count(), 0u);

  // Expect any PacketStreams to drop, eventually. Our Control should still be valid though.
  EXPECT_EQ(FakeCompositePacketStream::count(), 0u);

  // Expect TopologyIsChanged if revert-to-default represents a topology change.

  // Expect ElementStateIsChanged for any element where revert-to-default represents a state change.

  EXPECT_TRUE(notify()->device_is_reset());
}
}  // namespace

// RingBuffer test cases
//
// Creating a RingBuffer should succeed.
void CompositeTest::TestCreateRingBuffer(const std::shared_ptr<Device>& device,
                                         ElementId element_id, const fha::Format2& safe_format) {
  std::ostringstream stream;
  stream << "Validating CreateRingBuffer on element_id " << element_id << " with format "
         << safe_format.pcm_format().value();
  SCOPED_TRACE(stream.str());

  uint32_t requested_ring_buffer_bytes = 4000;
  auto callback_received = false;
  Device::RingBufferInfo rb_info;

  EXPECT_TRUE(device->CreateRingBuffer(
      element_id, safe_format, requested_ring_buffer_bytes,
      [&callback_received,
       &rb_info](fit::result<fad::ControlCreateRingBufferError, Device::RingBufferInfo> result) {
        ASSERT_TRUE(result.is_ok());
        rb_info = std::move(*result);
        callback_received = true;
      }));

  RunLoopUntilIdle();
  ASSERT_TRUE(callback_received);

  ASSERT_TRUE(rb_info.properties.turn_on_delay().has_value());
  EXPECT_GE(*rb_info.properties.turn_on_delay(), 0);

  ASSERT_TRUE(rb_info.properties.valid_bits_per_sample().has_value());
  EXPECT_LE(*rb_info.properties.valid_bits_per_sample(),
            safe_format.pcm_format()->bytes_per_sample() * 8);

  ASSERT_TRUE(rb_info.ring_buffer.buffer().has_value());
  EXPECT_GT(rb_info.ring_buffer.buffer()->size(), requested_ring_buffer_bytes);
  EXPECT_TRUE(rb_info.ring_buffer.buffer()->vmo().is_valid());

  ASSERT_TRUE(rb_info.ring_buffer.consumer_bytes().has_value());
  EXPECT_GT(*rb_info.ring_buffer.consumer_bytes(), 0u);
  ASSERT_TRUE(rb_info.ring_buffer.producer_bytes().has_value());
  EXPECT_GT(*rb_info.ring_buffer.producer_bytes(), 0u);

  EXPECT_TRUE(*rb_info.ring_buffer.consumer_bytes() >= requested_ring_buffer_bytes ||
              *rb_info.ring_buffer.producer_bytes() >= requested_ring_buffer_bytes);

  ASSERT_TRUE(rb_info.ring_buffer.format()->channel_count().has_value());
  EXPECT_EQ(*rb_info.ring_buffer.format()->channel_count(),
            safe_format.pcm_format()->number_of_channels());

  ASSERT_TRUE(rb_info.ring_buffer.format()->sample_type().has_value());

  ASSERT_TRUE(rb_info.ring_buffer.format()->frames_per_second().has_value());
  EXPECT_EQ(*rb_info.ring_buffer.format()->frames_per_second(),
            safe_format.pcm_format()->frame_rate());

  ASSERT_TRUE(rb_info.ring_buffer.reference_clock().has_value());
  EXPECT_TRUE(rb_info.ring_buffer.reference_clock()->is_valid());

  ASSERT_TRUE(rb_info.ring_buffer.reference_clock_domain().has_value());
  EXPECT_EQ(*rb_info.ring_buffer.reference_clock_domain(), fha::kClockDomainMonotonic);

  // Now client can drop its RingBuffer connection.
  device->DropRingBuffer(element_id);
}

namespace {

TEST_F(CompositeTest, CreateRingBuffers) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto ring_buffer_format_sets_by_element = ElementDriverRingBufferFormatSets(device);
  ASSERT_FALSE(ring_buffer_format_sets_by_element.empty());
  ASSERT_EQ(device->ring_buffer_ids().size(), ring_buffer_format_sets_by_element.size());

  for (auto element_id : device->ring_buffer_ids()) {
    fake_driver->ReserveRingBufferSize(element_id, 8192);

    auto safe_format = SafeDriverRingBufferFormatFromElementDriverRingBufferFormatSets(
        element_id, ring_buffer_format_sets_by_element);

    TestCreateRingBuffer(device, element_id, safe_format);
  }
}

// Tests on the Composite RingBuffer interface
//
// Maybe, rather than testing the entire CreateRingBuffer meta-method, we should test the individual
// component-level methods: ConnectRingBufferFidl, RetrieveRingBufferProperties, GetVmo.

// Verify that we get DeviceDroppedRingBuffer notifications
TEST_F(CompositeTest, DeviceDroppedRingBuffer) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto ring_buffer_format_sets_by_element = ElementDriverRingBufferFormatSets(device);
  ASSERT_FALSE(ring_buffer_format_sets_by_element.empty());
  ASSERT_EQ(device->ring_buffer_ids().size(), ring_buffer_format_sets_by_element.size());
  constexpr uint32_t reserved_ring_buffer_bytes = 8192;
  constexpr uint32_t requested_ring_buffer_bytes = 4000;

  for (auto element_id : device->ring_buffer_ids()) {
    SCOPED_TRACE(std::string("Testing DropRingBuffer: element_id ") + std::to_string(element_id));
    fake_driver->ReserveRingBufferSize(element_id, reserved_ring_buffer_bytes);
    auto safe_format = SafeDriverRingBufferFormatFromElementDriverRingBufferFormatSets(
        element_id, ring_buffer_format_sets_by_element);
    auto callback_received = false;
    Device::RingBufferInfo rb_info;
    ASSERT_TRUE(device->CreateRingBuffer(
        element_id, safe_format, requested_ring_buffer_bytes,
        [&callback_received](
            const fit::result<fad::ControlCreateRingBufferError, Device::RingBufferInfo>& result) {
          EXPECT_TRUE(result.is_ok());
          callback_received = true;
        }));

    RunLoopUntilIdle();
    ASSERT_TRUE(callback_received);
    ASSERT_TRUE(RingBufferIsStopped(device, element_id));

    fake_driver->DropRingBuffer(element_id);

    RunLoopUntilIdle();
    EXPECT_FALSE(HasRingBuffer(device, element_id));
    EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
    EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);
  }
}

// Verify Start and Stop
TEST_F(CompositeTest, RingBufferStartAndStop) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto ring_buffer_format_sets_by_element = ElementDriverRingBufferFormatSets(device);
  ASSERT_FALSE(ring_buffer_format_sets_by_element.empty());
  ASSERT_EQ(device->ring_buffer_ids().size(), ring_buffer_format_sets_by_element.size());
  constexpr uint32_t reserved_ring_buffer_bytes = 8192;
  constexpr uint32_t requested_ring_buffer_bytes = 4000;

  for (auto element_id : device->ring_buffer_ids()) {
    SCOPED_TRACE(std::string("Testing RingBuffer Start/Stop: element_id ") +
                 std::to_string(element_id));
    fake_driver->ReserveRingBufferSize(element_id, reserved_ring_buffer_bytes);
    auto safe_format = SafeDriverRingBufferFormatFromElementDriverRingBufferFormatSets(
        element_id, ring_buffer_format_sets_by_element);
    auto callback_received = false;
    Device::RingBufferInfo rb_info;
    ASSERT_TRUE(device->CreateRingBuffer(
        element_id, safe_format, requested_ring_buffer_bytes,
        [&callback_received](
            const fit::result<fad::ControlCreateRingBufferError, Device::RingBufferInfo>& result) {
          EXPECT_TRUE(result.is_ok());
          callback_received = true;
        }));

    RunLoopUntilIdle();
    ASSERT_TRUE(callback_received);
    zx::time before_start = zx::clock::get_monotonic();
    zx::time start_time;
    callback_received = false;

    device->StartRingBuffer(element_id,
                            [&callback_received, &start_time](zx::result<zx::time> result) {
                              EXPECT_TRUE(result.is_ok());
                              start_time = *result;
                              EXPECT_LT(start_time.get(), zx::clock::get_monotonic().get());
                              callback_received = true;
                            });

    RunLoopUntilIdle();
    ASSERT_TRUE(callback_received);
    EXPECT_GT(start_time.get(), before_start.get());
    callback_received = false;

    device->StopRingBuffer(element_id, [&callback_received](zx_status_t status) {
      EXPECT_EQ(status, ZX_OK);
      callback_received = true;
    });

    RunLoopUntilIdle();
    EXPECT_TRUE(callback_received);
  }
}

// Verify SetActiveChannels -- for a driver that supports it.
TEST_F(CompositeTest, SetActiveChannelsSupported) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto ring_buffer_format_sets_by_element = ElementDriverRingBufferFormatSets(device);
  ASSERT_FALSE(ring_buffer_format_sets_by_element.empty());
  ASSERT_EQ(device->ring_buffer_ids().size(), ring_buffer_format_sets_by_element.size());
  constexpr uint32_t reserved_ring_buffer_bytes = 8192;
  constexpr uint32_t requested_ring_buffer_bytes = 4000;

  for (auto element_id : device->ring_buffer_ids()) {
    SCOPED_TRACE(std::string("Testing SetActiveChannels (supported): element_id ") +
                 std::to_string(element_id));
    fake_driver->EnableActiveChannelsSupport(element_id);
    fake_driver->ReserveRingBufferSize(element_id, reserved_ring_buffer_bytes);
    auto safe_format = SafeDriverRingBufferFormatFromElementDriverRingBufferFormatSets(
        element_id, ring_buffer_format_sets_by_element);
    auto callback_received = false;
    Device::RingBufferInfo rb_info;
    ASSERT_TRUE(device->CreateRingBuffer(
        element_id, safe_format, requested_ring_buffer_bytes,
        [&callback_received](
            const fit::result<fad::ControlCreateRingBufferError, Device::RingBufferInfo>& result) {
          EXPECT_TRUE(result.is_ok());
          callback_received = true;
        }));

    RunLoopUntilIdle();
    ASSERT_TRUE(callback_received);
    uint64_t channel_bitmask = (1U << safe_format.pcm_format()->number_of_channels()) - 2;
    zx::time set_time;
    callback_received = false;
    zx::time before_set = zx::clock::get_monotonic();

    auto succeeded = device->SetActiveChannels(
        element_id, channel_bitmask, [&callback_received, &set_time](zx::result<zx::time> result) {
          EXPECT_TRUE(result.is_ok());
          set_time = *result;
          EXPECT_LT(set_time.get(), zx::clock::get_monotonic().get());
          callback_received = true;
        });

    RunLoopUntilIdle();
    ASSERT_TRUE(succeeded);
    ASSERT_TRUE(callback_received);
    EXPECT_GT(set_time.get(), before_set.get());
    // Use the same channel_bitmask value -- it should succeed, but set_time should be unchanged.
    zx::time set_time2;
    callback_received = false;
    zx::time before_set2 = zx::clock::get_monotonic();

    succeeded = device->SetActiveChannels(
        element_id, channel_bitmask, [&callback_received, &set_time2](zx::result<zx::time> result) {
          EXPECT_TRUE(result.is_ok());
          set_time2 = *result;
          callback_received = true;
        });

    RunLoopUntilIdle();
    EXPECT_TRUE(succeeded);
    EXPECT_TRUE(callback_received);
    EXPECT_LT(set_time2.get(), before_set2.get());
    EXPECT_EQ(set_time.get(), set_time2.get())
        << "set_time should only change if the channel bitmask does (still " << std::hex
        << channel_bitmask << ")";
  }
}

// Verify SetActiveChannels -- for a driver that does not support it.
TEST_F(CompositeTest, SetActiveChannelsUnsupported) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto ring_buffer_format_sets_by_element = ElementDriverRingBufferFormatSets(device);
  ASSERT_FALSE(ring_buffer_format_sets_by_element.empty());
  ASSERT_EQ(device->ring_buffer_ids().size(), ring_buffer_format_sets_by_element.size());
  constexpr uint32_t reserved_ring_buffer_bytes = 8192;
  constexpr uint32_t requested_ring_buffer_bytes = 4000;

  for (auto element_id : device->ring_buffer_ids()) {
    SCOPED_TRACE(std::string("Testing SetActiveChannels (unsupported): element_id ") +
                 std::to_string(element_id));
    fake_driver->DisableActiveChannelsSupport(element_id);
    fake_driver->ReserveRingBufferSize(element_id, reserved_ring_buffer_bytes);
    auto safe_format = SafeDriverRingBufferFormatFromElementDriverRingBufferFormatSets(
        element_id, ring_buffer_format_sets_by_element);
    auto callback_received = false;
    Device::RingBufferInfo rb_info;
    ASSERT_TRUE(device->CreateRingBuffer(
        element_id, safe_format, requested_ring_buffer_bytes,
        [&callback_received](
            const fit::result<fad::ControlCreateRingBufferError, Device::RingBufferInfo>& result) {
          EXPECT_TRUE(result.is_ok());
          callback_received = true;
        }));

    RunLoopUntilIdle();
    ASSERT_TRUE(callback_received);
    uint64_t channel_bitmask = (1U << safe_format.pcm_format()->number_of_channels()) - 2;
    callback_received = false;

    // During CreateRingBuffer, SetActiveChannels is not called, so Device does not yet know whether
    // the driver supports this method. In this case it doesn't (DisableActiveChannelsSupport above)
    // so we expect the method to return true (it did call the driver), with NOT_SUPPORTED error.
    auto succeeded = device->SetActiveChannels(
        element_id, channel_bitmask, [&callback_received](zx::result<zx::time> result) {
          ASSERT_TRUE(result.is_error()) << "Unexpected successful SetActiveChannels";
          EXPECT_EQ(result.status_value(), ZX_ERR_NOT_SUPPORTED);
          callback_received = true;
        });

    RunLoopUntilIdle();
    EXPECT_TRUE(succeeded);
    EXPECT_TRUE(callback_received);
    callback_received = false;

    // Device now knows that the driver does not support this method, so we can expect it to avoid
    // subsequent SetActiveChannels driver calls - returning false means it did NOT call the driver.
    succeeded = device->SetActiveChannels(
        element_id, channel_bitmask, [&callback_received](zx::result<zx::time> result) {
          FAIL() << "Unexpected response to unsupported SetActiveChannels";
          callback_received = true;
        });

    RunLoopUntilIdle();
    EXPECT_FALSE(succeeded);
    EXPECT_FALSE(callback_received);
  }
}

// Verify that we get DelayInfo change notifications
TEST_F(CompositeTest, WatchDelayInfoInitial) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));
  auto ring_buffer_format_sets_by_element = ElementDriverRingBufferFormatSets(device);
  ASSERT_FALSE(ring_buffer_format_sets_by_element.empty());
  ASSERT_EQ(device->ring_buffer_ids().size(), ring_buffer_format_sets_by_element.size());
  constexpr uint32_t reserved_ring_buffer_bytes = 8192;
  constexpr uint32_t requested_ring_buffer_bytes = 4000;

  for (auto element_id : device->ring_buffer_ids()) {
    SCOPED_TRACE(std::string("Testing RingBuffer initial DelayInfo: element_id ") +
                 std::to_string(element_id));
    fake_driver->ReserveRingBufferSize(element_id, reserved_ring_buffer_bytes);
    auto safe_format = SafeDriverRingBufferFormatFromElementDriverRingBufferFormatSets(
        element_id, ring_buffer_format_sets_by_element);
    auto created_ring_buffer = false;
    Device::RingBufferInfo rb_info;
    ASSERT_TRUE(device->CreateRingBuffer(
        element_id, safe_format, requested_ring_buffer_bytes,
        [&created_ring_buffer](
            const fit::result<fad::ControlCreateRingBufferError, Device::RingBufferInfo>& result) {
          created_ring_buffer = true;
          EXPECT_TRUE(result.is_ok());
        }));

    RunLoopUntilIdle();
    ASSERT_TRUE(created_ring_buffer);
    // Verify that the Device received the expected values from the driver.
    ASSERT_TRUE(DeviceDelayInfo(device, element_id));
    ASSERT_TRUE(DeviceDelayInfo(device, element_id)->internal_delay());
    EXPECT_EQ(*DeviceDelayInfo(device, element_id)->internal_delay(),
              FakeCompositeRingBuffer::kDefaultInternalDelay->get());
    EXPECT_TRUE(!DeviceDelayInfo(device, element_id)->external_delay().has_value() &&
                !FakeCompositeRingBuffer::kDefaultExternalDelay.has_value());

    // Verify that the ControlNotify was sent the expected values.
    ASSERT_TRUE(notify()->delay_info(element_id))
        << "ControlNotify was not notified of initial delay info";
    ASSERT_TRUE(notify()->delay_info(element_id)->internal_delay());
    EXPECT_EQ(*notify()->delay_info(element_id)->internal_delay(),
              FakeCompositeRingBuffer::kDefaultInternalDelay->get());
    EXPECT_TRUE(!notify()->delay_info(element_id)->external_delay().has_value() &&
                !FakeCompositeRingBuffer::kDefaultExternalDelay.has_value());
  }
}

// Dynamic delay updates
TEST_F(CompositeTest, WatchDelayInfoUpdate) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));
  auto ring_buffer_format_sets_by_element = ElementDriverRingBufferFormatSets(device);
  ASSERT_FALSE(ring_buffer_format_sets_by_element.empty());
  ASSERT_EQ(device->ring_buffer_ids().size(), ring_buffer_format_sets_by_element.size());
  constexpr uint32_t reserved_ring_buffer_bytes = 8192;
  constexpr uint32_t requested_ring_buffer_bytes = 4000;

  for (auto element_id : device->ring_buffer_ids()) {
    SCOPED_TRACE(std::string("Testing RingBuffer initial DelayInfo: element_id ") +
                 std::to_string(element_id));
    fake_driver->ReserveRingBufferSize(element_id, reserved_ring_buffer_bytes);
    auto safe_format = SafeDriverRingBufferFormatFromElementDriverRingBufferFormatSets(
        element_id, ring_buffer_format_sets_by_element);
    auto created_ring_buffer = false;
    Device::RingBufferInfo rb_info;
    ASSERT_TRUE(device->CreateRingBuffer(
        element_id, safe_format, requested_ring_buffer_bytes,
        [&created_ring_buffer](
            const fit::result<fad::ControlCreateRingBufferError, Device::RingBufferInfo>& result) {
          created_ring_buffer = true;
          EXPECT_TRUE(result.is_ok());
        }));

    RunLoopUntilIdle();
    ASSERT_TRUE(created_ring_buffer);
    ASSERT_TRUE(DeviceDelayInfo(device, element_id));
    ASSERT_TRUE(notify()->delay_info(element_id))
        << "ControlNotify was not notified of initial delay info";
  }

  notify()->clear_delay_infos();
  for (auto element_id : device->ring_buffer_ids()) {
    EXPECT_FALSE(notify()->delay_info(element_id))
        << "ControlNotify was not cleared for element_id " << element_id;
    // For each element, inject int_delay [`element_id` nsec] and ext_delay [`element_id` usec].
    fake_driver->RingBufferInjectDelayUpdate(element_id, zx::nsec(static_cast<int64_t>(element_id)),
                                             zx::usec(static_cast<int64_t>(element_id)));
  }

  RunLoopUntilIdle();

  for (auto element_id : device->ring_buffer_ids()) {
    // Ensure the Device received the updated values from the driver.
    ASSERT_TRUE(DeviceDelayInfo(device, element_id));
    ASSERT_TRUE(DeviceDelayInfo(device, element_id)->internal_delay());
    ASSERT_TRUE(DeviceDelayInfo(device, element_id)->external_delay());
    EXPECT_EQ(*DeviceDelayInfo(device, element_id)->internal_delay(), ZX_NSEC(element_id));
    EXPECT_EQ(*DeviceDelayInfo(device, element_id)->external_delay(), ZX_USEC(element_id));

    // Ensure that ControlNotify received these updates as well.
    ASSERT_TRUE(notify()->delay_info(element_id))
        << "ControlNotify was not notified with updated delay info";
    ASSERT_TRUE(notify()->delay_info(element_id)->internal_delay());
    ASSERT_TRUE(notify()->delay_info(element_id)->external_delay());
    EXPECT_EQ(*notify()->delay_info(element_id)->internal_delay(), ZX_NSEC(element_id));
    EXPECT_EQ(*notify()->delay_info(element_id)->external_delay(), ZX_USEC(element_id));
  }
}

}  // namespace

void CompositeTest::TestCreatePacketStream(const std::shared_ptr<Device>& device,
                                           ElementId element_id, const fha::Format2& safe_format) {
  std::ostringstream stream;
  stream << "Validating CreatePacketStream on element_id " << element_id << " with format "
         << (safe_format.pcm_format() ? "PCM" : "Encoding");
  SCOPED_TRACE(stream.str());

  auto callback_received = false;

  ASSERT_TRUE(device->CreatePacketStream(
      element_id, safe_format,
      [&callback_received](const fit::result<fad::ControlCreatePacketStreamError,
                                             Device::PacketStreamInfo>& result) {
        ASSERT_TRUE(result.is_ok());
        callback_received = true;
      }));

  RunLoopUntilIdle();
  ASSERT_TRUE(callback_received);
  EXPECT_TRUE(PacketStreamIsStopped(device, element_id));
}

// PacketStream Buffer Configuration Expectations based on Driver Capabilities:
//
// 1. Driver supports CLIENT_OWNED, DRIVER_OWNED, INLINE
//    - SetBuffers(Allocate) -> Success. Start() -> Success. PutPacket(VMO/INLINE) -> Success.
//    - SetBuffers(Register) -> Success. Start() -> Success. PutPacket(VMO/INLINE) -> Success.
//    - No SetBuffers -> Start() succeeds. PutPacket(VMO) -> ZX_ERR_BAD_STATE. PutPacket(INLINE) ->
//    Success.
//
// 2. Driver supports CLIENT_OWNED, DRIVER_OWNED
//    - SetBuffers(Allocate) -> Success. Start() -> Success. PutPacket(INLINE) ->
//    ZX_ERR_INVALID_ARGS.
//    - SetBuffers(Register) -> Success. Start() -> Success. PutPacket(INLINE) ->
//    ZX_ERR_INVALID_ARGS.
//    - No SetBuffers -> Start() fails with ZX_ERR_BAD_STATE.
//
// 3. Driver supports CLIENT_OWNED, INLINE
//    - SetBuffers(Allocate) -> Fails with ZX_ERR_INVALID_ARGS.
//    - SetBuffers(Register) -> Success. Start() -> Success. PutPacket(VMO/INLINE) -> Success.
//    - No SetBuffers -> Start() succeeds. PutPacket(VMO) -> ZX_ERR_BAD_STATE. PutPacket(INLINE) ->
//    Success.
//
// 4. Driver supports DRIVER_OWNED, INLINE
//    - SetBuffers(Allocate) -> Success. Start() -> Success. PutPacket(VMO/INLINE) -> Success.
//    - SetBuffers(Register) -> Fails with ZX_ERR_INVALID_ARGS.
//    - No SetBuffers -> Start() succeeds. PutPacket(VMO) -> ZX_ERR_BAD_STATE. PutPacket(INLINE) ->
//    Success.
//
// 5. Driver supports CLIENT_OWNED only
//    - SetBuffers(Allocate) -> Fails with ZX_ERR_INVALID_ARGS.
//    - SetBuffers(Register) -> Success. Start() -> Success. PutPacket(INLINE) ->
//    ZX_ERR_INVALID_ARGS.
//    - No SetBuffers -> Start() fails with ZX_ERR_BAD_STATE.
//
// 6. Driver supports DRIVER_OWNED only
//    - SetBuffers(Allocate) -> Success. Start() -> Success. PutPacket(INLINE) ->
//    ZX_ERR_INVALID_ARGS.
//    - SetBuffers(Register) -> Fails with ZX_ERR_INVALID_ARGS.
//    - No SetBuffers -> Start() fails with ZX_ERR_BAD_STATE.
//
// 7. Driver supports INLINE only
//    - SetBuffers(Allocate/Register) -> Fails with ZX_ERR_INVALID_ARGS.
//    - No SetBuffers -> Start() succeeds. PutPacket(VMO) -> ZX_ERR_INVALID_ARGS. PutPacket(INLINE)
//    -> Success.
//
// 8. Driver supports [NO bits set]
//    - ADR should fail to initialize and add the device.
//
void CompositeTest::TestAllocatePacketStreamBuffers(const std::shared_ptr<Device>& device,
                                                    ElementId element_id) {
  SCOPED_TRACE("TestAllocatePacketStreamBuffers");
  auto callback_received = false;
  device->SetPacketStreamBuffers(
      element_id,
      fad::PacketStreamSetupVmoInfo::WithAllocateInfo(fha::AllocateVmosConfig{{
          .min_vmo_size = 8192,
          .vmo_count = 1,
      }}),
      [&callback_received](
          fit::result<fad::PacketStreamSetBufferError, fad::PacketStreamBuffers> result) {
        ASSERT_TRUE(result.is_ok());
        ASSERT_TRUE(result->vmo_infos().has_value());
        EXPECT_FALSE(result->vmo_infos()->empty());
        callback_received = true;
      });
  RunLoopUntilIdle();
  ASSERT_TRUE(callback_received);
}

void CompositeTest::TestRegisterPacketStreamBuffers(const std::shared_ptr<Device>& device,
                                                    ElementId element_id) {
  SCOPED_TRACE("TestRegisterPacketStreamBuffers");
  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(8192, 0, &vmo), ZX_OK);

  std::vector<fha::VmoInfo> vmo_infos;
  vmo_infos.push_back(fha::VmoInfo{{
      .id = 0,
      .vmo = std::move(vmo),
  }});

  auto callback_received = false;
  device->SetPacketStreamBuffers(
      element_id,
      fad::PacketStreamSetupVmoInfo::WithRegisterInfo(fha::RegisterVmosConfig{{
          .vmo_infos = std::move(vmo_infos),
      }}),
      [&callback_received](
          fit::result<fad::PacketStreamSetBufferError, fad::PacketStreamBuffers> result) {
        ASSERT_TRUE(result.is_ok());
        ASSERT_TRUE(result->vmo_infos().has_value());
        EXPECT_EQ(result->vmo_infos()->size(), 1u);
        callback_received = true;
      });
  RunLoopUntilIdle();
  ASSERT_TRUE(callback_received);
}

namespace {

// Allocating buffers on a PacketStream that supports allocated buffers should succeed.
TEST_F(CompositeTest, PacketStreamAllocateBuffers) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto packet_stream_format_sets_by_element = ElementDriverPacketStreamFormatSets(device);
  for (auto element_id : device->packet_stream_ids()) {
    for (const auto& safe_format :
         SafeDriverPacketStreamFormats(element_id, packet_stream_format_sets_by_element)) {
      TestCreatePacketStream(device, element_id, safe_format);
      TestAllocatePacketStreamBuffers(device, element_id);
    }
  }
}

// Registering buffers on a PacketStream that supports registered buffers should succeed.
TEST_F(CompositeTest, PacketStreamRegisterBuffers) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto packet_stream_format_sets_by_element = ElementDriverPacketStreamFormatSets(device);
  for (auto element_id : device->packet_stream_ids()) {
    for (const auto& safe_format :
         SafeDriverPacketStreamFormats(element_id, packet_stream_format_sets_by_element)) {
      TestCreatePacketStream(device, element_id, safe_format);
      TestRegisterPacketStreamBuffers(device, element_id);
    }
  }
}

// Creating PacketStreams on every element should succeed.
TEST_F(CompositeTest, CreatePacketStreams) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  // Create PacketStreams on every PacketStream element.
  auto packet_stream_format_sets_by_element = ElementDriverPacketStreamFormatSets(device);
  for (auto element_id : device->packet_stream_ids()) {
    for (const auto& safe_format :
         SafeDriverPacketStreamFormats(element_id, packet_stream_format_sets_by_element)) {
      TestCreatePacketStream(device, element_id, safe_format);
      TestAllocatePacketStreamBuffers(device, element_id);
    }
  }
  EXPECT_EQ(FakeCompositePacketStream::count(), device->packet_stream_ids().size());
}

// Verify that we get DeviceDroppedPacketStream notifications.
TEST_F(CompositeTest, DeviceDroppedPacketStream) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto packet_stream_ids = device->packet_stream_ids();
  ASSERT_FALSE(packet_stream_ids.empty());
  auto element_id = *packet_stream_ids.begin();

  auto packet_stream_format_sets_by_element = ElementDriverPacketStreamFormatSets(device);
  auto safe_format =
      SafeDriverPacketStreamFormats(element_id, packet_stream_format_sets_by_element).front();
  TestCreatePacketStream(device, element_id, safe_format);

  EXPECT_EQ(FakeCompositePacketStream::count(), 1u);

  fake_driver->DropPacketStream(element_id);
  RunLoopUntilIdle();

  EXPECT_EQ(FakeCompositePacketStream::count(), 0u);
  EXPECT_FALSE(HasPacketStream(device, element_id));
  EXPECT_EQ(device_presence_watcher()->ready_devices().size(), 1u);
  EXPECT_EQ(device_presence_watcher()->error_devices().size(), 0u);
}

// Valid Start/Stop on a PacketStream should succeed.
TEST_F(CompositeTest, PacketStreamStartAndStop) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  auto packet_stream_format_sets_by_element = ElementDriverPacketStreamFormatSets(device);
  ASSERT_FALSE(packet_stream_format_sets_by_element.empty());
  ASSERT_EQ(device->packet_stream_ids().size(), packet_stream_format_sets_by_element.size());

  for (auto element_id : device->packet_stream_ids()) {
    for (const auto& safe_format :
         SafeDriverPacketStreamFormats(element_id, packet_stream_format_sets_by_element)) {
      SCOPED_TRACE(std::string("Testing PacketStream Start/Stop: element_id ") +
                   std::to_string(element_id));
      TestCreatePacketStream(device, element_id, safe_format);
      TestAllocatePacketStreamBuffers(device, element_id);

      auto callback_received = false;
      device->StartPacketStream(element_id, [&callback_received](zx::result<> result) {
        EXPECT_TRUE(result.is_ok());
        callback_received = true;
      });

      RunLoopUntilIdle();
      ASSERT_TRUE(callback_received);
      callback_received = false;

      device->StopPacketStream(element_id, [&callback_received](zx_status_t status) {
        EXPECT_EQ(status, ZX_OK);
        callback_received = true;
      });

      RunLoopUntilIdle();
      EXPECT_TRUE(callback_received);
    }
  }
}

//// This is needed to support hardware where the audio device is NOT in kClockDomainMonotonic.
//// Until then, it is not a high priority.
// TEST_F(CompositeTest, PositionNotifications) { FAIL() << "NOT YET IMPLEMENTED"; }

// Signalprocessing test cases
//
// Ensure that we captured the full contents of FakeComposite signalprocessing functionality,
// specifically the signalprocessing elements.
TEST_F(CompositeTest, GetElements) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());

  ASSERT_TRUE(device->info().has_value());
  ASSERT_TRUE(device->info()->signal_processing_elements().has_value());
  auto& elements = device->info()->signal_processing_elements();
  ASSERT_TRUE(elements.has_value());
  ASSERT_EQ(elements->size(), FakeComposite::kElements.size());

  // Get all the `has_value` checks done in automated fashion upfront.
  for (const auto& element : *elements) {
    ASSERT_TRUE(element.id().has_value());
    ASSERT_TRUE(elements->at(0).type().has_value());
    ASSERT_TRUE(element.description().has_value());
    ASSERT_TRUE(element.can_stop().has_value());
    ASSERT_TRUE(element.can_bypass().has_value());
    if (element.type() == fhasp::ElementType::kDaiInterconnect) {
      ASSERT_TRUE(element.type_specific().has_value());
      ASSERT_TRUE(element.type_specific()->dai_interconnect().has_value());
      ASSERT_TRUE(
          element.type_specific()->dai_interconnect()->plug_detect_capabilities().has_value());
    }
  }

  EXPECT_EQ(elements->at(0).id(), FakeComposite::kSourceDaiElementId);
  EXPECT_EQ(elements->at(0).type(), fhasp::ElementType::kDaiInterconnect);
  EXPECT_FALSE(*elements->at(0).can_bypass());
  EXPECT_TRUE(*elements->at(0).can_stop());
  EXPECT_EQ(*elements->at(0).description(), *FakeComposite::kSourceDaiElement.description());
  EXPECT_EQ(elements->at(0).type_specific()->dai_interconnect()->plug_detect_capabilities(),
            fhasp::PlugDetectCapabilities::kCanAsyncNotify);

  EXPECT_EQ(elements->at(1).id(), FakeComposite::kSourceRbElementId);
  EXPECT_EQ(elements->at(1).type(), fhasp::ElementType::kRingBuffer);
  EXPECT_FALSE(*elements->at(1).can_bypass());
  EXPECT_FALSE(*elements->at(1).can_stop());
  EXPECT_EQ(*elements->at(1).description(), *FakeComposite::kSourceRbElement.description());

  EXPECT_EQ(elements->at(2).id(), FakeComposite::kSourcePsElementId);
  EXPECT_EQ(elements->at(2).type(), fhasp::ElementType::kPacketStream);
  EXPECT_FALSE(*elements->at(2).can_bypass());
  EXPECT_FALSE(*elements->at(2).can_stop());
  EXPECT_EQ(*elements->at(2).description(), *FakeComposite::kSourcePsElement.description());

  EXPECT_EQ(elements->at(3).id(), FakeComposite::kSourceDualSupportPsElementId);
  EXPECT_EQ(elements->at(3).type(), fhasp::ElementType::kPacketStream);
  EXPECT_FALSE(*elements->at(3).can_bypass());
  EXPECT_FALSE(*elements->at(3).can_stop());
  EXPECT_EQ(*elements->at(3).description(),
            *FakeComposite::kSourceDualSupportPsElement.description());

  EXPECT_EQ(elements->at(4).id(), FakeComposite::kDestDaiElementId);
  EXPECT_EQ(elements->at(4).type(), fhasp::ElementType::kDaiInterconnect);
  EXPECT_FALSE(*elements->at(4).can_bypass());
  EXPECT_TRUE(*elements->at(4).can_stop());
  EXPECT_EQ(*elements->at(4).description(), *FakeComposite::kDestDaiElement.description());
  EXPECT_EQ(elements->at(4).type_specific()->dai_interconnect()->plug_detect_capabilities(),
            fhasp::PlugDetectCapabilities::kCanAsyncNotify);

  EXPECT_EQ(elements->at(5).id(), FakeComposite::kDestRbElementId);
  EXPECT_EQ(elements->at(5).type(), fhasp::ElementType::kRingBuffer);
  EXPECT_FALSE(*elements->at(5).can_bypass());
  EXPECT_FALSE(*elements->at(5).can_stop());
  EXPECT_EQ(*elements->at(5).description(), *FakeComposite::kDestRbElement.description());

  EXPECT_EQ(elements->at(6).id(), FakeComposite::kDestPsElementId);
  EXPECT_EQ(elements->at(6).type(), fhasp::ElementType::kPacketStream);
  EXPECT_FALSE(*elements->at(6).can_bypass());
  EXPECT_FALSE(*elements->at(6).can_stop());
  EXPECT_EQ(*elements->at(6).description(), *FakeComposite::kDestPsElement.description());

  EXPECT_EQ(elements->at(7).id(), FakeComposite::kVendorSpecificElementId);
  EXPECT_EQ(elements->at(7).type(), fhasp::ElementType::kVendorSpecific);
  EXPECT_TRUE(*elements->at(7).can_bypass());
  EXPECT_FALSE(*elements->at(7).can_stop());
  EXPECT_EQ(*elements->at(7).description(), *FakeComposite::kVendorSpecificElement.description());
  EXPECT_TRUE(elements->at(7).type_specific().has_value());

  EXPECT_EQ(elements->at(8).id(), FakeComposite::kDynamicsElementId);
  EXPECT_EQ(elements->at(8).type(), fhasp::ElementType::kDynamics);
  EXPECT_TRUE(*elements->at(8).can_bypass());
  EXPECT_TRUE(*elements->at(8).can_stop());
  EXPECT_EQ(*elements->at(8).description(), *FakeComposite::kDynamicsElement.description());
  EXPECT_TRUE(elements->at(8).type_specific().has_value());

  EXPECT_EQ(elements->at(9).id(), FakeComposite::kEqualizerElementId);
  EXPECT_EQ(elements->at(9).type(), fhasp::ElementType::kEqualizer);
  EXPECT_TRUE(*elements->at(9).can_bypass());
  EXPECT_TRUE(*elements->at(9).can_stop());
  EXPECT_EQ(*elements->at(9).description(), *FakeComposite::kEqualizerElement.description());
  EXPECT_TRUE(elements->at(9).type_specific().has_value());

  EXPECT_EQ(elements->at(10).id(), FakeComposite::kGainElementId);
  EXPECT_EQ(elements->at(10).type(), fhasp::ElementType::kGain);
  EXPECT_TRUE(*elements->at(10).can_bypass());
  EXPECT_FALSE(*elements->at(10).can_stop());
  EXPECT_EQ(*elements->at(10).description(), *FakeComposite::kGainElement.description());
  EXPECT_TRUE(elements->at(10).type_specific().has_value());

  EXPECT_EQ(elements->at(11).id(), FakeComposite::kMuteElementId);
  EXPECT_EQ(elements->at(11).type(), fhasp::ElementType::kMute);
  EXPECT_TRUE(*elements->at(11).can_bypass());
  EXPECT_FALSE(*elements->at(11).can_stop());
  EXPECT_EQ(*elements->at(11).description(), *FakeComposite::kMuteElement.description());
  EXPECT_FALSE(elements->at(11).type_specific().has_value());
}

// Ensure that we captured the full contents of FakeComposite signalprocessing functionality,
// specifically the signalprocessing topologies.
TEST_F(CompositeTest, GetTopologies) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());

  ASSERT_TRUE(device->info().has_value());
  ASSERT_TRUE(device->info()->signal_processing_elements().has_value());
  auto& topologies = device->info()->signal_processing_topologies();
  ASSERT_EQ(topologies->size(), FakeComposite::kTopologies.size());

  ASSERT_TRUE(topologies->at(0).id().has_value());
  EXPECT_EQ(*topologies->at(0).id(), FakeComposite::kInputOnlyTopologyId);
  ASSERT_TRUE(topologies->at(0).processing_elements_edge_pairs().has_value());
  ASSERT_GE(topologies->at(0).processing_elements_edge_pairs()->size(), 1u);
  EXPECT_EQ(topologies->at(0).processing_elements_edge_pairs()->at(0).processing_element_id_from(),
            FakeComposite::kSourceDaiElementId);
  EXPECT_EQ(topologies->at(0).processing_elements_edge_pairs()->at(0).processing_element_id_to(),
            FakeComposite::kDestRbElementId);

  ASSERT_TRUE(topologies->at(1).id().has_value());
  EXPECT_EQ(*topologies->at(1).id(), FakeComposite::kPacketStreamCaptureTopologyId);
  ASSERT_TRUE(topologies->at(1).processing_elements_edge_pairs().has_value());
  ASSERT_GE(topologies->at(1).processing_elements_edge_pairs()->size(), 1u);
  EXPECT_EQ(topologies->at(1).processing_elements_edge_pairs()->at(0).processing_element_id_from(),
            FakeComposite::kSourceDaiElementId);
  EXPECT_EQ(topologies->at(1).processing_elements_edge_pairs()->at(0).processing_element_id_to(),
            FakeComposite::kDestPsElementId);

  ASSERT_TRUE(topologies->at(2).id().has_value());
  EXPECT_EQ(*topologies->at(2).id(), FakeComposite::kFullDuplexTopologyId);
  ASSERT_TRUE(topologies->at(2).processing_elements_edge_pairs().has_value());
  ASSERT_GE(topologies->at(2).processing_elements_edge_pairs()->size(), 2u);
  EXPECT_EQ(topologies->at(2).processing_elements_edge_pairs()->at(0).processing_element_id_from(),
            FakeComposite::kSourceDaiElementId);
  EXPECT_EQ(topologies->at(2).processing_elements_edge_pairs()->at(0).processing_element_id_to(),
            FakeComposite::kDestRbElementId);
  EXPECT_EQ(topologies->at(2).processing_elements_edge_pairs()->at(1).processing_element_id_from(),
            FakeComposite::kSourceRbElementId);
  EXPECT_EQ(topologies->at(2).processing_elements_edge_pairs()->at(1).processing_element_id_to(),
            FakeComposite::kDestDaiElementId);

  ASSERT_TRUE(topologies->at(3).id().has_value());
  EXPECT_EQ(*topologies->at(3).id(), FakeComposite::kOutputOnlyTopologyId);
  ASSERT_TRUE(topologies->at(3).processing_elements_edge_pairs().has_value());
  ASSERT_GE(topologies->at(3).processing_elements_edge_pairs()->size(), 1u);
  EXPECT_EQ(topologies->at(3).processing_elements_edge_pairs()->at(0).processing_element_id_from(),
            FakeComposite::kSourceRbElementId);
  EXPECT_EQ(topologies->at(3).processing_elements_edge_pairs()->at(0).processing_element_id_to(),
            FakeComposite::kDestDaiElementId);

  ASSERT_TRUE(topologies->at(4).id().has_value());
  EXPECT_EQ(*topologies->at(4).id(), FakeComposite::kPacketStreamOutputTopologyId);
  ASSERT_TRUE(topologies->at(4).processing_elements_edge_pairs().has_value());
  ASSERT_GE(topologies->at(4).processing_elements_edge_pairs()->size(), 1u);
  EXPECT_EQ(topologies->at(4).processing_elements_edge_pairs()->at(0).processing_element_id_from(),
            FakeComposite::kSourcePsElementId);
  EXPECT_EQ(topologies->at(4).processing_elements_edge_pairs()->at(0).processing_element_id_to(),
            FakeComposite::kDestDaiElementId);

  ASSERT_TRUE(topologies->at(5).id().has_value());
  EXPECT_EQ(*topologies->at(5).id(), FakeComposite::kOutputWithProcessingTopologyId);
  ASSERT_TRUE(topologies->at(5).processing_elements_edge_pairs().has_value());
  auto& pairs = topologies->at(5).processing_elements_edge_pairs().value();
  ASSERT_GE(pairs.size(), 6u);
  EXPECT_EQ(pairs[0].processing_element_id_from(), FakeComposite::kSourceRbElementId);
  EXPECT_EQ(pairs[0].processing_element_id_to(), FakeComposite::kVendorSpecificElementId);
  EXPECT_EQ(pairs[1].processing_element_id_from(), FakeComposite::kVendorSpecificElementId);
  EXPECT_EQ(pairs[1].processing_element_id_to(), FakeComposite::kDynamicsElementId);
  EXPECT_EQ(pairs[2].processing_element_id_from(), FakeComposite::kDynamicsElementId);
  EXPECT_EQ(pairs[2].processing_element_id_to(), FakeComposite::kEqualizerElementId);
  EXPECT_EQ(pairs[3].processing_element_id_from(), FakeComposite::kEqualizerElementId);
  EXPECT_EQ(pairs[3].processing_element_id_to(), FakeComposite::kGainElementId);
  EXPECT_EQ(pairs[4].processing_element_id_from(), FakeComposite::kGainElementId);
  EXPECT_EQ(pairs[4].processing_element_id_to(), FakeComposite::kMuteElementId);
  EXPECT_EQ(pairs[5].processing_element_id_from(), FakeComposite::kMuteElementId);
  EXPECT_EQ(pairs[5].processing_element_id_to(), FakeComposite::kDestDaiElementId);
}

// Ensure that we captured the full contents of FakeComposite signalprocessing functionality,
// specifically the initial signalprocessing state.
TEST_F(CompositeTest, WatchElementStateInitial) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(AddObserver(device));

  const auto& states = notify()->element_states();
  ASSERT_EQ(states.size(), FakeComposite::kElements.size());

  auto state = states.find(FakeComposite::kSourceDaiElementId)->second;
  ASSERT_TRUE(state.type_specific().has_value());
  ASSERT_TRUE(state.vendor_specific_data().has_value());
  ASSERT_TRUE(state.started().has_value());
  ASSERT_TRUE(state.bypassed().has_value());
  ASSERT_TRUE(state.processing_delay().has_value());

  const auto& endpt_state1 = state.type_specific()->dai_interconnect();
  ASSERT_TRUE(endpt_state1.has_value());
  ASSERT_TRUE(endpt_state1->plug_state().has_value());
  ASSERT_TRUE(endpt_state1->plug_state()->plugged().has_value());
  EXPECT_TRUE(*endpt_state1->plug_state()->plugged());
  EXPECT_EQ(*endpt_state1->plug_state()->plug_state_time(), 0);
  ASSERT_EQ(state.vendor_specific_data()->size(), 8u);
  EXPECT_EQ(state.vendor_specific_data()->at(0), 1u);
  EXPECT_EQ(state.vendor_specific_data()->at(7), 8u);
  EXPECT_FALSE(*state.started());
  EXPECT_FALSE(*state.bypassed());
  EXPECT_EQ(*state.processing_delay(), FakeComposite::kSourceDaiElementProcessingDelay.get());

  state = states.find(FakeComposite::kDestDaiElementId)->second;
  ASSERT_TRUE(state.type_specific().has_value());
  ASSERT_TRUE(state.vendor_specific_data().has_value());
  ASSERT_TRUE(state.started().has_value());
  ASSERT_TRUE(state.bypassed().has_value());
  ASSERT_TRUE(state.processing_delay().has_value());

  const auto& endpt_state2 = state.type_specific()->dai_interconnect();
  ASSERT_TRUE(endpt_state2.has_value());
  ASSERT_TRUE(endpt_state2->plug_state().has_value());
  ASSERT_TRUE(endpt_state2->plug_state()->plugged().has_value());
  EXPECT_TRUE(*endpt_state2->plug_state()->plugged());
  EXPECT_EQ(*endpt_state2->plug_state()->plug_state_time(), 0);
  ASSERT_EQ(state.vendor_specific_data()->size(), 9u);
  EXPECT_EQ(state.vendor_specific_data()->at(0), 8u);
  EXPECT_EQ(state.vendor_specific_data()->at(8), 0u);
  EXPECT_FALSE(*state.started());
  EXPECT_FALSE(*state.bypassed());
  EXPECT_EQ(*state.processing_delay(), FakeComposite::kDestDaiElementProcessingDelay.get());

  state = states.find(FakeComposite::kSourceRbElementId)->second;
  EXPECT_FALSE(state.type_specific().has_value());
  EXPECT_FALSE(state.vendor_specific_data().has_value());
  ASSERT_TRUE(state.started().has_value());
  ASSERT_TRUE(state.bypassed().has_value());
  ASSERT_TRUE(state.processing_delay().has_value());

  EXPECT_TRUE(*state.started());
  EXPECT_FALSE(*state.bypassed());
  EXPECT_EQ(*state.processing_delay(), FakeComposite::kSourceRbElementProcessingDelay.get());

  state = states.find(FakeComposite::kDestRbElementId)->second;
  EXPECT_FALSE(state.type_specific().has_value());
  EXPECT_FALSE(state.vendor_specific_data().has_value());
  ASSERT_TRUE(state.started().has_value());
  ASSERT_TRUE(state.bypassed().has_value());
  ASSERT_FALSE(state.processing_delay().has_value());

  EXPECT_TRUE(*state.started());
  EXPECT_FALSE(*state.bypassed());

  state = states.find(FakeComposite::kMuteElementId)->second;
  EXPECT_FALSE(state.type_specific().has_value());
  EXPECT_FALSE(state.vendor_specific_data().has_value());
  ASSERT_TRUE(state.started().has_value());
  ASSERT_TRUE(state.bypassed().has_value());
  EXPECT_FALSE(state.processing_delay().has_value());

  EXPECT_TRUE(*state.started());
  EXPECT_TRUE(*state.bypassed());
}

TEST_F(CompositeTest, WatchElementStateUpdate) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(AddObserver(device));

  auto& elements = *device->info()->signal_processing_elements();
  const auto& states = notify()->element_states();
  ASSERT_EQ(states.size(), FakeComposite::kElements.size());

  // Determine which states we can inject change into.
  std::unordered_map<ElementId, fhasp::ElementState> element_states_to_inject;
  auto plug_change_time_to_inject = zx::clock::get_monotonic();
  for (const auto& element : elements) {
    auto element_id = *element.id();
    auto match_state = states.find(element_id);
    ASSERT_NE(match_state, states.end());
    auto state = match_state->second;

    // Handle the Mute node
    if (element.type() == fhasp::ElementType::kMute && element.can_bypass().value_or(false)) {
      // By configuration, our Mute starts bypassed (we activate it as our ElementState change).
      ASSERT_TRUE(state.started().has_value());
      EXPECT_TRUE(*state.started());
      ASSERT_TRUE(state.bypassed().has_value());
      EXPECT_TRUE(*state.bypassed());
      element_states_to_inject.insert_or_assign(element_id, fhasp::ElementState{{
                                                                .started = true,
                                                                .bypassed = false,
                                                            }});
      continue;
    }

    // Then weed out any non-pluggable elements.
    if (element.type() != fhasp::ElementType::kDaiInterconnect ||
        !element.type_specific().has_value() ||
        !element.type_specific()->dai_interconnect().has_value() ||
        element.type_specific()->dai_interconnect()->plug_detect_capabilities() !=
            fhasp::PlugDetectCapabilities::kCanAsyncNotify) {
      continue;
    }
    if (!state.type_specific().has_value() ||
        !state.type_specific()->dai_interconnect().has_value() ||
        !state.type_specific()->dai_interconnect()->plug_state().has_value() ||
        !state.type_specific()->dai_interconnect()->plug_state()->plugged().has_value() ||
        !state.type_specific()->dai_interconnect()->plug_state()->plug_state_time().has_value()) {
      continue;
    }
    auto was_plugged = state.type_specific()->dai_interconnect()->plug_state()->plugged();
    auto
        new_state =
            fhasp::ElementState{
                {
                    .type_specific = fhasp::TypeSpecificElementState::WithDaiInterconnect(
                        fhasp::DaiInterconnectElementState{{
                            .plug_state = fhasp::PlugState{{
                                .plugged = !was_plugged,
                                .plug_state_time = plug_change_time_to_inject.get(),
                            }},
                            .external_delay = ZX_MSEC(element_id),
                        }}),
                    .vendor_specific_data =
                        {
                            {
                                '0',
                                '1',
                                '2',
                                '3',
                                '4',
                                '5',
                                '6',
                                '7',
                                '8',
                                '9',
                                'A',
                                'B',
                                'C',
                                'D',
                                'E',
                                'F',
                                'Z',
                            },
                        },  // 'Z' is located at byte [16].
                    .started = true,
                    .bypassed = false,
                    .processing_delay = ZX_USEC(element_id),
                }};
    ASSERT_EQ(new_state.vendor_specific_data()->size(), 17u) << "Test configuration error";
    element_states_to_inject.insert_or_assign(element_id, new_state);
  }

  if (element_states_to_inject.empty()) {
    GTEST_SKIP()
        << "No element states can be changed, so dynamic element_state change cannot be tested";
  }

  notify()->clear_element_states();

  // Inject the changes.
  for (const auto& [element_id, element_state] : element_states_to_inject) {
    fake_driver->InjectElementStateChange(element_id, element_state);
  }

  RunLoopUntilIdle();
  EXPECT_EQ(element_states_to_inject.size(), notify()->element_states().size());
  for (const auto& [element_id, state_received] : notify()->element_states()) {
    // Compare to actual static values we know.
    if (element_id == FakeComposite::kMuteElementId) {
      EXPECT_FALSE(state_received.type_specific().has_value());
      EXPECT_FALSE(state_received.vendor_specific_data().has_value());

      ASSERT_TRUE(state_received.started().has_value());
      EXPECT_TRUE(*state_received.started());

      ASSERT_TRUE(state_received.bypassed().has_value());
      EXPECT_FALSE(*state_received.bypassed());

      EXPECT_FALSE(state_received.processing_delay().has_value());
    } else {
      ASSERT_TRUE(state_received.type_specific().has_value());
      ASSERT_EQ(
          state_received.type_specific()->Which(),
          fuchsia_hardware_audio_signalprocessing::TypeSpecificElementState::Tag::kDaiInterconnect);
      ASSERT_TRUE(state_received.type_specific()->dai_interconnect().has_value());
      ASSERT_TRUE(state_received.type_specific()->dai_interconnect()->plug_state().has_value());
      ASSERT_TRUE(
          state_received.type_specific()->dai_interconnect()->plug_state()->plugged().has_value());
      ASSERT_TRUE(state_received.type_specific()
                      ->dai_interconnect()
                      ->plug_state()
                      ->plug_state_time()
                      .has_value());
      EXPECT_EQ(
          *state_received.type_specific()->dai_interconnect()->plug_state()->plug_state_time(),
          plug_change_time_to_inject.get());
      ASSERT_TRUE(state_received.type_specific()->dai_interconnect()->external_delay().has_value());
      EXPECT_EQ(*state_received.type_specific()->dai_interconnect()->external_delay(),
                ZX_MSEC(element_id));

      ASSERT_TRUE(state_received.vendor_specific_data().has_value());
      ASSERT_EQ(state_received.vendor_specific_data()->size(), 17u);
      EXPECT_EQ(state_received.vendor_specific_data()->at(16), 'Z');

      ASSERT_TRUE(state_received.started().has_value());
      EXPECT_TRUE(state_received.started());

      ASSERT_TRUE(state_received.bypassed().has_value());
      EXPECT_FALSE(*state_received.bypassed());

      ASSERT_TRUE(state_received.processing_delay().has_value());
      EXPECT_EQ(*state_received.processing_delay(), ZX_USEC(element_id));
    }

    // Compare to what we injected.
    ASSERT_TRUE(element_states_to_inject.contains(element_id))
        << "WatchElementState response received for unknown element_id " << element_id;
    const auto& state_injected = element_states_to_inject.find(element_id)->second;
    EXPECT_EQ(state_received, state_injected);
  }
}

TEST_F(CompositeTest, WatchTopologyInitial) {
  auto fake_driver = MakeFakeComposite();
  fake_driver->InjectTopologyChange(FakeComposite::kDefaultTopologyId);
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(AddObserver(device));

  RunLoopUntilIdle();
  ASSERT_TRUE(notify()->topology_id().has_value());
  EXPECT_EQ(*notify()->topology_id(), FakeComposite::kDefaultTopologyId);
}

TEST_F(CompositeTest, WatchTopologyUpdate) {
  auto fake_driver = MakeFakeComposite();
  fake_driver->InjectTopologyChange(FakeComposite::kDefaultTopologyId);
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(AddObserver(device));

  RunLoopUntilIdle();
  ASSERT_TRUE(notify()->topology_id().has_value());
  EXPECT_EQ(*notify()->topology_id(), FakeComposite::kDefaultTopologyId);
  notify()->clear_topology_id();

  RunLoopUntilIdle();
  EXPECT_FALSE(notify()->topology_id().has_value());

  fake_driver->InjectTopologyChange(FakeComposite::kSubsequentTopologyId);

  RunLoopUntilIdle();
  ASSERT_TRUE(notify()->topology_id().has_value());
  EXPECT_EQ(*notify()->topology_id(), FakeComposite::kSubsequentTopologyId);
}

TEST_F(CompositeTest, SetTopology) {
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

  EXPECT_EQ(device->SetTopology(topology_id_to_set), ZX_OK);

  RunLoopUntilIdle();
  ASSERT_TRUE(notify()->topology_id().has_value());
  EXPECT_EQ(*notify()->topology_id(), topology_id_to_set);
}

TEST_F(CompositeTest, SetElementState) {
  auto fake_driver = MakeFakeComposite();
  auto device = InitializeDeviceForFakeComposite(fake_driver);
  ASSERT_TRUE(device->is_operational());
  ASSERT_TRUE(SetControl(device));

  RunLoopUntilIdle();
  ASSERT_TRUE(notify()->element_states().contains(FakeComposite::kMuteElementId));
  notify()->clear_element_states();
  fhasp::SettableElementState state{{
      .started = true,
      .bypassed = false,
  }};

  EXPECT_EQ(device->SetElementState(FakeComposite::kMuteElementId, state), ZX_OK);

  RunLoopUntilIdle();
  ASSERT_TRUE(notify()->element_states().contains(FakeComposite::kMuteElementId));
  auto new_state = notify()->element_states().find(FakeComposite::kMuteElementId)->second;

  EXPECT_FALSE(new_state.type_specific().has_value());
  EXPECT_FALSE(new_state.vendor_specific_data().has_value());

  ASSERT_TRUE(new_state.started().has_value());
  EXPECT_TRUE(*new_state.started());

  ASSERT_TRUE(new_state.bypassed().has_value());
  EXPECT_FALSE(*new_state.bypassed());

  EXPECT_FALSE(new_state.processing_delay().has_value());
}

}  // namespace
}  // namespace media_audio
