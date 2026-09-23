// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "reports_reader.h"

#include <lib/async/cpp/task.h>
#include <zircon/assert.h>

#include <algorithm>

#include <fbl/auto_lock.h>

namespace fake_input_report_device {

FakeInputReportsReaderV2::FakeInputReportsReaderV2(
    fidl::InterfaceRequest<fuchsia::input::report::InputReportsReaderV2> request,
    async_dispatcher_t* dispatcher, uint16_t max_unacknowledged_reports)
    : binding_(this, std::move(request), dispatcher),
      max_unacknowledged_reports_(std::max<uint16_t>(1, max_unacknowledged_reports)) {
  zx_status_t status = zx::event::create(0, &shutdown_event_);
  ZX_ASSERT(status == ZX_OK);
  dispatcher_shutdown_.emplace(shutdown_event_.get());
  status = dispatcher_shutdown_->Begin(
      binding_.dispatcher(), [this](async_dispatcher_t* dispatcher, async::WaitOnce* wait,
                                    zx_status_t status, const zx_packet_signal_t* signal) {
        fbl::AutoLock lock(&lock_);
        pending_reports_.clear();
      });
  ZX_ASSERT(status == ZX_OK);
}

void FakeInputReportsReaderV2::AcknowledgeReports(uint64_t last_acknowledged_report_stamp) {
  fbl::AutoLock lock(&lock_);
  if (last_acknowledged_report_stamp > last_acknowledged_report_stamp_) {
    last_acknowledged_report_stamp_ = std::min(last_acknowledged_report_stamp, last_report_stamp_);
  }
  SendReportsLocked();
}

void FakeInputReportsReaderV2::SendReports(
    std::vector<fuchsia::input::report::InputReport> reports) {
  if (reports.empty()) {
    return;
  }
  fbl::AutoLock lock(&lock_);
  for (auto& report : reports) {
    pending_reports_.push_back(std::move(report));
  }
  async::PostTask(binding_.dispatcher(), [this]() {
    fbl::AutoLock lock(&lock_);
    SendReportsLocked();
  });
}

void FakeInputReportsReaderV2::SendReportsLocked() {
  if (!binding_.is_bound()) {
    return;
  }
  while (binding_.is_bound() && !pending_reports_.empty()) {
    uint64_t unacknowledged = last_report_stamp_ > last_acknowledged_report_stamp_
                                  ? (last_report_stamp_ - last_acknowledged_report_stamp_)
                                  : 0;
    if (unacknowledged >= max_unacknowledged_reports_) {
      break;
    }
    size_t available_capacity = static_cast<size_t>(max_unacknowledged_reports_ - unacknowledged);
    size_t batch_size = std::min(
        {pending_reports_.size(),
         static_cast<size_t>(fuchsia::input::report::MAX_DEVICE_REPORT_COUNT), available_capacity});
    if (batch_size == 0) {
      break;
    }
    std::vector<fuchsia::input::report::InputReport> batch;
    batch.reserve(batch_size);
    for (size_t i = 0; i < batch_size; ++i) {
      batch.push_back(std::move(pending_reports_.front()));
      pending_reports_.pop_front();
    }
    last_report_stamp_ += batch.size();
    binding_.events().OnInputReports(std::move(batch), last_report_stamp_);
  }
}

}  // namespace fake_input_report_device
