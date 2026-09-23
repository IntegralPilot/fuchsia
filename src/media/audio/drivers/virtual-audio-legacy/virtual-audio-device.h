// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_LEGACY_VIRTUAL_AUDIO_DEVICE_H_
#define SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_LEGACY_VIRTUAL_AUDIO_DEVICE_H_

#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <fidl/fuchsia.virtualaudio/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/ddk/device.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fit/result.h>
#include <lib/zx/clock.h>

#include <memory>

#include <audio-proto/audio-proto.h>
#include <fbl/ref_ptr.h>

namespace virtual_audio {

class VirtualAudioDriver;

// Controller for a `VirtualAudioDriver`.
//
// Each instance of this class represents two objects:
//
// 1. A virtual audio device in the device tree, represented by a `VirtualAudioDriver` object.
//    This device appears under `/dev/class/audio-{input,output}`, `/dev/class/dai`, or
//    `/dev/class/codec`.
//
// 2. A FIDL channel (`fuchsia.virtualaudio.Device`) which controls and monitors the device.
//
// The device lives until the controlling FIDL channel is closed or the device host process decides
// to remove the `VirtualAudioDriver`.
class VirtualAudioDevice : public fidl::WireServer<fuchsia_virtualaudio::Device>,
                           public std::enable_shared_from_this<VirtualAudioDevice> {
 public:
  // `on_shutdown` called when FIDL server and driver have completely shutdown.
  static fit::result<fuchsia_virtualaudio::Error, std::shared_ptr<VirtualAudioDevice>> Create(
      const fuchsia_virtualaudio::Configuration& cfg,
      fidl::ServerEnd<fuchsia_virtualaudio::Device> server, zx_device_t* dev_node,
      fit::closure on_shutdown);

  std::optional<bool> is_input() const { return is_input_; }

  // Executes the given task on the FIDL channel's main dispatcher thread.
  // Used to deliver callbacks or events from the driver execution domain.
  void PostToDispatcher(fit::closure task_to_post);

  // Shuts down the FIDL server and the driver.
  void ShutdownAsync();

  //
  // Implementation of virtualaudio.Device.
  // Event triggers (e.g. NotifySetFormat) may be called from any thread.
  //

  void GetFormat(GetFormatCompleter::Sync& completer) override;
  void NotifySetFormat(uint32_t frames_per_second, uint32_t sample_format, uint32_t num_channels,
                       zx_duration_t external_delay);

  void GetGain(GetGainCompleter::Sync& completer) override;
  void NotifySetGain(bool current_mute, bool current_agc, float current_gain_db);

  void GetBuffer(GetBufferCompleter::Sync& completer) override;
  void NotifyBufferCreated(zx::vmo ring_buffer_vmo, uint32_t num_ring_buffer_frames,
                           uint32_t notifications_per_ring);

  void SetNotificationFrequency(SetNotificationFrequencyRequestView request,
                                SetNotificationFrequencyCompleter::Sync& completer) override;

  void NotifyStart(zx_time_t start_time);
  void NotifyStop(zx_time_t stop_time, uint32_t ring_buffer_position);

  void GetPosition(GetPositionCompleter::Sync& completer) override;
  void NotifyPosition(zx_time_t monotonic_time, uint32_t ring_buffer_position);

  void ChangePlugState(ChangePlugStateRequestView request,
                       ChangePlugStateCompleter::Sync& completer) override;

  void AdjustClockRate(AdjustClockRateRequestView request,
                       AdjustClockRateCompleter::Sync& completer) override;

  // Public for std::make_shared. Use Create, not this ctor.
  VirtualAudioDevice(std::optional<bool> is_input, fit::closure on_shutdown);
  ~VirtualAudioDevice() override;

 private:
  // Called when `binding_` is unbound.
  static void OnFidlServerUnbound(VirtualAudioDevice* device, fidl::UnbindInfo unbind_info,
                                  fidl::ServerEnd<fuchsia_virtualaudio::Device> server_end);

  // Called by `driver_` when it has completely shutdown. May be called from any dispatcher.
  void OnDriverShutdown();

  const std::optional<bool> is_input_;
  async_dispatcher_t* const dispatcher_ = fdf::Dispatcher::GetCurrent()->async_dispatcher();

  // Will be set to `std::nullopt` when the FIDL server is unbound.
  std::optional<fidl::ServerBindingRef<fuchsia_virtualaudio::Device>> binding_;

  // Will be set to `std::nullptr` when the driver is completely shutdown.
  std::shared_ptr<VirtualAudioDriver> driver_;

  // Called when `binding_` is unbound and `driver_` is shutdown.
  fit::closure on_shutdown_;
};

}  // namespace virtual_audio

#endif  // SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_LEGACY_VIRTUAL_AUDIO_DEVICE_H_
