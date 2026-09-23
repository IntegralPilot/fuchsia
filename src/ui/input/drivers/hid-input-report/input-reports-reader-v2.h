// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_INPUT_DRIVERS_HID_INPUT_REPORT_INPUT_REPORTS_READER_V2_H_
#define SRC_UI_INPUT_DRIVERS_HID_INPUT_REPORT_INPUT_REPORTS_READER_V2_H_

#include <fidl/fuchsia.hardware.hidbus/cpp/wire.h>
#include <fidl/fuchsia.input.report/cpp/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <zircon/assert.h>

#include <array>
#include <deque>

#include <fbl/ring_buffer.h>

#include "src/ui/input/drivers/hid-input-report/input-reports-reader.h"
#include "src/ui/input/lib/hid-input-report/device.h"

namespace hid_input_report_dev {

class InputReportsReaderV2 : public fidl::WireServer<fuchsia_input_report::InputReportsReaderV2> {
 public:
  using ReportStamp = uint64_t;

  // `base` must outlive the InputReportsReaderV2 instance.
  // InputReportsReaderV2 will be freed by InputReportBase on unbind.
  explicit InputReportsReaderV2(InputReportBase* base, uint32_t reader_id,
                                fidl::ServerEnd<fuchsia_input_report::InputReportsReaderV2> server,
                                uint16_t max_unacknowledged_reports)
      : reader_id_(reader_id),
        max_unacknowledged_reports_(max_unacknowledged_reports),
        base_(base),
        binding_(fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(server), this,
                 [this](fidl::UnbindInfo) { base_->RemoveReaderFromList(this); }) {
    ZX_ASSERT(max_unacknowledged_reports >= 1);
  }
  ~InputReportsReaderV2() override { binding_.Close(ZX_ERR_PEER_CLOSED); }

  void ReceiveReport(cpp20::span<const uint8_t> raw_report, zx::time report_time,
                     hid_input_report::Device* device);

  // fidl::WireServer<fuchsia_input_report::InputReportsReaderV2>:
  void AcknowledgeReports(AcknowledgeReportsRequestView request,
                          AcknowledgeReportsCompleter::Sync& completer) override;

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_input_report::InputReportsReaderV2> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    fdf::warn("Unexpected fidl method invoked: {}", metadata.method_ordinal);
  }

 private:
  static constexpr size_t kFidlReportBufferSize = 8192;

  struct RawReport {
    RawReport() {}
    std::array<uint8_t, fuchsia_hardware_hidbus::wire::kMaxReportLen> data;
    size_t size = 0;
    zx::time report_time;
    hid_input_report::Device* device = nullptr;
    ReportStamp stamp = 0;
  };

  void SendReports();

  const uint32_t reader_id_;
  // Limits maximum unacknowledged OnInputReports batch events in flight per FIDL spec.
  const uint16_t max_unacknowledged_reports_;
  InputReportBase* base_;
  fidl::ServerBinding<fuchsia_input_report::InputReportsReaderV2> binding_;
  fidl::Arena<kFidlReportBufferSize> report_allocator_;
  fbl::RingBuffer<RawReport, fuchsia_input_report::wire::kMaxDeviceReportCount> reports_data_;
  // Stores the report stamps of sent OnInputReports batch events awaiting client acknowledgment.
  std::deque<ReportStamp> unacknowledged_batches_;
  ReportStamp next_report_stamp_ = 1;
};

}  // namespace hid_input_report_dev

#endif  // SRC_UI_INPUT_DRIVERS_HID_INPUT_REPORT_INPUT_REPORTS_READER_V2_H_
