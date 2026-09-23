// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/testing/fakes/fake_codec.h"

#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/markers.h>
#include <fidl/fuchsia.hardware.audio/cpp/natural_types.h>
#include <lib/fit/result.h>
#include <zircon/errors.h>

#include <iterator>

#include <gtest/gtest.h>

namespace media_audio {

namespace fha = fuchsia_hardware_audio;

namespace {

void on_unbind(FakeCodec* fake_codec, fidl::UnbindInfo info,
               fidl::ServerEnd<fha::Codec> server_end) {
  ADR_LOG(kLogFakeCodec || kLogObjectLifetimes) << "FakeCodec disconnected";
}

}  // namespace
const fha::DaiFrameFormat FakeCodec::kDefaultFrameFormat =
    fha::DaiFrameFormat::WithFrameFormatStandard(fha::DaiFrameFormatStandard::kI2S);

const std::vector<uint32_t> FakeCodec::kDefaultNumberOfChannelsSet{
    FakeCodec::kDefaultNumberOfChannels,
    FakeCodec::kDefaultNumberOfChannels2,
};
const std::vector<fha::DaiSampleFormat> FakeCodec::kDefaultSampleFormatSet{
    FakeCodec::kDefaultDaiSampleFormat};
const std::vector<fha::DaiFrameFormat> FakeCodec::kDefaultFrameFormatSet{
    FakeCodec::kDefaultFrameFormat};
const std::vector<uint32_t> FakeCodec::kDefaultFrameRateSet{FakeCodec::kDefaultFrameRate};
const std::vector<uint8_t> FakeCodec::kDefaultBitsPerSlotSet{FakeCodec::kDefaultBitsPerSlot};
const std::vector<uint8_t> FakeCodec::kDefaultBitsPerSampleSet{FakeCodec::kDefaultBitsPerSample};
const fha::DaiSupportedFormats FakeCodec::kDefaultDaiFormatSet{{
    .number_of_channels = FakeCodec::kDefaultNumberOfChannelsSet,
    .sample_formats = FakeCodec::kDefaultSampleFormatSet,
    .frame_formats = FakeCodec::kDefaultFrameFormatSet,
    .frame_rates = FakeCodec::kDefaultFrameRateSet,
    .bits_per_slot = FakeCodec::kDefaultBitsPerSlotSet,
    .bits_per_sample = FakeCodec::kDefaultBitsPerSampleSet,
}};

const std::vector<fha::DaiSupportedFormats> FakeCodec::kDefaultDaiFormatSets{
    FakeCodec::kDefaultDaiFormatSet};

FakeCodec::FakeCodec(zx::channel server_end, zx::channel client_end, async_dispatcher_t* dispatcher)
    : dispatcher_(dispatcher),
      server_end_(std::move(server_end)),
      client_end_(std::move(client_end)) {
  ADR_LOG_METHOD(kLogFakeCodec || kLogObjectLifetimes);

  set_stream_unique_id(kDefaultUniqueInstanceId);
  SetDefaultFormatSets();
}

FakeCodec::~FakeCodec() {
  ADR_LOG_METHOD(kLogFakeCodec || kLogObjectLifetimes);
  signal_processing_binding_.reset();
  binding_.reset();
}

fidl::ClientEnd<fha::Codec> FakeCodec::Enable() {
  ADR_LOG_METHOD(kLogFakeCodec);
  EXPECT_TRUE(server_end_.is_valid());
  EXPECT_TRUE(client_end_.is_valid());
  EXPECT_TRUE(dispatcher_);
  EXPECT_FALSE(binding_);

  binding_ = fidl::BindServer(dispatcher_, std::move(server_end_), this, &on_unbind);
  EXPECT_FALSE(server_end_.is_valid());

  return std::move(client_end_);
}

void FakeCodec::DropCodec() {
  ADR_LOG_METHOD(kLogFakeCodec);
  watch_plug_state_completer_.reset();
  binding_->Close(ZX_ERR_PEER_CLOSED);
}

void FakeCodec::GetHealthState(GetHealthStateCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeCodec);
  if (healthy_.has_value()) {
    completer.Reply(fha::HealthState{{
        healthy_,
    }});
  } else {
    completer.Reply({});
  }
}

void FakeCodec::SignalProcessingConnect(SignalProcessingConnectRequest& request,
                                        SignalProcessingConnectCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeCodec);
  if (!supports_signalprocessing_) {
    request.protocol().Close(ZX_ERR_NOT_SUPPORTED);
    return;
  }

  // TODO(https://fxbug.dev/323270827): implement signalprocessing for Codec (topology, gain).
  // Not yet implemented.
  signal_processing_binding_ =
      fidl::BindServer(dispatcher_,
                       fidl::ServerEnd<fuchsia_hardware_audio_signalprocessing::SignalProcessing>(
                           request.protocol().TakeChannel()),
                       this);
  signal_processing_binding_->Close(ZX_ERR_NOT_SUPPORTED);
}

void FakeCodec::GetProperties(GetPropertiesCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeCodec);
  // Gather the properties and return them.
  fha::CodecProperties codec_properties{};
  if (is_input_.has_value()) {
    codec_properties.is_input(is_input_);
  }
  if (manufacturer_.has_value()) {
    codec_properties.manufacturer(manufacturer_);
  }
  if (product_.has_value()) {
    codec_properties.product(product_);
  }
  if (uid_.has_value()) {
    codec_properties.unique_id(uid_);
  }
  if (plug_detect_capabilities_.has_value()) {
    codec_properties.plug_detect_capabilities(plug_detect_capabilities_);
  }

  completer.Reply(codec_properties);
}

void FakeCodec::GetDaiFormats(GetDaiFormatsCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeCodec);

  completer.Reply(fit::ok(std::vector<fha::DaiSupportedFormats>{
      {{
          .number_of_channels = number_of_channels_,
          .sample_formats = sample_formats_,
          .frame_formats = frame_formats_,
          .frame_rates = frame_rates_,
          .bits_per_slot = bits_per_slot_,
          .bits_per_sample = bits_per_sample_,
      }},
  }));
}

bool FakeCodec::CheckDaiFormatSupported(const fha::DaiFormat& candidate) {
  return (std::find(number_of_channels_.begin(), number_of_channels_.end(),
                    candidate.number_of_channels()) != std::end(number_of_channels_) &&
          candidate.channels_to_use_bitmask() < (1u << candidate.number_of_channels()) &&
          std::find(sample_formats_.begin(), sample_formats_.end(), candidate.sample_format()) !=
              std::end(sample_formats_) &&
          std::find(frame_formats_.begin(), frame_formats_.end(), candidate.frame_format()) !=
              std::end(frame_formats_) &&
          std::find(frame_rates_.begin(), frame_rates_.end(), candidate.frame_rate()) !=
              std::end(frame_rates_) &&
          std::find(bits_per_slot_.begin(), bits_per_slot_.end(), candidate.bits_per_slot()) !=
              std::end(bits_per_slot_) &&
          std::find(bits_per_sample_.begin(), bits_per_sample_.end(),
                    candidate.bits_per_sample()) != std::end(bits_per_sample_));
}
void FakeCodec::SetDaiFormat(SetDaiFormatRequest& request, SetDaiFormatCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeCodec);

  if (!CheckDaiFormatSupported(request.format())) {
    completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
    return;
  }

  selected_format_.emplace(request.format());

  fha::CodecFormatInfo info;
  if (external_delay_.has_value()) {
    info.external_delay(external_delay_->get());
  }
  if (turn_on_delay_.has_value()) {
    info.turn_on_delay(turn_on_delay_->get());
  }
  if (turn_off_delay_.has_value()) {
    info.turn_off_delay(turn_off_delay_->get());
  }

  completer.Reply(fit::ok(info));
}

void FakeCodec::Start(StartCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeCodec);

  EXPECT_TRUE(selected_format_);
  if (!is_running_) {
    mono_start_time_ = zx::clock::get_monotonic();
    is_running_ = true;
  }

  completer.Reply(mono_start_time_.get());
}

void FakeCodec::Stop(StopCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeCodec);

  if (is_running_) {
    mono_stop_time_ = zx::clock::get_monotonic();
    is_running_ = false;
  }

  completer.Reply(mono_stop_time_.get());
}

void FakeCodec::Reset(ResetCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeCodec);

  if (is_running_) {
    mono_stop_time_ = zx::clock::get_monotonic();
    is_running_ = false;
  }
  if (selected_format_.has_value()) {
    selected_format_.reset();
    external_delay_.reset();
    turn_on_delay_.reset();
    turn_off_delay_.reset();
  }

  // TODO(https://fxbug.dev/323270827): implement signalprocessing for Codec (topology, gain).
  // Reset all signalprocessing Elements and the signalprocessing Topology.

  completer.Reply();
}

void FakeCodec::WatchPlugState(WatchPlugStateCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeCodec);

  ASSERT_FALSE(watch_plug_state_completer_.has_value())
      << " received second WatchPlugState while first is still pending";

  // If pending plug state change, clear the dirty bit and return the state with this completer.
  if (plug_has_changed_) {
    plug_has_changed_ = false;
    completer.Reply(fha::PlugState{{
        .plugged = plugged_,
        .plug_state_time = plug_state_time_.get(),
    }});
  } else {
    watch_plug_state_completer_ = completer.ToAsync();
  }
}

void FakeCodec::InjectPluggedAt(zx::time plug_time) {
  ADR_LOG_METHOD(kLogFakeCodec) << "(plugged, " << plug_time.get() << ")";
  ASSERT_TRUE(!watch_plug_state_completer_.has_value() || !plug_has_changed_)
      << "Inconsistent state: WatchPlugState is pending, but a plug change is waiting to be reported";

  // Compare to previously-reported plug state: only report _changes_ that occurred _later_.
  if (!plugged_ && plug_time > plug_state_time_) {
    plugged_ = true;
    plug_state_time_ = plug_time;

    HandlePlugResponse();
  }
}

void FakeCodec::InjectUnpluggedAt(zx::time plug_time) {
  ADR_LOG_METHOD(kLogFakeCodec) << "(unplugged, " << plug_time.get() << ")";
  ASSERT_TRUE(!watch_plug_state_completer_.has_value() || !plug_has_changed_)
      << "Inconsistent state: WatchPlugState is pending, but a plug change is waiting to be reported";

  // Compare to previously-reported plug state: only report _changes_ that occurred _later_.
  if (plugged_ && plug_time > plug_state_time_) {
    plugged_ = false;
    plug_state_time_ = plug_time;

    HandlePlugResponse();
  }
}

void FakeCodec::HandlePlugResponse() {
  // A WatchPlugState is pending; complete it.
  if (watch_plug_state_completer_.has_value()) {
    watch_plug_state_completer_->Reply(fha::PlugState{{
        .plugged = plugged_,
        .plug_state_time = plug_state_time_.get(),
    }});
    watch_plug_state_completer_.reset();
    plug_has_changed_ = false;
  } else {  // Pend this plug change for a WatchPlugState call in the future.
    plug_has_changed_ = true;
  }
}

}  // namespace media_audio
