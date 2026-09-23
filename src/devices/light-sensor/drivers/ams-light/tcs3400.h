// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_LIGHT_SENSOR_DRIVERS_AMS_LIGHT_TCS3400_H_
#define SRC_DEVICES_LIGHT_SENSOR_DRIVERS_AMS_LIGHT_TCS3400_H_

#include <fidl/fuchsia.hardware.gpio/cpp/wire.h>
#include <fidl/fuchsia.input.report/cpp/wire.h>
#include <lib/async/cpp/irq.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/inspect/cpp/bounded_list_node.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/result.h>
#include <time.h>

#include <memory>
#include <optional>

#include <fbl/mutex.h>

#include "src/devices/i2c/lib/i2c-channel/i2c-channel.h"
#include "src/ui/input/lib/input-report-reader/reader.h"

namespace tcs {

struct Tcs3400InputReport {
  zx::time event_time = zx::time(ZX_TIME_INFINITE_PAST);
  int64_t illuminance;
  int64_t red;
  int64_t blue;
  int64_t green;

  void ToFidlInputReport(
      fidl::WireTableBuilder<::fuchsia_input_report::wire::InputReport>& input_report,
      fidl::AnyArena& allocator) const;

  bool is_valid() const { return event_time.get() != ZX_TIME_INFINITE_PAST; }
};

struct Tcs3400FeatureReport {
  zx::time event_time = zx::time(ZX_TIME_INFINITE_PAST);
  int64_t report_interval_us = 0;
  fuchsia_input_report::wire::SensorReportingState reporting_state;
  int64_t sensitivity = 0;
  int64_t threshold_high = 0;
  int64_t threshold_low = 0;
  int64_t integration_time_us = 0;

  fuchsia_input_report::wire::FeatureReport ToFidlFeatureReport(fidl::AnyArena& allocator) const;
};

struct InspectTcs3400FeatureReport {
  inspect::Node node;
  inspect::UintProperty event_time;
  inspect::UintProperty report_interval_us;
  inspect::StringProperty reporting_state;
  inspect::UintProperty sensitivity;
  inspect::UintProperty threshold_high;
  inspect::UintProperty threshold_low;
  inspect::UintProperty integration_time_us;

  explicit InspectTcs3400FeatureReport(inspect::Node n, const Tcs3400FeatureReport& report);
};

class Tcs3400 : public fdf::DriverBase2,
                public fidl::WireServer<fuchsia_input_report::InputDevice> {
 public:
  static constexpr std::string_view kDriverName = "tcs3400_light";
  static constexpr std::string_view kChildNodeName = "tcs-3400";

  Tcs3400() : fdf::DriverBase2(kDriverName) {}

  // fdf::DriverBase2 implementation.
  zx::result<> Start(fdf::DriverContext context) override;

  zx::result<> InitMetadata(fdf::Namespace& incoming);

  // fidl::WireServer<fuchsia_input_report::InputDevice> implementation.
  void GetInputReportsReader(GetInputReportsReaderRequestView request,
                             GetInputReportsReaderCompleter::Sync& completer) override;
  void GetInputReportsReaderV2(GetInputReportsReaderV2RequestView request,
                               GetInputReportsReaderV2Completer::Sync& completer) override;

  void GetDescriptor(GetDescriptorCompleter::Sync& completer) override;
  void SendOutputReport(SendOutputReportRequestView request,
                        SendOutputReportCompleter::Sync& completer) override;
  void GetFeatureReport(GetFeatureReportCompleter::Sync& completer) override;
  void SetFeatureReport(SetFeatureReportRequestView request,
                        SetFeatureReportCompleter::Sync& completer) override;
  void GetInputReport(GetInputReportRequestView request,
                      GetInputReportCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_input_report::InputDevice> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    fdf::warn("Unexpected fidl method invoked: {}", metadata.method_ordinal);
  }

 protected:
  // Used by tests.
  virtual void OnNextReader() {}

  std::optional<inspect::ComponentInspector> component_inspector_;
  std::optional<inspect::BoundedListNode> inspect_reports_;

 private:
  static constexpr size_t kMaxFeatureReports = 10;
  static constexpr size_t kFeatureAndDescriptorBufferSize = 512;

  // Max unacknowledged report count allowed for 1/2 second.
  // TCS3400 integration time step size is 2.78 ms (2780 µs), giving a max sampling rate of ~360 Hz.
  // 360 Hz / 2 = 180 reports per 1/2 second.
  static constexpr uint16_t kMaxReportsPerHalfSecond = 180;

  void DevfsConnect(fidl::ServerEnd<fuchsia_input_report::InputDevice> request);

  void HandlePoll();
  void HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq, zx_status_t status,
                 const zx_packet_interrupt_t* interrupt);
  void RearmIrq();
  void Configure();

  async::IrqMethod<Tcs3400, &Tcs3400::HandleIrq> irq_handler_{this};
  async::TaskClosureMethod<Tcs3400, &Tcs3400::HandlePoll> polling_handler_{this};
  async::TaskClosureMethod<Tcs3400, &Tcs3400::RearmIrq> rearm_irq_handler_{this};

  i2c::I2cChannel i2c_;
  fidl::WireSyncClient<fuchsia_hardware_gpio::Gpio> gpio_;
  zx::interrupt irq_;
  fbl::Mutex input_lock_;
  fbl::Mutex feature_lock_;
  Tcs3400InputReport input_rpt_ TA_GUARDED(input_lock_) = {};
  Tcs3400FeatureReport feature_rpt_ TA_GUARDED(feature_lock_) = {};
  uint8_t atime_ = 1;
  uint8_t again_ = 1;
  bool isSaturated_ = false;
  zx::time lastSaturatedLog_ = zx::time::infinite_past();
  input_report_reader::InputReportReaderManager<Tcs3400InputReport,
                                                fuchsia_input_report::wire::kMaxDeviceReportCount>
      readers_;

  zx::result<Tcs3400InputReport> ReadInputRpt();
  zx_status_t InitGain(uint8_t gain);
  zx::result<> WriteReg(uint8_t reg, uint8_t value);
  zx::result<uint8_t> ReadReg(uint8_t reg);

  driver_devfs::Connector<fuchsia_input_report::InputDevice> devfs_connector_{
      fit::bind_member<&Tcs3400::DevfsConnect>(this)};
  fidl::ServerBindingGroup<fuchsia_input_report::InputDevice> bindings_;
  fdf::OwnedChildNode child_;
};
}  // namespace tcs

#endif  // SRC_DEVICES_LIGHT_SENSOR_DRIVERS_AMS_LIGHT_TCS3400_H_
