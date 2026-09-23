// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/input/testing/fake_input_report_device/fake.h"

#include <zircon/assert.h>

#include <fbl/auto_lock.h>

#include "lib/fidl/cpp/clone.h"

namespace fake_input_report_device {

void FakeInputDevice::SetDescriptor(fuchsia::input::report::DeviceDescriptorPtr descriptor) {
  fbl::AutoLock lock(&lock_);
  descriptor_ = std::move(descriptor);
}

void FakeInputDevice::GetDescriptor(GetDescriptorCallback callback) {
  fbl::AutoLock lock(&lock_);
  fuchsia::input::report::DeviceDescriptor desc;
  fidl::Clone(*descriptor_, &desc);
  callback(std::move(desc));
}

void FakeInputDevice::GetInputReportsReader(
    fidl::InterfaceRequest<fuchsia::input::report::InputReportsReader> reader) {
  reader.Close(ZX_ERR_NOT_SUPPORTED);
}

void FakeInputDevice::GetInputReportsReaderV2(
    ::fidl::InterfaceRequest<::fuchsia::input::report::InputReportsReaderV2> reader,
    uint16_t max_unacknowledged_reports_limit, GetInputReportsReaderV2Callback callback) {
  fbl::AutoLock lock(&lock_);
  if (reader_v2_) {
    reader.Close(ZX_ERR_ALREADY_BOUND);
    callback(0);
    return;
  }
  uint16_t max_unacknowledged_reports =
      max_unacknowledged_reports_limit == 0 ? 1 : max_unacknowledged_reports_limit;
  reader_v2_.emplace(std::move(reader), binding_.dispatcher(), max_unacknowledged_reports);
  if (!reports_.empty()) {
    std::vector<fuchsia::input::report::InputReport> reports_to_send;
    fidl::Clone(reports_, &reports_to_send);
    reader_v2_->SendReports(std::move(reports_to_send));
  }
  callback(max_unacknowledged_reports);
}

void FakeInputDevice::SendOutputReport(fuchsia::input::report::OutputReport report,
                                       SendOutputReportCallback callback) {
  callback(
      fuchsia::input::report::InputDevice_SendOutputReport_Result::WithErr(ZX_ERR_NOT_SUPPORTED));
}

void FakeInputDevice::GetInputReport(::fuchsia::input::report::DeviceType device_type,
                                     GetInputReportCallback callback) {
  ZX_DEBUG_ASSERT(false);
  callback(
      fuchsia::input::report::InputDevice_GetInputReport_Result::WithErr(ZX_ERR_NOT_SUPPORTED));
}

void FakeInputDevice::GetFeatureReport(GetFeatureReportCallback callback) {
  fbl::AutoLock lock(&lock_);
  if (feature_reports_.empty()) {
    callback(
        fuchsia::input::report::InputDevice_GetFeatureReport_Result::WithErr(ZX_ERR_NOT_SUPPORTED));
  } else {
    fuchsia::input::report::InputDevice_GetFeatureReport_Response response(
        std::move(feature_reports_[0]));
    callback(fuchsia::input::report::InputDevice_GetFeatureReport_Result::WithResponse(
        std::move(response)));
    feature_reports_.erase(feature_reports_.begin());
  }
}

void FakeInputDevice::SetReports(std::vector<fuchsia::input::report::InputReport> reports) {
  fbl::AutoLock lock(&lock_);
  reports_ = std::move(reports);
  if (reader_v2_) {
    std::vector<fuchsia::input::report::InputReport> reports_to_send;
    fidl::Clone(reports_, &reports_to_send);
    reader_v2_->SendReports(std::move(reports_to_send));
  }
}

void FakeInputDevice::SetReports(std::vector<fuchsia::input::report::FeatureReport> reports) {
  fbl::AutoLock lock(&lock_);
  feature_reports_ = std::move(reports);
}

}  // namespace fake_input_report_device
