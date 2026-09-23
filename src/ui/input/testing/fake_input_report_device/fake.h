// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_INPUT_TESTING_FAKE_INPUT_REPORT_DEVICE_FAKE_H_
#define SRC_UI_INPUT_TESTING_FAKE_INPUT_REPORT_DEVICE_FAKE_H_

#include <fuchsia/input/report/cpp/fidl.h>
#include <lib/fidl/cpp/binding_set.h>

#include <fbl/mutex.h>

#include "reports_reader.h"

namespace fake_input_report_device {

// Creates a fake device that vends fuchsia.input.report FIDL.
//
// Calling `SetReport` and `SetDescriptor` will change the behavior of
// the device when the client goes to read the report or the descriptor.
class FakeInputDevice final : public fuchsia::input::report::InputDevice {
 public:
  explicit FakeInputDevice(fidl::InterfaceRequest<fuchsia::input::report::InputDevice> request,
                           async_dispatcher_t* dispatcher)
      : binding_(this, std::move(request), dispatcher) {}

  // Sets the fake's report, which will be read with |ReadInputReports| and
  // |GetInputReport|. This also causes any InputReportReaders to receive data
  // they are waiting for.
  void SetReports(std::vector<fuchsia::input::report::InputReport> reports);

  // Sets the fake's report, which will be read with |GetFeatureReport|.
  void SetReports(std::vector<fuchsia::input::report::FeatureReport> reports);

  // Sets the fake's descriptor, which will be read with |GetDescriptor|.
  void SetDescriptor(fuchsia::input::report::DeviceDescriptorPtr descriptor);

  // The overriden FIDL function calls.
  void GetInputReportsReader(
      fidl::InterfaceRequest<fuchsia::input::report::InputReportsReader> reader) override;
  void GetInputReportsReaderV2(
      ::fidl::InterfaceRequest<::fuchsia::input::report::InputReportsReaderV2> reader,
      uint16_t max_unacknowledged_reports_limit, GetInputReportsReaderV2Callback callback) override;
  void GetDescriptor(GetDescriptorCallback callback) override;
  void SendOutputReport(fuchsia::input::report::OutputReport report,
                        SendOutputReportCallback callback) override;
  void GetFeatureReport(GetFeatureReportCallback callback) override;
  void SetFeatureReport(::fuchsia::input::report::FeatureReport report,
                        SetFeatureReportCallback callback) override {
    callback(
        fuchsia::input::report::InputDevice_SetFeatureReport_Result::WithErr(ZX_ERR_NOT_SUPPORTED));
  }
  void GetInputReport(::fuchsia::input::report::DeviceType device_type,
                      GetInputReportCallback callback) override;
  void handle_unknown_method(uint64_t ordinal, bool method_has_response) override {}

 private:
  fidl::Binding<fuchsia::input::report::InputDevice> binding_;

  // This lock makes the class thread-safe, which is important because setting the
  // reports and handling the FIDL calls can happen on seperate threads.
  fbl::Mutex lock_;

  std::vector<fuchsia::input::report::InputReport> reports_ __TA_GUARDED(lock_);
  std::vector<fuchsia::input::report::FeatureReport> feature_reports_ __TA_GUARDED(lock_);
  fuchsia::input::report::DeviceDescriptorPtr descriptor_ __TA_GUARDED(lock_);
  std::optional<FakeInputReportsReaderV2> reader_v2_ __TA_GUARDED(lock_);
};

}  // namespace fake_input_report_device

#endif  // SRC_UI_INPUT_TESTING_FAKE_INPUT_REPORT_DEVICE_FAKE_H_
