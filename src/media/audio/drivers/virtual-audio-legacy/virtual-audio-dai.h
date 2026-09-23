// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_LEGACY_VIRTUAL_AUDIO_DAI_H_
#define SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_LEGACY_VIRTUAL_AUDIO_DAI_H_

#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <fidl/fuchsia.virtualaudio/cpp/wire.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/platform-defs.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <ddktl/device.h>

#include "src/media/audio/drivers/virtual-audio-legacy/virtual-audio-device.h"
#include "src/media/audio/drivers/virtual-audio-legacy/virtual-audio-driver.h"

namespace virtual_audio {

class VirtualAudioDai;
using VirtualAudioDaiDeviceType =
    ddk::Device<VirtualAudioDai, ddk::Messageable<fuchsia_hardware_audio::DaiConnector>::Mixin>;

class VirtualAudioDai final : public VirtualAudioDaiDeviceType,
                              public ddk::internal::base_protocol,
                              public fidl::Server<fuchsia_hardware_audio::Dai>,
                              public fidl::Server<fuchsia_hardware_audio::RingBuffer>,
                              public VirtualAudioDriver {
 public:
  static fuchsia_virtualaudio::Configuration GetDefaultConfig(bool is_input);

  VirtualAudioDai(fuchsia_virtualaudio::Configuration config,
                  std::weak_ptr<VirtualAudioDevice> owner, zx_device_t* parent,
                  fit::closure on_shutdown);
  ~VirtualAudioDai() override;
  void ResetDaiState() { dai_bindings_.RemoveAll(); }
  void ShutdownAsync() override;
  void DdkRelease();

  // VirtualAudioDriver overrides.
  // TODO(https://fxbug.dev/42075676): Add support for GetPositionForVA,
  // SetNotificationFrequencyFromVA and AdjustClockRateFromVA.
  using ErrorT = fuchsia_virtualaudio::Error;
  void GetFormatForVA(fit::callback<void(fit::result<ErrorT, CurrentFormat>)> callback) override;
  void GetBufferForVA(fit::callback<void(fit::result<ErrorT, CurrentBuffer>)> callback) override;

 protected:
  // FIDL LLCPP method for fuchsia.hardware.audio.DaiConnector.
  void Connect(ConnectRequestView request, ConnectCompleter::Sync& completer) override {
    zxlogf(INFO, "VirtualAudioDai is connecting!     (%p)", this);
    dai_bindings_.AddBinding(dispatcher_, std::move(request->dai_protocol), this,
                             [weak = weak_from_this()](VirtualAudioDai*, fidl::UnbindInfo) {
                               if (auto self = weak.lock()) {
                                 zxlogf(INFO, "VirtualAudioDai is disconnecting!  (%p)",
                                        self.get());
                               } else {
                                 zxlogf(INFO, "VirtualAudioDai is disconnecting!  (unknown)");
                               }
                             });
  }

  // FIDL natural C++ methods for fuchsia.hardware.audio.Dai.
  void Reset(ResetCompleter::Sync& completer) override { completer.Reply(); }
  void GetProperties(
      fidl::Server<fuchsia_hardware_audio::Dai>::GetPropertiesCompleter::Sync& completer) override;
  void GetHealthState(GetHealthStateCompleter::Sync& completer) override {
    completer.Reply(fuchsia_hardware_audio::HealthState{}.healthy(true));
  }
  void SignalProcessingConnect(SignalProcessingConnectRequest& request,
                               SignalProcessingConnectCompleter::Sync& completer) override {
    request.protocol().Close(ZX_ERR_NOT_SUPPORTED);
  }
  void GetRingBufferFormats(GetRingBufferFormatsCompleter::Sync& completer) override;
  void GetDaiFormats(GetDaiFormatsCompleter::Sync& completer) override;
  void CreateRingBuffer(CreateRingBufferRequest& request,
                        CreateRingBufferCompleter::Sync& completer) override;

  // FIDL natural C++ methods for fuchsia.hardware.audio.RingBuffer.
  void GetProperties(fidl::Server<fuchsia_hardware_audio::RingBuffer>::GetPropertiesCompleter::Sync&
                         completer) override;
  void GetVmo(
      GetVmoRequest& request,
      fidl::Server<fuchsia_hardware_audio::RingBuffer>::GetVmoCompleter::Sync& completer) override;
  void Start(StartCompleter::Sync& completer) override;
  void Stop(StopCompleter::Sync& completer) override;
  void WatchClockRecoveryPositionInfo(
      WatchClockRecoveryPositionInfoCompleter::Sync& completer) override;
  void WatchDelayInfo(WatchDelayInfoCompleter::Sync& completer) override;
  void SetActiveChannels(fuchsia_hardware_audio::RingBufferSetActiveChannelsRequest& request,
                         SetActiveChannelsCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_audio::RingBuffer> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  void ResetRingBuffer();
  fuchsia_virtualaudio::Dai& dai_config() { return config_.device_specific()->dai().value(); }

  // This should never be invalid: this VirtualAudioStream should always be destroyed before
  // its parent. This field is a weak_ptr to avoid a circular reference count.
  const std::weak_ptr<VirtualAudioDevice> parent_;
  static int instance_count_;
  char instance_name_[64];
  fidl::ServerBindingGroup<fuchsia_hardware_audio::Dai> dai_bindings_;
  std::optional<fidl::ServerBindingRef<fuchsia_hardware_audio::RingBuffer>> ring_buffer_binding_;

  fzl::VmoMapper ring_buffer_mapper_;
  uint32_t notifications_per_ring_ = 0;
  uint32_t num_ring_buffer_frames_ = 0;
  uint32_t frame_size_ = 4;
  zx::vmo ring_buffer_vmo_;
  bool should_reply_to_delay_request_ = true;
  std::optional<WatchDelayInfoCompleter::Async> delay_info_completer_;
  bool should_reply_to_position_request_ = true;
  std::optional<WatchClockRecoveryPositionInfoCompleter::Async> position_info_completer_;

  bool ring_buffer_vmo_fetched_ = false;
  bool ring_buffer_started_ = false;
  std::optional<fuchsia_hardware_audio::Format> ring_buffer_format_;
  uint64_t ring_buffer_active_channel_mask_;
  zx::time active_channel_set_time_;

  std::optional<fuchsia_hardware_audio::DaiFormat> dai_format_;
  fuchsia_virtualaudio::Configuration config_;
  async_dispatcher_t* dispatcher_ = fdf::Dispatcher::GetCurrent()->async_dispatcher();
};

}  // namespace virtual_audio

#endif  // SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_LEGACY_VIRTUAL_AUDIO_DAI_H_
