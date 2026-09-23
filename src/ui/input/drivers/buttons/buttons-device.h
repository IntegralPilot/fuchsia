// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_INPUT_DRIVERS_BUTTONS_BUTTONS_DEVICE_H_
#define SRC_UI_INPUT_DRIVERS_BUTTONS_BUTTONS_DEVICE_H_

#include <fidl/fuchsia.buttons/cpp/fidl.h>
#include <fidl/fuchsia.hardware.gpio/cpp/wire.h>
#include <fidl/fuchsia.input.report/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/power/cpp/wake-lease.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/port.h>
#include <lib/zx/timer.h>
#include <zircon/syscalls-next.h>
#include <zircon/threads.h>

#include <fbl/array.h>

#include "src/ui/input/lib/input-report-reader/reader.h"

namespace buttons {

// zx_port_packet::key.
constexpr uint64_t kPortKeyShutDown = 0x01;
// Start of up to kNumberOfRequiredGpios port types used for interrupts.
constexpr uint64_t kPortKeyInterruptStart = 0x10;
// Timer start
constexpr uint64_t kPortKeyTimerStart = 0x100;
// Poll timer
constexpr uint64_t kPortKeyPollTimer = 0x1000;
// Debounce threshold.
constexpr uint64_t kDebounceThresholdNs = 50'000'000;
// Max unacknowledged report count allowed for 1/2 second based on 50 ms debounce threshold (500 ms
// / 50 ms = 10 reports per 1/2 second).
constexpr uint16_t kMaxReportsPerHalfSecond =
    static_cast<uint16_t>(500'000'000 / kDebounceThresholdNs);

class ButtonsDevice : public fidl::WireServer<fuchsia_input_report::InputDevice> {
 public:
  struct Gpio {
    fidl::WireSyncClient<fuchsia_hardware_gpio::Gpio> client;
    zx::interrupt irq;
    fuchsia_buttons::GpioConfig config;
  };

  explicit ButtonsDevice(async_dispatcher_t* dispatcher,
                         std::vector<fuchsia_buttons::GpioButtonConfig> buttons,
                         std::vector<Gpio> gpios,
                         fidl::ClientEnd<fuchsia_power_system::ActivityGovernor> sag_client);
  void Notify(size_t button_index);
  void ShutDown();

  // fuchsia_input_report::InputDevice required methods
  void GetInputReportsReader(GetInputReportsReaderRequestView request,
                             GetInputReportsReaderCompleter::Sync& completer) override;
  void GetInputReportsReaderV2(GetInputReportsReaderV2RequestView request,
                               GetInputReportsReaderV2Completer::Sync& completer) override;
  void GetDescriptor(GetDescriptorCompleter::Sync& completer) override;
  void SendOutputReport(SendOutputReportRequestView request,
                        SendOutputReportCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void GetFeatureReport(GetFeatureReportCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void SetFeatureReport(SetFeatureReportRequestView request,
                        SetFeatureReportCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void GetInputReport(GetInputReportRequestView request,
                      GetInputReportCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_input_report::InputDevice> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    fdf::warn("Unexpected fidl method invoked: {}", metadata.method_ordinal);
  }

 private:
  friend class ButtonsDeviceTest;
  static constexpr size_t kFeatureAndDescriptorBufferSize = 512;

  struct ButtonsInputReport {
    zx::time event_time = zx::time(ZX_TIME_INFINITE_PAST);
    std::array<bool, fuchsia_buttons::kMaxGpioButtonIdOrd> buttons = {};

    void ToFidlInputReport(
        fidl::WireTableBuilder<::fuchsia_input_report::wire::InputReport>& input_report,
        fidl::AnyArena& allocator) const;

    bool operator==(const ButtonsInputReport& other) const { return buttons == other.buttons; }
    bool operator!=(const ButtonsInputReport& other) const { return !(*this == other); }

    void set(uint32_t button_id, bool pressed) {
      if (button_id >= buttons.size()) {
        return;
      }
      buttons[button_id] = pressed;
    }
    bool empty() const {
      return std::all_of(buttons.cbegin(), buttons.cend(), [](bool i) { return !i; });
    }
  };

  int Thread();
  zx_status_t Init();
  zx::result<bool> ReconfigurePolarity(size_t idx, uint64_t int_port);
  zx_status_t ConfigureInterrupt(size_t idx, uint64_t int_port);
  zx::result<bool> MatrixScan(uint32_t row, uint32_t col, zx_duration_t delay);
  zx::result<ButtonsInputReport> GetInputReportInternal();

  zx::port port_;
  async_dispatcher_t* dispatcher_;

  thrd_t thread_;
  libsync::Completion thread_started_;
  input_report_reader::InputReportReaderManager<ButtonsInputReport> readers_;
  std::vector<fuchsia_buttons::GpioButtonConfig> buttons_;
  std::vector<Gpio> gpios_;

  struct debounce_state {
    bool enqueued;
    zx::timer timer;
    bool value;
    zx::time timestamp = zx::time::infinite_past();
  };
  fbl::Array<debounce_state> debounce_states_;
  // last_report_ saved to de-duplicate reports
  std::optional<ButtonsInputReport> last_report_ = std::nullopt;

  zx::duration poll_period_{zx::duration::infinite()};
  zx::timer poll_timer_;

  inspect::Inspector inspector_;
  inspect::Node metrics_root_;
  // Note that because this driver handles both polling and IRQ reports, latency is only measured
  // for IRQ reports because it is not meaningful for polling.
  inspect::UintProperty average_latency_usecs_;
  inspect::UintProperty max_latency_usecs_;
  // However, total_report_count_ and last_event_timestamp_ will reflect both polling and IRQ
  // reports.
  inspect::UintProperty total_report_count_;
  inspect::UintProperty last_event_timestamp_;

  uint64_t report_count_ = 0;
  zx::duration total_latency_ = {};
  zx::duration max_latency_ = {};

  fdf_power::TimeoutWakeLease wake_lease_;
};

}  // namespace buttons

#endif  // SRC_UI_INPUT_DRIVERS_BUTTONS_BUTTONS_DEVICE_H_
