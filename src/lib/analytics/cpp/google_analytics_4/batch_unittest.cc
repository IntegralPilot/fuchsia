// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/analytics/cpp/google_analytics_4/batch.h"

#include <vector>

#include <gtest/gtest.h>

namespace analytics::google_analytics_4 {

class MockEvent : public Event {
 public:
  using Event::SetParameter;
  explicit MockEvent(std::string name) : Event(std::move(name)) {}
};

TEST(BatchTest, All) {
  std::vector<std::unique_ptr<Event>> sent_events;
  Batch batch([&](std::vector<std::unique_ptr<Event>> events) { sent_events = std::move(events); },
              3);
  auto event0 = std::make_unique<MockEvent>("test-event");
  event0->SetParameter("order", 0);
  auto event1 = std::make_unique<MockEvent>("other-event");
  event1->SetParameter("order", 1);
  auto event2 = std::make_unique<MockEvent>("test-event");
  event2->SetParameter("order", 2);
  auto event3 = std::make_unique<MockEvent>("test-event");
  event3->SetParameter("order", 3);

  batch.AddEvent(std::move(event0));
  EXPECT_EQ(sent_events.size(), 0u);
  batch.AddEvent(std::move(event1));
  EXPECT_EQ(sent_events.size(), 0u);
  batch.AddEvent(std::move(event2));
  ASSERT_EQ(sent_events.size(), 3u);
  EXPECT_EQ(std::get<int64_t>(sent_events[0]->parameters_opt()->at("order")), 0);
  EXPECT_EQ(std::get<int64_t>(sent_events[1]->parameters_opt()->at("order")), 1);
  EXPECT_EQ(std::get<int64_t>(sent_events[2]->parameters_opt()->at("order")), 2);
  sent_events.clear();
  batch.AddEvent(std::move(event3));
  EXPECT_EQ(sent_events.size(), 0u);
  batch.Send();
  ASSERT_EQ(sent_events.size(), 1u);
  EXPECT_EQ(std::get<int64_t>(sent_events[0]->parameters_opt()->at("order")), 3);
}

}  // namespace analytics::google_analytics_4
