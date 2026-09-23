// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "input-reports-reader-v2.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/trace/event.h>

namespace hid_input_report_dev {

namespace {

constexpr size_t kPerLine = 16;

void hexdump(const cpp20::span<const uint8_t> data) {
  constexpr uint32_t kCharsPerByte = 3;
  for (size_t i = 0; i < data.size_bytes(); i += kPerLine) {
    const size_t line_size = std::min(kPerLine, data.size_bytes() - i);
    char line[kCharsPerByte * kPerLine + 1] = {};
    for (size_t j = 0; j < line_size; j++) {
      snprintf(&line[kCharsPerByte * j], 4, "%02X ", data[i + j]);
    }

    fdf::info("hid-dump({}): {}", i, static_cast<const char*>(line));
  }
}

}  // namespace

void InputReportsReaderV2::AcknowledgeReports(AcknowledgeReportsRequestView request,
                                              AcknowledgeReportsCompleter::Sync& completer) {
  while (!unacknowledged_batches_.empty() &&
         unacknowledged_batches_.front() <= request->last_acknowledged_report_stamp) {
    unacknowledged_batches_.pop_front();
  }
  SendReports();
}

void InputReportsReaderV2::SendReports() {
  while (!reports_data_.empty() && unacknowledged_batches_.size() < max_unacknowledged_reports_) {
    std::array<fuchsia_input_report::wire::InputReport,
               fuchsia_input_report::wire::kMaxDeviceReportCount>
        reports;
    size_t num_reports = 0;
    ReportStamp last_stamp = 0;

    TRACE_DURATION("input", "InputReportReaderV2 SendReports", "instance_id", reader_id_);
    while (!reports_data_.empty() && num_reports < reports.size()) {
      RawReport& item = reports_data_.front();

      // Defer parsing raw HID reports and building FIDL tables on report_allocator_
      // until constructing the batch to send. This ensures that report_allocator_ is
      // only used for transient allocations during SendReports(), allowing it to be
      // reset safely and avoiding unbounded arena growth under client backpressure.
      fuchsia_input_report::wire::InputReport report(report_allocator_);
      const hid_input_report::ParseResult result =
          item.device->ParseInputReport(item.data.data(), item.size, report_allocator_, report);
      if (result != hid_input_report::ParseResult::kOk) {
        fdf::error("SendReports: Device failed to parse report correctly {} ({})",
                   ParseResultGetString(result), static_cast<int>(result));
        hexdump(cpp20::span<const uint8_t>(item.data.data(), item.size));
        reports_data_.pop();
        continue;
      }

      report.set_report_id(*item.device->InputReportId());
      report.set_event_time(report_allocator_, item.report_time.get());
      report.set_trace_id(report_allocator_, TRACE_NONCE());

      TRACE_FLOW_BEGIN("input", "input_report", report.trace_id());

      last_stamp = item.stamp;
      reports[num_reports++] = report;
      reports_data_.pop();
    }

    if (num_reports > 0) {
      auto reports_view = fidl::VectorView<fuchsia_input_report::wire::InputReport>::FromExternal(
          reports.data(), num_reports);

      fidl::Status status = fidl::WireSendEvent(binding_)->OnInputReports(reports_view, last_stamp);
      if (!status.ok()) {
        fdf::error("SendReports: Failed to send reports: {}", status.FormatDescription());
        report_allocator_.Reset();
        break;
      }
      unacknowledged_batches_.push_back(last_stamp);
    }
    report_allocator_.Reset();
  }
}

void InputReportsReaderV2::ReceiveReport(cpp20::span<const uint8_t> raw_report,
                                         zx::time report_time, hid_input_report::Device* device) {
  if (!device->InputReportId().has_value()) {
    fdf::error("ReceiveReport: Device cannot receive input reports");
    return;
  }

  if (raw_report.size() > fuchsia_hardware_hidbus::wire::kMaxReportLen) {
    fdf::error("ReceiveReport: Report size {} exceeds max {}", raw_report.size(),
               fuchsia_hardware_hidbus::wire::kMaxReportLen);
    return;
  }

  if (reports_data_.full()) {
    fdf::warn("ReceiveReport: Ring buffer full, dropping oldest report");
    TRACE_INSTANT("input", "InputReportDrop", TRACE_SCOPE_PROCESS);
    reports_data_.pop();
  }

  // Store raw report directly in the ring buffer to avoid large stack allocations.
  reports_data_.emplace();
  RawReport& item = reports_data_.back();
  item.size = raw_report.size();
  item.report_time = report_time;
  item.device = device;
  item.stamp = next_report_stamp_++;
  std::copy(raw_report.begin(), raw_report.end(), item.data.begin());

  SendReports();
}

}  // namespace hid_input_report_dev
