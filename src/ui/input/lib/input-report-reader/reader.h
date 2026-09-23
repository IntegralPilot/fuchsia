// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_INPUT_LIB_INPUT_REPORT_READER_READER_H_
#define SRC_UI_INPUT_LIB_INPUT_REPORT_READER_READER_H_

#include <fidl/fuchsia.input.report/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/stdcompat/inplace_vector.h>
#include <lib/trace/event.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>

#include <array>
#include <deque>
#include <functional>
#include <list>
#include <memory>
#include <mutex>
#include <new>
#include <optional>

#include <fbl/alloc_checker.h>
#include <fbl/strong_int.h>

namespace input_report_reader {

// InputReportReaderManager is used to simplify implementation of input drivers. An input driver may
// use InputReportReaderManager to keep track of all upstream readers that want to receive reports.
// An upstream driver that wants to read input reports from this device may register with
// InputReportReaderManager, which calls CreateReader. When an input report arrives, whether in the
// form of HID reports or device readings by polling, etc., the report is pushed to all readers
// registered by calling SendReportToAllReaders where it is then translated to
// fuchsia_input_report::InputReport.
//
// If kMaxUnreadReports is non-zero, then at most that many reports are allowed to accumulate for
// any client before reports are dropped, starting with the oldest ones first.
//
// If kMaxBatchSize is greater than 1, then multiple reports may be batched together before being
// sent to clients.
//
// If kMaxBatchSize is greater than 1, then kMaxBatchDelayNs must be set. This is the time in
// nanoseconds from receiving the first input report that we will wait before sending a batch to
// clients.
//
// This class creates and manages the InputReportReaders. It is able to send reports
// to all existing InputReportReaders.
// When this class is destructed, all of the InputReportReaders will be freed.
// This class is thread-safe.
// Typical Usage:
// An InputReport Driver should have one InputReportReaderManager member object.
// The Driver should also have some form of InputReport object that can be converted to Fidl.
//
// Eg:
//
// class MyTouchScreenDriver {
// ...
// private:
//   struct TouchScreenReport {
//      int64_t x;
//      int64_t y;
//      void ToFidlInputReport(fidl::WireTableBuilder<::fuchsia_input_report::wire::InputReport>&
//                             input_report, fidl::AnyArena& allocator) const;
//   };
//
//   InputReportReaderManager<TouchScreenReport> input_report_readers_;
// };
//
// See
// https://fuchsia.dev/fuchsia-src/development/drivers/concepts/driver_architectures/input_drivers/input?hl=en
template <class Report, size_t kMaxUnreadReports = 0, size_t kMaxBatchSize = 1,
          zx_duration_t kMaxBatchDelayNs = 0>
class InputReportReaderManager final {
 private:
  class InputReportReader;

 public:
  InputReportReaderManager() = default;
  // This object can't be moved, because InputReportReaders point to this object.
  InputReportReaderManager(const InputReportReaderManager&) = delete;
  InputReportReaderManager(InputReportReaderManager&&) = delete;
  InputReportReaderManager& operator=(const InputReportReaderManager&) = delete;
  InputReportReaderManager& operator=(InputReportReaderManager&&) = delete;

  // Create a new InputReportReader that is managed by this InputReportReaderManager. If
  // initial_report exists, InputReportReaderManager will send initial_report to the new reader.
  zx_status_t CreateReader(async_dispatcher_t* dispatcher,
                           fidl::ServerEnd<fuchsia_input_report::InputReportsReader> server,
                           std::optional<Report> initial_report = std::nullopt) {
    ZX_PANIC("GetInputReportsReader is no longer supported; use GetInputReportsReaderV2");
  }

  // Create a new InputReportReader that is managed by this InputReportReaderManager. If
  // initial_report exists, InputReportReaderManager will send initial_report to the new reader.
  //
  // `dispatcher` must be non-null.
  // `max_unacknowledged_reports` must be at least 1.
  zx_status_t CreateReaderV2(async_dispatcher_t* dispatcher,
                             fidl::ServerEnd<fuchsia_input_report::InputReportsReaderV2> server,
                             uint16_t max_unacknowledged_reports,
                             std::optional<Report> initial_report = std::nullopt) {
    ZX_ASSERT(dispatcher);
    ZX_ASSERT(max_unacknowledged_reports >= 1);
    std::scoped_lock lock(lock_);
    fbl::AllocChecker ac;
    std::unique_ptr<InputReportReader> reader(new (&ac) InputReportReader(
        this, next_reader_id_, dispatcher, std::move(server), max_unacknowledged_reports));
    if (!ac.check()) {
      return ZX_ERR_NO_MEMORY;
    }
    next_reader_id_++;
    if (initial_report.has_value()) {
      reader->ReceiveReport(std::move(*initial_report));
    }
    readers_list_.push_back(std::move(reader));
    return ZX_OK;
  }

  // Send a report to all InputReportReaders. Returns the total number of reports that are dropped
  // due to InputReportReader report queues being full.
  size_t SendReportToAllReaders(const Report& report) {
    std::scoped_lock lock(lock_);
    size_t dropped_reports = 0;
    for (auto& reader : readers_list_) {
      dropped_reports += reader->ReceiveReport(report);
    }
    return dropped_reports;
  }

  // Remove a given reader from the list. This is called by the InputReportReader itself
  // when it wishes to be removed.
  void RemoveReaderFromList(InputReportReader* reader) {
    std::scoped_lock lock(lock_);
    std::erase_if(readers_list_, [reader](const std::unique_ptr<InputReportReader>& item) {
      return item.get() == reader;
    });
  }

 private:
  static_assert(kMaxBatchSize == 1 || (kMaxBatchSize > 1 && kMaxBatchDelayNs > 0));
  static_assert(kMaxUnreadReports == 0 || (kMaxUnreadReports >= kMaxBatchSize));
  // Assert that our template type `Report` has the following function:
  //      void ToFidlInputReport(fidl::WireTableBuilder<::fuchsia_input_report::wire::InputReport>&
  //                             input_report, fidl::AnyArena& allocator) const;
  //
  // TODO(https://fxbug.dev/380355303): Replace the type traits with concepts
  // when concepts are supported.
  template <typename T>
  struct has_to_fidl_input_report {
   private:
    template <typename C>
    static std::true_type test(
        decltype(static_cast<void (C::*)(
                     fidl::WireTableBuilder<fuchsia_input_report::wire::InputReport>& input_report,
                     fidl::AnyArena& allocator) const>(&C::ToFidlInputReport)));
    template <typename C>
    static std::false_type test(...);

   public:
    static constexpr bool value = decltype(test<T>(nullptr))::value;
  };
  static_assert(
      has_to_fidl_input_report<Report>::value,
      "Report must implement void "
      "ToFidlInputReport(fidl::WireTableBuilder<::fuchsia_input_report::wire::InputReport>& "
      "input_report, fidl::AnyArena& allocator) const;");

  std::mutex lock_;
  size_t next_reader_id_ __TA_GUARDED(lock_) = 1;
  std::list<std::unique_ptr<InputReportReader>> readers_list_ __TA_GUARDED(lock_);
};

// This class represents an InputReportReader that sends InputReports out to a specific client.
// This class is thread safe.
// Typical usage:
//  This class shouldn't be touched directly. An InputReport driver should only manipulate
//  the InputReportReaderManager.
template <class Report, size_t kMaxUnreadReports, size_t kMaxBatchSize,
          zx_duration_t kMaxBatchDelayNs>
class InputReportReaderManager<Report, kMaxUnreadReports, kMaxBatchSize,
                               kMaxBatchDelayNs>::InputReportReader final
    : public fidl::WireServer<fuchsia_input_report::InputReportsReaderV2> {
 public:
  // `manager` and `dispatcher` must be non-null and must outlive this reader.
  // `server` must be valid.
  // `max_unacknowledged_reports` must be > 0.
  explicit InputReportReader(
      InputReportReaderManager<Report, kMaxUnreadReports, kMaxBatchSize, kMaxBatchDelayNs>* manager,
      size_t reader_id, async_dispatcher_t* dispatcher,
      fidl::ServerEnd<fuchsia_input_report::InputReportsReaderV2> server,
      uint16_t max_unacknowledged_on_input_reports_events)
      : dispatcher_(dispatcher),
        manager_(manager),
        binding_(dispatcher, std::move(server), this, std::mem_fn(&InputReportReader::OnUnbound)),
        reader_id_(reader_id),
        max_unacknowledged_on_input_reports_events_(max_unacknowledged_on_input_reports_events) {
    ZX_DEBUG_ASSERT(manager_ != nullptr);
    ZX_DEBUG_ASSERT(dispatcher_ != nullptr);
    ZX_DEBUG_ASSERT(max_unacknowledged_on_input_reports_events_ > 0);
  }

  size_t ReceiveReport(const Report& report) __TA_EXCLUDES(&report_lock_);

  void AcknowledgeReports(
      fidl::WireServer<fuchsia_input_report::InputReportsReaderV2>::AcknowledgeReportsRequestView
          request,
      AcknowledgeReportsCompleter::Sync& completer) __TA_EXCLUDES(&report_lock_) override;

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_input_report::InputReportsReaderV2> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

 private:
  static constexpr size_t kInputReportBufferSize = 4096 * 4;

  DEFINE_STRONG_INT(ReportStamp, uint64_t);

  void DelayedSend() __TA_EXCLUDES(&report_lock_);
  void SendReports(bool is_delayed = false) __TA_REQUIRES(&report_lock_);
  void OnUnbound(fidl::UnbindInfo info) {
    ZX_DEBUG_ASSERT(manager_ != nullptr);
    manager_->RemoveReaderFromList(this);
  }

  async_dispatcher_t* const dispatcher_;
  InputReportReaderManager<Report, kMaxUnreadReports, kMaxBatchSize, kMaxBatchDelayNs>* const
      manager_;
  fidl::ServerBinding<fuchsia_input_report::InputReportsReaderV2> binding_;

  std::mutex report_lock_;
  fidl::Arena<kInputReportBufferSize> report_allocator_ __TA_GUARDED(report_lock_);
  std::deque<Report> stamped_reports_ __TA_GUARDED(report_lock_);
  // Stores the report stamps of sent OnInputReports batch events awaiting client acknowledgment.
  std::deque<ReportStamp> unacknowledged_report_stamps_ __TA_GUARDED(report_lock_);
  ReportStamp next_report_stamp_ __TA_GUARDED(report_lock_){1};
  async::TaskClosureMethod<InputReportReader, &InputReportReader::DelayedSend> batch_task_
      __TA_GUARDED(report_lock_){this};

  const size_t reader_id_;
  // Limits maximum unacknowledged OnInputReports batch events in flight per FIDL spec.
  const uint16_t max_unacknowledged_on_input_reports_events_;
};

// Template Implementation.
template <class Report, size_t kMaxUnreadReports, size_t kMaxBatchSize,
          zx_duration_t kMaxBatchDelayNs>
inline size_t
InputReportReaderManager<Report, kMaxUnreadReports, kMaxBatchSize,
                         kMaxBatchDelayNs>::InputReportReader::ReceiveReport(const Report& report) {
  std::scoped_lock lock(report_lock_);

  size_t dropped_reports = 0;
  if constexpr (kMaxUnreadReports > 0) {
    // Drop old reports if the client isn't reading them out fast enough.
    while (stamped_reports_.size() >= kMaxUnreadReports) {
      TRACE_INSTANT("input", "InputReportDrop", TRACE_SCOPE_PROCESS);
      stamped_reports_.pop_front();
      dropped_reports++;
    }
  }

  stamped_reports_.push_back(report);
  SendReports();
  return dropped_reports;
}

template <class Report, size_t kMaxUnreadReports, size_t kMaxBatchSize,
          zx_duration_t kMaxBatchDelayNs>
inline void InputReportReaderManager<Report, kMaxUnreadReports, kMaxBatchSize, kMaxBatchDelayNs>::
    InputReportReader::AcknowledgeReports(
        fidl::WireServer<fuchsia_input_report::InputReportsReaderV2>::AcknowledgeReportsRequestView
            request,
        AcknowledgeReportsCompleter::Sync& completer) {
  std::scoped_lock lock(report_lock_);
  while (!unacknowledged_report_stamps_.empty() &&
         unacknowledged_report_stamps_.front() <=
             ReportStamp(request->last_acknowledged_report_stamp)) {
    unacknowledged_report_stamps_.pop_front();
  }
  if (!batch_task_.is_pending()) {
    SendReports(/*is_running_in_delayed_task=*/true);
  } else {
    SendReports();
  }
}

template <class Report, size_t kMaxUnreadReports, size_t kMaxBatchSize,
          zx_duration_t kMaxBatchDelayNs>
inline void InputReportReaderManager<Report, kMaxUnreadReports, kMaxBatchSize,
                                     kMaxBatchDelayNs>::InputReportReader::DelayedSend() {
  std::scoped_lock lock(report_lock_);
  SendReports(/*is_delayed=*/true);
}

template <class Report, size_t kMaxUnreadReports, size_t kMaxBatchSize,
          zx_duration_t kMaxBatchDelayNs>
inline void InputReportReaderManager<Report, kMaxUnreadReports, kMaxBatchSize, kMaxBatchDelayNs>::
    InputReportReader::SendReports(const bool is_running_in_delayed_task) {
  if constexpr (kMaxBatchSize > 1) {
    if (!is_running_in_delayed_task) {
      if (stamped_reports_.size() < kMaxBatchSize) {
        if (!batch_task_.is_pending()) {
          batch_task_.PostDelayed(dispatcher_, zx::duration(kMaxBatchDelayNs));
        }
        return;
      }
      if (stamped_reports_.size() >= kMaxBatchSize) {
        batch_task_.Cancel();
      }
    }
  }

  while (!stamped_reports_.empty() &&
         unacknowledged_report_stamps_.size() < max_unacknowledged_on_input_reports_events_) {
    cpp26::inplace_vector<fuchsia_input_report::wire::InputReport,
                          fuchsia_input_report::wire::kMaxDeviceReportCount>
        reports;

    TRACE_DURATION("input", "InputReportReader SendReports", "instance_id", reader_id_);
    for (; !stamped_reports_.empty() && reports.size() < reports.capacity();) {
      // Build the report.
      auto input_report = fuchsia_input_report::wire::InputReport::Builder(report_allocator_);

      // Add some common fields. Will be overwritten if set.
      input_report.trace_id(TRACE_NONCE());
      input_report.event_time(zx_clock_get_monotonic());

      stamped_reports_.front().ToFidlInputReport(input_report, report_allocator_);

      reports.push_back(input_report.Build());

      TRACE_FLOW_BEGIN("input", "input_report", reports.back().trace_id());
      stamped_reports_.pop_front();
    }

    if (!reports.empty()) {
      auto reports_view = fidl::VectorView<fuchsia_input_report::wire::InputReport>::FromExternal(
          reports.data(), reports.size());

      ReportStamp stamp = next_report_stamp_;
      next_report_stamp_++;

      fidl::Status status =
          fidl::WireSendEvent(binding_)->OnInputReports(reports_view, stamp.value());
      if (!status.ok()) {
        report_allocator_.Reset();
        break;
      }
      unacknowledged_report_stamps_.push_back(stamp);
    }
    report_allocator_.Reset();
  }
}

}  // namespace input_report_reader

#endif  // SRC_UI_INPUT_LIB_INPUT_REPORT_READER_READER_H_
