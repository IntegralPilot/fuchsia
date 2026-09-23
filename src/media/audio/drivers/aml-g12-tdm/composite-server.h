// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_AML_G12_TDM_COMPOSITE_SERVER_H_
#define SRC_MEDIA_AUDIO_DRIVERS_AML_G12_TDM_COMPOSITE_SERVER_H_

#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/fidl.h>
#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <fidl/fuchsia.hardware.clock/cpp/fidl.h>
#include <fidl/fuchsia.hardware.gpio/cpp/fidl.h>
#include <fidl/fuchsia.hardware.pin/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/fit/defer.h>
#include <lib/fzl/pinned-vmo.h>
#include <lib/trace/event.h>
#include <lib/zx/result.h>

#include <bitset>

#include "src/media/audio/drivers/aml-g12-tdm/aml-tdm-config-device.h"
#include "src/media/audio/drivers/lib/inspect/recorder.h"

namespace audio::aml_g12 {

constexpr size_t kNumberOfPipelines = 3;
constexpr size_t kNumberOfTdmEngines = 2 * kNumberOfPipelines;  // 2, 1 for input 1 for output.

class RingBufferServer;
class AudioCompositeServer;

struct Engine {
  inline Engine();

  size_t ring_buffer_index;
  size_t dai_index;
  std::optional<AmlTdmConfigDevice> device;
  std::unique_ptr<RingBufferServer> ring_buffer;
  fuchsia_hardware_audio::Format2 ring_buffer_format;
  metadata::AmlConfig config;
};

struct SclkPin {
  fidl::WireSyncClient<fuchsia_hardware_gpio::Gpio> gpio;
  fidl::WireSyncClient<fuchsia_hardware_pin::Pin> pin;
};

class RingBufferServer final : public fidl::Server<fuchsia_hardware_audio::RingBuffer> {
 public:
  static std::unique_ptr<RingBufferServer> CreateRingBufferServer(
      async_dispatcher_t* dispatcher, AudioCompositeServer& owner, size_t engine_index,
      fidl::ServerEnd<fuchsia_hardware_audio::RingBuffer> ring_buffer, uint64_t element_id);
  RingBufferServer(async_dispatcher_t* dispatcher, AudioCompositeServer& owner, size_t engine_index,
                   fidl::ServerEnd<fuchsia_hardware_audio::RingBuffer> ring_buffer,
                   uint64_t element_id);
  void Unbind(zx_status_t status) { binding_.Close(status); }

 protected:
  // FIDL natural C++ methods for fuchsia.hardware.audio.RingBuffer.
  void GetProperties(GetPropertiesCompleter::Sync& completer) override;
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

  RingBufferRecorder& ring_buffer_inspect() { return ring_buffer_recorder_; }
  std::unique_ptr<Recorder>& device_inspect();

 private:
  void OnRingBufferClosed(fidl::UnbindInfo info);
  zx_status_t InitBuffer(size_t size);
  void ProcessRingNotification();
  void ResetRingBuffer();

  Engine& engine_;
  size_t engine_index_;
  async_dispatcher_t* dispatcher_;
  AudioCompositeServer& owner_;
  fidl::ServerBinding<fuchsia_hardware_audio::RingBuffer> binding_;

  zx::duration notification_period_{0};
  bool started_ = false;
  bool fetched_ = false;
  zx::duration internal_delay_{0};

  async::TaskClosureMethod<RingBufferServer, &RingBufferServer::ProcessRingNotification>
      notify_timer_{this};
  zx::vmo ring_buffer_vmo_;
  fzl::PinnedVmo pinned_ring_buffer_;
  std::atomic<uint32_t> expected_notifications_per_ring_{0};

  std::optional<WatchClockRecoveryPositionInfoCompleter::Async> position_completer_;
  struct DelayCompleter {
    // One-shot flag that indicates whether or not WatchDelayInfo has been called yet.
    bool first_response_sent;
    std::optional<WatchDelayInfoCompleter::Async> completer;
  };
  DelayCompleter delay_completer_;

  RingBufferRecorder& ring_buffer_recorder_;
};

class AudioCompositeServer final
    : public fidl::Server<fuchsia_hardware_audio::Composite>,
      public fidl::Server<fuchsia_hardware_audio_signalprocessing::SignalProcessing> {
  friend class RingBufferServer;

 public:
  static constexpr std::array<fuchsia_hardware_audio::ElementId, kNumberOfPipelines> kDaiIds = {
      1, 2, 3};
  static constexpr std::array<fuchsia_hardware_audio::ElementId, kNumberOfTdmEngines>
      kRingBufferIds = {4, 5, 6, 7, 8, 9};

  AudioCompositeServer(std::array<std::optional<fdf::MmioBuffer>, kNumberOfTdmEngines> mmios,
                       zx::bti bti, async_dispatcher_t* dispatcher,
                       metadata::AmlVersion aml_version,
                       fidl::WireSyncClient<fuchsia_hardware_clock::Clock> clock_gate_client,
                       fidl::WireSyncClient<fuchsia_hardware_clock::Clock> pll_client,
                       std::vector<SclkPin> sclk_clients, std::unique_ptr<Recorder> recorder);

  async_dispatcher_t* dispatcher() { return dispatcher_; }
  zx::bti& bti() { return bti_; }
  fuchsia_hardware_audio::DaiFormat& current_dai_formats(size_t dai_index) {
    ZX_ASSERT(current_dai_formats_[dai_index].has_value());
    return *current_dai_formats_[dai_index];
  }

 protected:
  // FIDL natural C++ methods for fuchsia.hardware.audio.Composite.
  void Reset(ResetCompleter::Sync& completer) override;
  void GetProperties(GetPropertiesCompleter::Sync& completer) override;
  void GetHealthState(GetHealthStateCompleter::Sync& completer) override;
  void SignalProcessingConnect(SignalProcessingConnectRequest& request,
                               SignalProcessingConnectCompleter::Sync& completer) override;
  void GetRingBufferFormats(GetRingBufferFormatsRequest& request,
                            GetRingBufferFormatsCompleter::Sync& completer) override;
  void CreateRingBuffer(CreateRingBufferRequest& request,
                        CreateRingBufferCompleter::Sync& completer) override;
  void GetDaiFormats(GetDaiFormatsRequest& request,
                     GetDaiFormatsCompleter::Sync& completer) override;
  void SetDaiFormat(SetDaiFormatRequest& request, SetDaiFormatCompleter::Sync& completer) override;
  void GetPacketStreamFormats(GetPacketStreamFormatsRequest& request,
                              GetPacketStreamFormatsCompleter::Sync& completer) override;
  void CreatePacketStream(CreatePacketStreamRequest& request,
                          CreatePacketStreamCompleter::Sync& completer) override;

  // FIDL natural C++ methods for fuchsia.hardware.audio.signalprocessing.SignalProcessing.
  void GetElements(GetElementsCompleter::Sync& completer) override;
  void WatchElementState(WatchElementStateRequest& request,
                         WatchElementStateCompleter::Sync& completer) override;
  void SetElementState(SetElementStateRequest& request,
                       SetElementStateCompleter::Sync& completer) override;
  void GetTopologies(GetTopologiesCompleter::Sync& completer) override;
  void WatchTopology(WatchTopologyCompleter::Sync& completer) override;
  void SetTopology(SetTopologyRequest& request, SetTopologyCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<
                                 typename fuchsia_hardware_audio_signalprocessing::SignalProcessing>
                                 metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_audio::Composite> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  std::unique_ptr<Recorder>& device_inspect() { return recorder_; }

 private:
  static constexpr fuchsia_hardware_audio::TopologyId kTopologyId = 1;

  static constexpr zx::duration kTimeToStabilizePll = zx::msec(10);

  struct ElementCompleter {
    // One-shot flag that indicates whether or not WatchElementState has been called
    // for this element yet.
    bool first_response_sent;
    std::optional<WatchElementStateCompleter::Async> completer;
  };
  struct TopologyCompleter {
    // One-shot flag that indicates whether or not WatchTopology has been called
    // for this topology yet.
    bool first_response_sent;
    std::optional<WatchTopologyCompleter::Async> completer;
  };

  void OnSignalProcessingClosed(fidl::UnbindInfo info);
  zx_status_t ResetEngine(size_t index);
  zx_status_t ConfigEngine(size_t index, size_t dai_index, bool input, fdf::MmioBuffer mmio,
                           metadata::AmlVersion aml_version);
  zx_status_t StartSocPower(bool wait_for_completion);
  zx_status_t StopSocPower();
  auto TemporarilyStartSocPower() {
    bool last_soc_power_stopped = false;
    if (!soc_power_started_) {
      last_soc_power_stopped = true;
      [[maybe_unused]] zx_status_t status = StartSocPower(/*wait_for_completion*/ true);
    }
    return fit::defer([last_soc_power_stopped, this]() {
      if (last_soc_power_stopped) {
        StopSocPower();
      }
    });
  }

  async_dispatcher_t* dispatcher_;
  zx::bti bti_;
  std::optional<fidl::ServerBinding<fuchsia_hardware_audio_signalprocessing::SignalProcessing>>
      signal_;

  TopologyCompleter topology_completer_ = {};

  std::unordered_map<fuchsia_hardware_audio::ElementId, ElementCompleter> element_completers_;
  std::array<Engine, kNumberOfTdmEngines> engines_;
  std::bitset<kNumberOfTdmEngines> engines_on_;
  std::array<fuchsia_hardware_audio::PcmSupportedFormats, kNumberOfTdmEngines>
      supported_ring_buffer_formats_;
  std::array<fuchsia_hardware_audio::DaiSupportedFormats, kNumberOfPipelines>
      supported_dai_formats_;
  std::array<std::optional<fuchsia_hardware_audio::DaiFormat>, kNumberOfPipelines>
      current_dai_formats_;

  fidl::WireSyncClient<fuchsia_hardware_clock::Clock> clock_gate_;
  fidl::WireSyncClient<fuchsia_hardware_clock::Clock> pll_;
  std::vector<SclkPin> sclk_clients_;
  bool soc_power_started_ = false;
  zx::time last_started_time_;
  zx::time last_stopped_time_;
  trace_async_id_t trace_async_id_;

  // Inspect-related
  std::unique_ptr<Recorder> recorder_;
};

// This can't be defined until all the types are complete, so it has to be
// outside the Engine class definition.
inline Engine::Engine()
    : ring_buffer_format(
          fuchsia_hardware_audio::Format2::WithPcmFormat(fuchsia_hardware_audio::PcmFormat{})) {}

}  // namespace audio::aml_g12

#endif  // SRC_MEDIA_AUDIO_DRIVERS_AML_G12_TDM_COMPOSITE_SERVER_H_
