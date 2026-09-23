// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ANALYTICS_CPP_GOOGLE_ANALYTICS_4_BATCH_H_
#define SRC_LIB_ANALYTICS_CPP_GOOGLE_ANALYTICS_4_BATCH_H_

#include <cstddef>
#include <vector>

#include "lib/fit/function.h"
#include "src/lib/analytics/cpp/google_analytics_4/event.h"

namespace analytics::google_analytics_4 {

class Batch {
 public:
  Batch(const Batch&) = delete;

  Batch(fit::function<void(std::vector<std::unique_ptr<Event>>)> send_events, size_t batch_size);

  void AddEvent(std::unique_ptr<Event> event_ptr);
  void Send();

 private:
  size_t batch_size_;
  std::vector<std::unique_ptr<Event>> events_;
  fit::function<void(std::vector<std::unique_ptr<Event>>)> send_events_;
};

}  // namespace analytics::google_analytics_4

#endif  // SRC_LIB_ANALYTICS_CPP_GOOGLE_ANALYTICS_4_BATCH_H_
