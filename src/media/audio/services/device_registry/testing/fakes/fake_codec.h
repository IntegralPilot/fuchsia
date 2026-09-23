// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_TESTING_FAKES_FAKE_CODEC_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_TESTING_FAKES_FAKE_CODEC_H_

#include <fidl/fuchsia.audio.device/cpp/fidl.h>
#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/markers.h>
#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/test_base.h>
#include <fidl/fuchsia.hardware.audio/cpp/natural_types.h>
#include <fidl/fuchsia.hardware.audio/cpp/test_base.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/channel.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>

#include <cstring>
#include <optional>
#include <string_view>

#include "src/media/audio/services/device_registry/basic_types.h"
#include "src/media/audio/services/device_registry/testing/fakes/logging.h"

namespace media_audio {

// This driver implements the audio driver interface and is configurable to simulate audio hardware.
class FakeCodec final
    : public fidl::testing::TestBase<fuchsia_hardware_audio::CodecConnector>,
      public fidl::testing::TestBase<fuchsia_hardware_audio::Codec>,
      public fidl::testing::TestBase<fuchsia_hardware_audio_signalprocessing::SignalProcessing> {
 public:
  static constexpr char kDefaultManufacturer[] = "fake_codec device manufacturer";
  static constexpr char kDefaultProduct[] = "fake_codec device product";
  static constexpr UniqueId kDefaultUniqueInstanceId{
      0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54, 0x32, 0x10,
      0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF,
  };
  static constexpr bool kDefaultIsInput = false;

  static constexpr uint32_t kDefaultNumberOfChannels = 1;
  static constexpr uint32_t kDefaultNumberOfChannels2 = 2;
  static constexpr fuchsia_hardware_audio::DaiSampleFormat kDefaultDaiSampleFormat =
      fuchsia_hardware_audio::DaiSampleFormat::kPcmSigned;
  static constexpr uint32_t kDefaultFrameRate = 48000;
  static constexpr uint8_t kDefaultBitsPerSlot = 32;
  static constexpr uint8_t kDefaultBitsPerSample = 16;

  static constexpr fuchsia_hardware_audio::PlugDetectCapabilities kDefaultDriverPlugCaps =
      fuchsia_hardware_audio::PlugDetectCapabilities::kCanAsyncNotify;
  static constexpr fuchsia_audio_device::PlugDetectCapabilities kDefaultPlugCaps =
      fuchsia_audio_device::PlugDetectCapabilities::kPluggable;

  static const fuchsia_hardware_audio::DaiFrameFormat kDefaultFrameFormat;
  static const std::vector<uint32_t> kDefaultNumberOfChannelsSet;
  static const std::vector<fuchsia_hardware_audio::DaiSampleFormat> kDefaultSampleFormatSet;
  static const std::vector<fuchsia_hardware_audio::DaiFrameFormat> kDefaultFrameFormatSet;
  static const std::vector<uint32_t> kDefaultFrameRateSet;
  static const std::vector<uint8_t> kDefaultBitsPerSlotSet;
  static const std::vector<uint8_t> kDefaultBitsPerSampleSet;
  static const fuchsia_hardware_audio::DaiSupportedFormats kDefaultDaiFormatSet;
  static const std::vector<fuchsia_hardware_audio::DaiSupportedFormats> kDefaultDaiFormatSets;

  FakeCodec(zx::channel server_end, zx::channel client_end, async_dispatcher_t* dispatcher);
  ~FakeCodec() override;

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    ADR_WARN_OBJECT() << name;
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  // This returns a fidl::client_end<Codec>. The driver will not start serving requests until Enable
  // is called, which is why the construction/Enable separation exists.
  fidl::ClientEnd<fuchsia_hardware_audio::Codec> Enable();
  void DropCodec();

  async_dispatcher_t* dispatcher() { return dispatcher_; }
  bool is_bound() const { return binding_.has_value(); }

  std::optional<bool> health_state() const { return healthy_; }
  void set_health_state(std::optional<bool> healthy) { healthy_ = healthy; }

  std::optional<bool> is_input() const { return is_input_; }
  void set_is_input(std::optional<bool> is_input) { is_input_ = is_input; }
  void set_device_manufacturer(std::optional<std::string> mfgr) { manufacturer_ = std::move(mfgr); }
  void set_device_product(std::optional<std::string> product) { product_ = std::move(product); }
  void set_stream_unique_id(std::optional<UniqueId> uid) {
    if (uid) {
      uid_ = UniqueId{};
      std::memcpy(uid_->data(), uid->data(), fuchsia_audio_device::kUniqueInstanceIdSize);
    } else {
      uid_.reset();
    }
  }
  void set_plug_detect_capabilities(
      std::optional<fuchsia_hardware_audio::PlugDetectCapabilities> plug_detect_capabilities) {
    plug_detect_capabilities_ = plug_detect_capabilities;
  }

  // By default, support 1 format set, limited to 2-channel, unsigned int 16-in-32, 48kHz, I2S.
  void SetDefaultFormatSets() {
    clear_format_sets();

    number_of_channels_ = kDefaultNumberOfChannelsSet;
    sample_formats_ = kDefaultSampleFormatSet;
    frame_formats_ = kDefaultFrameFormatSet;
    frame_rates_ = kDefaultFrameRateSet;
    bits_per_slot_ = kDefaultBitsPerSlotSet;
    bits_per_sample_ = kDefaultBitsPerSampleSet;
    format_sets_ = kDefaultDaiFormatSets;
  }
  void clear_format_sets() { format_sets_.clear(); }
  void add_format_set(fuchsia_hardware_audio::DaiSupportedFormats format_set) {
    format_sets_.push_back(std::move(format_set));
  }

  bool is_running() const { return is_running_; }
  zx::time mono_start_time() const { return mono_start_time_; }
  zx::time mono_stop_time() const { return mono_stop_time_; }

  // The returned optional will be empty if no |SetDaiFormat| command has been received, or if the
  // Codec's state has been reset.
  std::optional<fuchsia_hardware_audio::DaiFormat> selected_format() const {
    return selected_format_;
  }

  void set_external_delay(zx::duration external_delay) { external_delay_ = external_delay; }
  void clear_external_delay() { external_delay_.reset(); }
  void set_turn_on_delay(zx::duration turn_on_delay) { turn_on_delay_ = turn_on_delay; }
  void clear_turn_on_delay() { turn_on_delay_.reset(); }
  void set_turn_off_delay(zx::duration turn_off_delay) { turn_off_delay_ = turn_off_delay; }
  void clear_turn_off_delay() { turn_off_delay_.reset(); }

  // Explicitly trigger a plug change.
  void InjectPluggedAt(zx::time plug_time);
  void InjectUnpluggedAt(zx::time plug_time);
  void HandlePlugResponse();

  ////////////////////////////////////////////////////////////////
 private:
  static constexpr std::string_view kClassName = "FakeCodec";

  // fuchsia_hardware_audio::CodecConnector
  void Connect(ConnectRequest& request, ConnectCompleter::Sync& completer) override {
    FX_CHECK(!binding_.has_value()) << "Codec is already bound; it cannot have multiple clients";
    binding_ = fidl::BindServer(dispatcher(), std::move(request.codec_protocol()), this);
  }

  // fuchsia_hardware_audio::Codec Interface
  void Reset(ResetCompleter::Sync& completer) override;
  void GetProperties(GetPropertiesCompleter::Sync& completer) override;
  void Stop(StopCompleter::Sync& completer) override;
  void Start(StartCompleter::Sync& completer) override;
  void GetDaiFormats(GetDaiFormatsCompleter::Sync& completer) override;
  void SetDaiFormat(SetDaiFormatRequest& request, SetDaiFormatCompleter::Sync& completer) override;
  void WatchPlugState(WatchPlugStateCompleter::Sync& completer) override;

  // fuchsia_hardware_audio.Health
  void GetHealthState(GetHealthStateCompleter::Sync& completer) override;

  // fuchsia_hardware_audio_signalprocessing.Connector
  void SignalProcessingConnect(SignalProcessingConnectRequest& request,
                               SignalProcessingConnectCompleter::Sync& completer) override;

  // fuchsia_hardware_audio_signalprocessing::SignalProcessing
  // These won't be called until SignalProcessingConnect is implemented, but we'll be safe.
  // TODO(https://fxbug.dev/323270827): implement signalprocessing for Codec (topology, gain).
  void GetElements(GetElementsCompleter::Sync& completer) final {
    ADR_LOG_OBJECT(kLogFakeCodec);
    completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
  }
  void WatchElementState(WatchElementStateRequest& request,
                         WatchElementStateCompleter::Sync& completer) final {
    ADR_LOG_OBJECT(kLogFakeCodec);
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }
  void GetTopologies(GetTopologiesCompleter::Sync& completer) final {
    ADR_LOG_OBJECT(kLogFakeCodec);
    completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
  }
  void WatchTopology(WatchTopologyCompleter::Sync& completer) final {
    ADR_LOG_OBJECT(kLogFakeCodec);
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }
  void SetElementState(SetElementStateRequest& request,
                       SetElementStateCompleter::Sync& completer) final {
    ADR_LOG_OBJECT(kLogFakeCodec);
    completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
  }
  void SetTopology(SetTopologyRequest& request, SetTopologyCompleter::Sync& completer) final {
    ADR_LOG_OBJECT(kLogFakeCodec);
    completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
  }
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_audio_signalprocessing::SignalProcessing>
          metadata,
      fidl::UnknownMethodCompleter::Sync& completer) final {
    ADR_WARN_METHOD() << "(SignalProcessing) ordinal " << metadata.method_ordinal;
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  bool CheckDaiFormatSupported(const fuchsia_hardware_audio::DaiFormat& candidate);

  async_dispatcher_t* dispatcher_;
  fidl::ServerEnd<fuchsia_hardware_audio::Codec> server_end_;
  fidl::ClientEnd<fuchsia_hardware_audio::Codec> client_end_;
  std::optional<fidl::ServerBindingRef<fuchsia_hardware_audio::Codec>> binding_;
  std::optional<fidl::ServerBindingRef<fuchsia_hardware_audio_signalprocessing::SignalProcessing>>
      signal_processing_binding_;

  std::optional<bool> healthy_ = true;

  bool supports_signalprocessing_ = false;

  std::optional<std::string> manufacturer_ = kDefaultManufacturer;
  std::optional<std::string> product_ = kDefaultProduct;
  std::optional<UniqueId> uid_ = kDefaultUniqueInstanceId;
  std::optional<bool> is_input_ = kDefaultIsInput;
  std::optional<fuchsia_hardware_audio::PlugDetectCapabilities> plug_detect_capabilities_ =
      kDefaultDriverPlugCaps;

  bool is_running_ = false;
  zx::time mono_start_time_{zx::time::infinite_past()};
  zx::time mono_stop_time_{zx::time::infinite_past()};

  // Format sets
  std::vector<fuchsia_hardware_audio::DaiSupportedFormats> format_sets_ = kDefaultDaiFormatSets;
  // The default values for these six vectors are set by SetDefaultFormats(), in the ctor.
  std::vector<uint32_t> number_of_channels_;
  std::vector<fuchsia_hardware_audio::DaiSampleFormat> sample_formats_;
  std::vector<fuchsia_hardware_audio::DaiFrameFormat> frame_formats_;
  std::vector<uint32_t> frame_rates_;
  std::vector<uint8_t> bits_per_slot_;
  std::vector<uint8_t> bits_per_sample_;

  // Current format
  std::optional<fuchsia_hardware_audio::DaiFormat> selected_format_;
  // CodecFormatInfo, returned when the format is set
  std::optional<zx::duration> external_delay_;
  std::optional<zx::duration> turn_on_delay_;
  std::optional<zx::duration> turn_off_delay_;

  // Plug state
  // Always respond to the first hanging-get request.
  bool plug_has_changed_ = true;
  std::optional<WatchPlugStateCompleter::Async> watch_plug_state_completer_;
  bool plugged_ = true;
  zx::time plug_state_time_{zx::time::infinite_past()};
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_TESTING_FAKES_FAKE_CODEC_H_
