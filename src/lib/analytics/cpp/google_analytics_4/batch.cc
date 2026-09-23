// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/analytics/cpp/google_analytics_4/batch.h"

namespace analytics::google_analytics_4 {

Batch::Batch(fit::function<void(std::vector<std::unique_ptr<Event>>)> send_events,
             size_t batch_size)
    : batch_size_(batch_size == 0 ? 1 : batch_size), send_events_(std::move(send_events)) {}

void Batch::AddEvent(std::unique_ptr<Event> event_ptr) {
  events_.push_back(std::move(event_ptr));
  if (events_.size() == batch_size_) {
    Send();
  }
}

void Batch::Send() {
  if (!events_.empty()) {
    send_events_(std::move(events_));
    events_.clear();
  }
}

}  // namespace analytics::google_analytics_4
