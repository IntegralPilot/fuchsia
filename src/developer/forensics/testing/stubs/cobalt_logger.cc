// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/testing/stubs/cobalt_logger.h"

namespace forensics {
namespace stubs {

void CobaltLoggerBase::InsertEvent(cobalt::EventType event_type, uint32_t metric_id,
                                   std::vector<uint32_t> event_codes, uint64_t count) {
  MarkMethodAsCalled(event_type);
  events_.push_back(cobalt::Event(event_type, metric_id, event_codes, count));
}

void CobaltLogger::LogOccurrence(LogOccurrenceRequest& request,
                                 LogOccurrenceCompleter::Sync& completer) {
  InsertEvent(cobalt::EventType::kOccurrence, request.metric_id(), request.event_codes(),
              request.count());
  completer.Reply(fit::success());
}

void CobaltLogger::LogInteger(LogIntegerRequest& request, LogIntegerCompleter::Sync& completer) {
  InsertEvent(cobalt::EventType::kInteger, request.metric_id(), request.event_codes(),
              request.value());
  completer.Reply(fit::success());
}

void CobaltLoggerIgnoresFirstEvents::LogOccurrence(LogOccurrenceRequest& request,
                                                   LogOccurrenceCompleter::Sync& completer) {
  if (call_idx_++ < ignore_call_count_) {
    ignored_completers_.push_back(completer.ToAsync());
    return;
  }

  InsertEvent(cobalt::EventType::kOccurrence, request.metric_id(), request.event_codes(),
              request.count());
  completer.Reply(fit::success());
}

}  // namespace stubs
}  // namespace forensics
