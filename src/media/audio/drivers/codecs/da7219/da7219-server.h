// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_CODECS_DA7219_DA7219_SERVER_H_
#define SRC_MEDIA_AUDIO_DRIVERS_CODECS_DA7219_DA7219_SERVER_H_

#include <fidl/fuchsia.hardware.audio/cpp/wire.h>
#include <fidl/fuchsia.hardware.i2c/cpp/wire.h>
#include <lib/async/cpp/irq.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/zx/interrupt.h>

#ifdef LOGGING_PATH
#include LOGGING_PATH
#else
#error "Must have logging path"
#endif

namespace audio::da7219 {

// Only one core for both server instantiations since the is one hardware servicing both
// an input and an output servers.
class Core {
 public:
  explicit Core(fidl::ClientEnd<fuchsia_hardware_i2c::Device> i2c, zx::interrupt irq,
                async_dispatcher_t* dispatcher);
  ~Core() = default;

  using PlugCallback = fit::function<void(bool)>;

  void Shutdown();
  fidl::ClientEnd<fuchsia_hardware_i2c::Device>& i2c() { return i2c_; }
  async_dispatcher_t* dispatcher() { return dispatcher_; }
  zx_status_t Reset();
  zx_status_t Initialize();
  void AddPlugCallback(bool is_input, PlugCallback cb);

 private:
  void HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq, zx_status_t status,
                 const zx_packet_interrupt_t* interrupt);
  void PlugDetected(bool plugged, bool with_mic);

  fidl::ClientEnd<fuchsia_hardware_i2c::Device> i2c_;
  zx::interrupt irq_;
  async::IrqMethod<Core, &Core::HandleIrq> irq_handler_{this};
  std::optional<PlugCallback> plug_callback_input_;
  std::optional<PlugCallback> plug_callback_output_;
  async_dispatcher_t* dispatcher_;
};

class Server final
    : public fidl::WireServer<fuchsia_hardware_audio::Codec>,
      public fidl::WireServer<fuchsia_hardware_audio_signalprocessing::SignalProcessing> {
 public:
  explicit Server(std::shared_ptr<Core> core, bool is_input);
  async_dispatcher_t* dispatcher() { return core_->dispatcher(); }

 private:
  static constexpr fuchsia_hardware_audio::wire::TopologyId kTopologyId = 1;
  static constexpr fuchsia_hardware_audio::wire::ElementId kHeadphoneGainPeId =
      1;  // Processing element.

  static constexpr float kMinHeadphoneGainDb = -57.0f;
  static constexpr float kMaxHeadphoneGainDb = 6.0f;
  static constexpr float kGainStepHeadphoneGainDb = 1.0f;

  // LLCPP implementation for the Codec API.
  void Reset(ResetCompleter::Sync& completer) override;
  void Stop(StopCompleter::Sync& completer) override;
  void Start(StartCompleter::Sync& completer) override;
  void GetProperties(GetPropertiesCompleter::Sync& completer) override;
  void GetHealthState(GetHealthStateCompleter::Sync& completer) override;
  void GetDaiFormats(GetDaiFormatsCompleter::Sync& completer) override;
  void SetDaiFormat(SetDaiFormatRequestView request,
                    SetDaiFormatCompleter::Sync& completer) override;
  void WatchPlugState(WatchPlugStateCompleter::Sync& completer) override;
  void SignalProcessingConnect(SignalProcessingConnectRequestView request,
                               SignalProcessingConnectCompleter::Sync& completer) override;

  // LLCPP implementation for the SignalProcessing API.
  void GetElements(GetElementsCompleter::Sync& completer) override;
  void WatchElementState(WatchElementStateRequestView request,
                         WatchElementStateCompleter::Sync& completer) override;
  void GetTopologies(GetTopologiesCompleter::Sync& completer) override;
  void WatchTopology(WatchTopologyCompleter::Sync& completer) override;
  void SetElementState(SetElementStateRequestView request,
                       SetElementStateCompleter::Sync& completer) override;
  void SetTopology(SetTopologyRequestView request, SetTopologyCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_audio_signalprocessing::SignalProcessing>
          metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  void MaybeCompleteWatchElementState();

  std::shared_ptr<Core> core_;
  bool is_input_;
  std::optional<fidl::ServerBinding<fuchsia_hardware_audio_signalprocessing::SignalProcessing>>
      signal_;

  // Plug state. Must reply to the first Watch request, if there is no plug state update before the
  // first Watch, reply with unplugged at time 0.
  bool plugged_ = false;
  zx::time plugged_time_;
  bool plug_state_updated_ = true;
  std::optional<WatchPlugStateCompleter::Async> plug_state_completer_;

  bool last_gain_update_reported_ = false;  // So we return the gain state on the first call.
  float gain_ = 0.0f;
  std::optional<WatchElementStateCompleter::Async> gain_completer_;

  bool responded_to_watch_topology_ = false;
  std::optional<WatchTopologyCompleter::Async> topology_completer_;
};

}  // namespace audio::da7219

#endif  // SRC_MEDIA_AUDIO_DRIVERS_CODECS_DA7219_DA7219_SERVER_H_
