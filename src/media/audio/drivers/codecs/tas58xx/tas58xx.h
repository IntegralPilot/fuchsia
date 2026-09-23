// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_CODECS_TAS58XX_TAS58XX_H_
#define SRC_MEDIA_AUDIO_DRIVERS_CODECS_TAS58XX_TAS58XX_H_

#include <fidl/fuchsia.hardware.gpio/cpp/wire.h>
#include <fidl/fuchsia.hardware.ti.metadata/cpp/fidl.h>
#include <lib/async/cpp/irq.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/simple-codec/simple-codec-server.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <threads.h>

#include <ddktl/device.h>
#include <ddktl/suspend-txn.h>
#include <fbl/mutex.h>

#include "src/devices/i2c/lib/i2c-channel-legacy/i2c-channel.h"
#include "tas58xx-inspect.h"

namespace audio {

class Tas58xx : public SimpleCodecServer,
                public fuchsia::hardware::audio::signalprocessing::SignalProcessing {
 public:
  static zx_status_t Create(zx_device_t* parent);

  explicit Tas58xx(zx_device_t* device, ddk::I2cChannel i2c,
                   fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> gpio_fault);

  // Implementation for SimpleCodecServer.
  zx_status_t Shutdown() override;

 protected:
  // Implementation for SimpleCodecServer.
  zx::result<DriverIds> Initialize() override;
  zx_status_t Reset() override;
  Info GetInfo() override;
  zx_status_t Stop() override;
  zx_status_t Start() override;
  DaiSupportedFormats GetDaiFormats() override;
  zx::result<CodecFormatInfo> SetDaiFormat(const DaiFormat& format) override;
  GainFormat GetGainFormat() override;
  GainState GetGainState() override;
  void SetGainState(GainState state) override;
  void GetElements(fuchsia::hardware::audio::signalprocessing::SignalProcessing::GetElementsCallback
                       callback) override;
  void SetElementState(
      fuchsia::hardware::audio::signalprocessing::ElementId processing_element_id,
      fuchsia::hardware::audio::signalprocessing::SettableElementState state,
      fuchsia::hardware::audio::signalprocessing::SignalProcessing::SetElementStateCallback
          callback) override;
  void WatchElementState(
      fuchsia::hardware::audio::signalprocessing::ElementId processing_element_id,
      fuchsia::hardware::audio::signalprocessing::SignalProcessing::WatchElementStateCallback
          callback) override;
  void GetTopologies(
      fuchsia::hardware::audio::signalprocessing::SignalProcessing::GetTopologiesCallback callback)
      override;
  void WatchTopology(
      fuchsia::hardware::audio::signalprocessing::SignalProcessing::WatchTopologyCallback callback)
      override;
  void SetTopology(fuchsia::hardware::audio::signalprocessing::TopologyId topology_id,
                   fuchsia::hardware::audio::signalprocessing::SignalProcessing::SetTopologyCallback
                       callback) override;
  void SignalProcessingConnect(
      fidl::InterfaceRequest<fuchsia::hardware::audio::signalprocessing::SignalProcessing>
          signal_processing) override;
  bool SupportsSignalProcessing() override { return true; }
  void handle_unknown_method(uint64_t ordinal, bool method_has_response) override;

  // Protected for unit tests.
  zx_status_t SetBand(bool enabled, size_t index, uint32_t frequency, float Q, float gain_db);
  static fuchsia_hardware_audio::wire::TopologyId GetTopologyId() { return kTopologyId; }
  static fuchsia_hardware_audio::wire::ElementId GetAglPeId() { return kAglPeId; }
  static fuchsia_hardware_audio::wire::ElementId GetEqPeId() { return kEqPeId; }
  static fuchsia_hardware_audio::wire::ElementId GetGainPeId() { return kGainPeId; }
  static fuchsia_hardware_audio::wire::ElementId GetMutePeId() { return kMutePeId; }
  virtual bool BackgroundFaultPollingIsEnabled() {
    return true;  // Unit test can override to disable.
  }
  void PeriodicPollFaults();  // Unit test can invoke this directly.

 private:
  static constexpr float kMaxGain = 24.0;
  static constexpr float kMinGain = -103.0;
  static constexpr float kGainStep = 0.5;
  static constexpr fuchsia_hardware_audio::wire::ElementId kAglPeId = 1;
  static constexpr fuchsia_hardware_audio::wire::ElementId kEqPeId = 2;
  static constexpr fuchsia_hardware_audio::wire::ElementId kGainPeId = 3;
  static constexpr fuchsia_hardware_audio::wire::ElementId kMutePeId = 4;
  static constexpr fuchsia_hardware_audio::wire::TopologyId kTopologyId = 1;
  static constexpr size_t kEqualizerNumberOfBands = 5;
  static constexpr uint32_t kEqualizerMinFrequency = 100;
  static constexpr uint32_t kEqualizerMaxFrequency = 20'000;
  static constexpr float kEqualizerMinGainDb = -5.f;
  static constexpr float kEqualizerMaxGainDb = 5.f;
  static constexpr float kSupportedQ = 1.f;
  static constexpr size_t kMaximumNumberOfSignalProcessingConnections = 8;

  zx_status_t WriteReg(uint8_t reg, uint8_t value);
  zx_status_t WriteRegs(uint8_t* regs, size_t count);
  zx_status_t ReadReg(uint8_t reg, uint8_t* value);
  zx_status_t UpdateReg(uint8_t reg, uint8_t mask, uint8_t value);
  zx_status_t SetGain(float gain);
  zx_status_t SetMute(bool mute);
  zx_status_t SetEqualizerElement(
      fuchsia::hardware::audio::signalprocessing::SettableElementState state);
  zx_status_t SetAutomaticGainLimiterElement(
      fuchsia::hardware::audio::signalprocessing::SettableElementState state);
  zx_status_t SetGainElement(
      fuchsia::hardware::audio::signalprocessing::SettableElementState state);
  zx_status_t SetMuteElement(
      fuchsia::hardware::audio::signalprocessing::SettableElementState state);
  void SendEqualizerWatchReply(
      fuchsia::hardware::audio::signalprocessing::SignalProcessing::WatchElementStateCallback
          callback);
  void SendGainWatchReply(
      fuchsia::hardware::audio::signalprocessing::SignalProcessing::WatchElementStateCallback
          callback);
  void SendMuteWatchReply(
      fuchsia::hardware::audio::signalprocessing::SignalProcessing::WatchElementStateCallback
          callback);

  ddk::I2cChannel i2c_;
  GainState gain_state_ = {};
  fuchsia_hardware_ti_metadata::TasMetadata metadata_;
  bool started_ = false;
  uint32_t number_of_channels_ = 2;
  uint32_t rate_ = 48'000;

  // How often to poll for codec faults.  Would be nice to use IRQ for this,
  // but we've exhausted the IRQ supply, so 20 seconds gives a balance between
  // keeping overhead low but polling often enough to catch faults on a
  // relatively timely basis.
  static constexpr zx::duration poll_interval_ = zx::sec(20);
  void ScheduleFaultPolling();
  fidl::WireSyncClient<fuchsia_hardware_gpio::Gpio> fault_gpio_;
  struct {
    bool i2c_error;         // True if saw an I2C error while reading fault
    uint8_t chan_fault;     // Channel fault bits
    uint8_t global_fault1;  // Global fault bits
    uint8_t global_fault2;  // More global fault bits
    uint8_t ot_warning;     // Over-temperature warning bit
  } fault_info_;
  Tas58xxInspect inspect_reporter_;

  // AGL.
  bool last_agl_ = false;
  std::optional<bool> last_reported_agl_;
  fidl::BindingSet<fuchsia::hardware::audio::signalprocessing::SignalProcessing>
      signal_processing_bindings_;
  std::optional<
      fuchsia::hardware::audio::signalprocessing::SignalProcessing::WatchElementStateCallback>
      agl_callback_;

  // Equalizer.
  uint32_t frequencies_[kEqualizerNumberOfBands] = {100, 250, 1'000, 4'000, 16'000};
  float gains_[kEqualizerNumberOfBands] = {0.f, 0.f, 0.f, 0.f, 0.f};
  bool band_enabled_[kEqualizerNumberOfBands] = {false, false, false, false, false};
  bool equalizer_enabled_ = true;
  std::optional<
      fuchsia::hardware::audio::signalprocessing::SignalProcessing::WatchElementStateCallback>
      equalizer_callback_;
  bool last_equalizer_update_reported_ = false;

  // Gain.
  bool gain_enabled_ = true;
  std::optional<
      fuchsia::hardware::audio::signalprocessing::SignalProcessing::WatchElementStateCallback>
      gain_callback_;
  bool last_gain_update_reported_ = false;

  // Mute.
  std::optional<
      fuchsia::hardware::audio::signalprocessing::SignalProcessing::WatchElementStateCallback>
      mute_callback_;
  bool last_mute_update_reported_ = false;

  bool responded_to_watch_topology_ = false;
  std::optional<fuchsia::hardware::audio::signalprocessing::Reader::WatchTopologyCallback>
      topology_callback_;
};
}  // namespace audio

#endif  // SRC_MEDIA_AUDIO_DRIVERS_CODECS_TAS58XX_TAS58XX_H_
