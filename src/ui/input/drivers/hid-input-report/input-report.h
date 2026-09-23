// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_INPUT_DRIVERS_HID_INPUT_REPORT_INPUT_REPORT_H_
#define SRC_UI_INPUT_DRIVERS_HID_INPUT_REPORT_INPUT_REPORT_H_

#include <fidl/fuchsia.hardware.input/cpp/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/power/cpp/wake-lease.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>

#include <list>
#include <vector>

#include <fbl/intrusive_double_list.h>

#include "input-reports-reader-v2.h"
#include "input-reports-reader.h"
#include "src/ui/input/lib/hid-input-report/device.h"

namespace hid_input_report_dev {

class InputReport : public fidl::WireServer<fuchsia_input_report::InputDevice>,
                    public InputReportBase {
 public:
  explicit InputReport(fidl::ClientEnd<fuchsia_hardware_input::Device> input_device)
      : input_device_(std::move(input_device)),
        wake_lease_(fdf::Dispatcher::GetCurrent()->async_dispatcher(), "hid-input-report", {}) {}

  zx_status_t Start();

  // InputReportBase functions.
  void RemoveReaderFromList(InputReportsReaderV2* reader) override;

  // Max unacknowledged report count allowed for 1/2 second.
  // Generic HID input devices are event/interrupt-driven.
  // Assuming a standard max report rate of 120 Hz yields 60 reports per 1/2 second (120 Hz / 2).
  static constexpr uint16_t kHalfSecondReportCount = 60;

  // FIDL functions.
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

  // For testing.
  sync_completion_t& next_reader_wait() { return next_reader_wait_; }

  zx::vmo InspectVmo() { return inspector_.DuplicateVmo(); }
  inspect::Inspector& Inspector() { return inspector_; }

 private:
  // This is the static size that is used to allocate this instance's InputDescriptor.
  // This amount of memory is stack allocated when a client calls GetDescriptor.
  static constexpr size_t kFidlDescriptorBufferSize = 8192;

  static constexpr uint64_t kLatencyBucketCount = 7;
  static constexpr zx::duration kLatencyFloor = zx::msec(5);
  static constexpr zx::duration kLatencyInitialStep = kLatencyFloor;
  static constexpr uint64_t kLatencyStepMultiplier = 3;

  bool ParseHidInputReportDescriptor(const hid::ReportDescriptor* report_desc);

  // If we have a consumer control device, get a report and send it to the reader,
  // since the reader needs the device's state.
  void SendInitialConsumerControlReport(InputReportsReaderV2* reader);

  std::string GetDeviceTypesString() const;

  void HandleReports(
      fidl::WireUnownedResult<fuchsia_hardware_input::DeviceReportsReader::ReadReports>& result);
  void HandleReport(cpp20::span<const uint8_t> report, zx::time report_time);

  fidl::WireSyncClient<fuchsia_hardware_input::Device> input_device_;
  fidl::WireClient<fuchsia_hardware_input::DeviceReportsReader> dev_reader_;

  fdf_power::TimeoutWakeLease wake_lease_;

  std::vector<std::unique_ptr<hid_input_report::Device>> devices_;

  uint32_t next_reader_id_ = 0;
  std::list<std::unique_ptr<InputReportsReaderV2>> readers_v2_list_;
  sync_completion_t next_reader_wait_;

  inspect::Inspector inspector_;
  inspect::Node root_;
  inspect::ExponentialUintHistogram latency_histogram_usecs_;
  inspect::UintProperty average_latency_usecs_;
  inspect::UintProperty max_latency_usecs_;
  inspect::StringProperty device_types_;
  inspect::UintProperty total_report_count_;
  inspect::UintProperty last_event_timestamp_;

  uint64_t report_count_ = 0;
  zx::duration total_latency_ = {};
  zx::duration max_latency_ = {};

  uint32_t sensor_count_ = 0;
};

}  // namespace hid_input_report_dev

#endif  // SRC_UI_INPUT_DRIVERS_HID_INPUT_REPORT_INPUT_REPORT_H_
