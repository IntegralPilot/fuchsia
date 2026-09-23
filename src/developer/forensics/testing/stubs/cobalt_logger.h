// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_TESTING_STUBS_COBALT_LOGGER_H_
#define SRC_DEVELOPER_FORENSICS_TESTING_STUBS_COBALT_LOGGER_H_

#include <fidl/fuchsia.metrics/cpp/fidl.h>
#include <fidl/fuchsia.metrics/cpp/test_base.h>

#include <set>
#include <vector>

#include "src/developer/forensics/testing/stubs/fidl_server.h"
#include "src/developer/forensics/utils/cobalt/event.h"

namespace forensics {
namespace stubs {

// Defines the interface all stub loggers must implement and provides common functionality.
class CobaltLoggerBase : public SingleBindingFidlServer<fuchsia_metrics::MetricEventLogger> {
 public:
  virtual ~CobaltLoggerBase() = default;

  const cobalt::Event& LastEvent() const { return events_.back(); }
  const std::vector<cobalt::Event>& Events() const { return events_; }

  bool WasMethodCalled(cobalt::EventType method) const { return calls_.contains(method); }

 protected:
  void InsertEvent(cobalt::EventType event_type, uint32_t metric_id,
                   std::vector<uint32_t> event_codes, uint64_t count);

  void MarkMethodAsCalled(cobalt::EventType method) { calls_.insert(method); }

 private:
  std::vector<cobalt::Event> events_;
  std::set<cobalt::EventType> calls_;
};

// Always record |metric_id| and |event_code| and call callback with |Status::OK|.
class CobaltLogger : public CobaltLoggerBase {
 public:
  // |fuchsia_metrics::MetricEventLogger|
  void LogOccurrence(LogOccurrenceRequest& request,
                     LogOccurrenceCompleter::Sync& completer) override;

  void LogInteger(LogIntegerRequest& request, LogIntegerCompleter::Sync& completer) override;

  void LogIntegerHistogram(LogIntegerHistogramRequest& request,
                           LogIntegerHistogramCompleter::Sync& completer) override {
    // Not Implemented
    completer.Reply(fit::error(fuchsia_metrics::Error::kInvalidArguments));
  }

  void LogString(LogStringRequest& request, LogStringCompleter::Sync& completer) override {
    // Not Implemented
    completer.Reply(fit::error(fuchsia_metrics::Error::kInvalidArguments));
  }
};

// Will not execute the callback for the first n events.
class CobaltLoggerIgnoresFirstEvents : public CobaltLoggerBase {
 public:
  explicit CobaltLoggerIgnoresFirstEvents(int n) : ignore_call_count_(n) {}

  // |fuchsia_metrics::MetricEventLogger|
  void LogOccurrence(LogOccurrenceRequest& request,
                     LogOccurrenceCompleter::Sync& completer) override;

 private:
  int ignore_call_count_;
  int call_idx_ = 0;
  std::vector<LogOccurrenceCompleter::Async> ignored_completers_;
};

}  // namespace stubs
}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_TESTING_STUBS_COBALT_LOGGER_H_
