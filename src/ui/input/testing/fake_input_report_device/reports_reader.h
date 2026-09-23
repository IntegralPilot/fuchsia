// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_INPUT_TESTING_FAKE_INPUT_REPORT_DEVICE_REPORTS_READER_H_
#define SRC_UI_INPUT_TESTING_FAKE_INPUT_REPORT_DEVICE_REPORTS_READER_H_

#include <fuchsia/input/report/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/wait.h>
#include <lib/async/time.h>
#include <lib/fidl/cpp/binding_set.h>

#include <deque>
#include <optional>
#include <vector>

#include <fbl/auto_lock.h>
#include <fbl/mutex.h>

namespace fake_input_report_device {

// Creates a fake class that vends the InputReportsReaderV2 API. This should be
// created and managed by FakeInputDevice.
// If this class is bound on a separate thread, that thread must be joined before
// this class is destructed.
class FakeInputReportsReaderV2 final : public fuchsia::input::report::InputReportsReaderV2 {
 public:
  explicit FakeInputReportsReaderV2(
      fidl::InterfaceRequest<fuchsia::input::report::InputReportsReaderV2> request,
      async_dispatcher_t* dispatcher, uint16_t max_unacknowledged_reports);

  void AcknowledgeReports(uint64_t last_acknowledged_report_stamp) override;
  void handle_unknown_method(uint64_t ordinal, bool method_has_response) override {}

  // Queues and sends reports to the client.
  void SendReports(std::vector<fuchsia::input::report::InputReport> reports);

 private:
  void SendReportsLocked() __TA_REQUIRES(lock_);

  // `shutdown_event_` must be declared before `dispatcher_shutdown_` so that `dispatcher_shutdown_`
  // is destructed (and cancelled) before `shutdown_event_` handle is closed.
  zx::event shutdown_event_;
  std::optional<async::WaitOnce> dispatcher_shutdown_;

  fbl::Mutex lock_;
  fidl::Binding<fuchsia::input::report::InputReportsReaderV2> binding_ __TA_GUARDED(lock_);
  const uint16_t max_unacknowledged_reports_;
  uint64_t last_report_stamp_ __TA_GUARDED(lock_) = 0;
  uint64_t last_acknowledged_report_stamp_ __TA_GUARDED(lock_) = 0;
  std::deque<fuchsia::input::report::InputReport> pending_reports_ __TA_GUARDED(lock_);
};

}  // namespace fake_input_report_device

#endif  // SRC_UI_INPUT_TESTING_FAKE_INPUT_REPORT_DEVICE_REPORTS_READER_H_
