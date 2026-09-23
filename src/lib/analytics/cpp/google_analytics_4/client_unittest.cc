// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/analytics/cpp/google_analytics_4/client.h"

#include <cstddef>
#include <memory>
#include <ostream>
#include <string>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/analytics/cpp/google_analytics_4/event.h"
#include "src/lib/fxl/strings/substitute.h"
#include "third_party/rapidjson/include/rapidjson/document.h"
#include "third_party/rapidjson/include/rapidjson/prettywriter.h"
#include "third_party/rapidjson/include/rapidjson/stringbuffer.h"

namespace rapidjson {

std::ostream& operator<<(std::ostream& os, const Document& doc) {
  StringBuffer s;
  PrettyWriter<StringBuffer> writer(s);
  writer.SetIndent(' ', 2);
  doc.Accept(writer);
  return os << s.GetString();
}

}  // namespace rapidjson

namespace analytics::google_analytics_4 {

class MockClient : public Client {
 public:
  MockClient() : Client(2) {}
  void expectEqData(const std::vector<std::string>& data) {
    ASSERT_EQ(data.size(), data_.size());
    for (size_t i = 0; i < data.size(); i++) {
      rapidjson::Document other, self;
      other.Parse(data[i]);
      self.Parse(data_[i]);
      EXPECT_EQ(self, other);
    }
  }

 private:
  void SendData(std::string body) override { data_.push_back(std::move(body)); }

  std::vector<std::string> data_;
};

class MockEvent : public Event {
 public:
  using Event::SetParameter;
  explicit MockEvent(std::string name) : Event(std::move(name)) {}
};

class ClientTest : public ::testing::Test {
 protected:
  void SetUp() override {
    client_.SetQueryParameters("IGNORED_IN_TEST_BUT_NEEDED", "IGNORED_IN_TEST_BUT_NEEDED");
    client_.SetClientId("TEST-CLIENT");
  }

  MockClient client_;
};

// When a null event is added, the library should not crash.
// Adding a null event is something we don't expect to happen, so we only
// require that the library does not crash. Particularly, the generated json is
// probably an invalid GA4 payload, which will be just a no-op to the GA4
// server.
TEST_F(ClientTest, NullEvent) {
  std::unique_ptr<Event> event(nullptr);
  std::string body =
      R"JSON(
      {
        "client_id": "TEST-CLIENT",
        "events": []
      }
  )JSON";
  client_.AddEvent(std::move(event));
  std::vector<std::string> data{body};
  client_.expectEqData(data);
}

TEST_F(ClientTest, SimpleEvent) {
  auto event = std::make_unique<MockEvent>("test-event");
  auto body = fxl::Substitute(
      R"JSON(
      {
        "client_id": "TEST-CLIENT",
        "events": [
          {
            "name": "test-event",
            "timestamp_micros": $0
          }
        ]
      }
  )JSON",
      std::to_string(event->timestamp_micros().count()));
  client_.AddEvent(std::move(event));
  std::vector<std::string> data{body};
  client_.expectEqData(data);
}

TEST_F(ClientTest, SimpleEventWithUserProperty) {
  client_.SetUserProperty("test-key", "test-val");
  auto event = std::make_unique<MockEvent>("test-event");
  auto body = fxl::Substitute(
      R"JSON(
      {
        "client_id": "TEST-CLIENT",
        "events": [
          {
            "name": "test-event",
            "timestamp_micros": $0
          }
        ],
        "user_properties": {"test-key": {"value": "test-val"}}
      }
  )JSON",
      std::to_string(event->timestamp_micros().count()));
  client_.AddEvent(std::move(event));
  std::vector<std::string> data{body};
  client_.expectEqData(data);
}

TEST_F(ClientTest, SimpleEventWithUserProperties) {
  client_.SetUserProperty("test-key-1", "test-val-1");
  client_.SetUserProperty("test-key-2", 2);
  client_.SetUserProperty("test-key-3", false);
  client_.SetUserProperty("test-key-4", 2.5);
  auto event = std::make_unique<MockEvent>("test-event");
  auto body = fxl::Substitute(
      R"JSON(
      {
        "client_id": "TEST-CLIENT",
        "events": [
          {
            "name": "test-event",
            "timestamp_micros": $0
          }
        ],
        "user_properties": {
          "test-key-1": {"value": "test-val-1"},
          "test-key-2": {"value": 2},
          "test-key-3": {"value": false},
          "test-key-4": {"value": 2.5}
        }
      }
    )JSON",
      std::to_string(event->timestamp_micros().count()));
  client_.AddEvent(std::move(event));
  std::vector<std::string> data{body};
  client_.expectEqData(data);
}

TEST_F(ClientTest, ConsecutiveEventsWithUserProperties) {
  client_.SetUserProperty("test-key-1", "test-val-1");
  client_.SetUserProperty("test-key-2", "test-val-2");
  auto event1 = std::make_unique<MockEvent>("test-event-1");
  auto body1 = fxl::Substitute(
      R"JSON(
      {
        "client_id": "TEST-CLIENT",
        "events": [
          {
            "name": "test-event-1",
            "timestamp_micros": $0
          }
        ],
        "user_properties": {
          "test-key-1": {"value": "test-val-1"},
          "test-key-2": {"value": "test-val-2"}
        }
      }
  )JSON",
      std::to_string(event1->timestamp_micros().count()));
  client_.AddEvent(std::move(event1));

  auto event2 = std::make_unique<MockEvent>("test-event-2");
  auto body2 = fxl::Substitute(
      R"JSON(
      {
        "client_id": "TEST-CLIENT",
        "events": [
          {
            "name": "test-event-2",
            "timestamp_micros": $0
          }
        ],
        "user_properties": {
          "test-key-1": {"value": "test-val-1"},
          "test-key-2": {"value": "test-val-2"}
        }
      }
  )JSON",
      std::to_string(event2->timestamp_micros().count()));
  client_.AddEvent(std::move(event2));
  std::vector<std::string> data{body1, body2};
  client_.expectEqData(data);
}

TEST_F(ClientTest, EventWithParameters) {
  auto event = std::make_unique<MockEvent>("test-event");
  event->SetParameter("test-key-1", "test-val-1");
  event->SetParameter("test-key-2", 2);
  event->SetParameter("test-key-3", false);
  event->SetParameter("test-key-4", 2.5);
  auto body = fxl::Substitute(
      R"JSON(
      {
        "client_id": "TEST-CLIENT",
        "events": [
          {
            "name": "test-event",
            "params": {
              "test-key-1": "test-val-1",
              "test-key-2": 2,
              "test-key-3": false,
              "test-key-4": 2.5
            },
            "timestamp_micros": $0
          }
        ]
      }
  )JSON",
      std::to_string(event->timestamp_micros().count()));
  client_.AddEvent(std::move(event));
  std::vector<std::string> data{body};
  client_.expectEqData(data);
}

TEST_F(ClientTest, AddEventsLessThanBatchSize) {
  size_t batch_size = 3;
  client_.SetUserProperty("user", "property");

  auto event = std::make_unique<MockEvent>("test-event");
  event->SetParameter("order", 0);
  std::string body = fxl::Substitute(R"JSON(
      {
        "client_id": "TEST-CLIENT",
        "events": [
          {
            "name": "test-event",
            "params": {
              "order": 0
            },
            "timestamp_micros": $0
          }
    ],
    "user_properties": {
          "user": {"value": "property"}
    }
  }
  )JSON",
                                     std::to_string(event->timestamp_micros().count()));

  std::vector<std::unique_ptr<Event>> events;
  events.push_back(std::move(event));
  std::vector<std::string> data{body};

  client_.AddEvents(std::move(events), batch_size);
  client_.expectEqData(data);
}

TEST_F(ClientTest, AddEventsEqualBatchSize) {
  size_t batch_size = 3;
  client_.SetUserProperty("user", "property");

  auto event0 = std::make_unique<MockEvent>("test-event");
  event0->SetParameter("order", 0);
  auto event1 = std::make_unique<MockEvent>("other-event");
  event1->SetParameter("order", 1);
  auto event2 = std::make_unique<MockEvent>("test-event");
  event2->SetParameter("order", 2);
  std::string body = fxl::Substitute(R"JSON(
      {
        "client_id": "TEST-CLIENT",
        "events": [
          {
            "name": "test-event",
            "params": {
              "order": 0
            },
            "timestamp_micros": $0
          },
          {
            "name": "other-event",
            "params": {
              "order": 1
            },
            "timestamp_micros": $1
          },
          {
            "name": "test-event",
            "params": {
              "order": 2
            },
            "timestamp_micros": $2
          }
    ],
    "user_properties": {
          "user": {"value": "property"}
    }
  }
  )JSON",
                                     std::to_string(event0->timestamp_micros().count()),
                                     std::to_string(event1->timestamp_micros().count()),
                                     std::to_string(event2->timestamp_micros().count()));

  std::vector<std::unique_ptr<Event>> events;
  events.push_back(std::move(event0));
  events.push_back(std::move(event1));
  events.push_back(std::move(event2));
  std::vector<std::string> data{body};

  client_.AddEvents(std::move(events), batch_size);
  client_.expectEqData(data);
}

TEST_F(ClientTest, AddEventsLargerThanBatchSize) {
  size_t batch_size = 2;
  client_.SetUserProperty("user", "property");

  auto event0 = std::make_unique<MockEvent>("test-event");
  event0->SetParameter("order", 0);
  auto event1 = std::make_unique<MockEvent>("other-event");
  event1->SetParameter("order", 1);
  auto event2 = std::make_unique<MockEvent>("test-event");
  event2->SetParameter("order", 2);
  std::string body1 = fxl::Substitute(R"JSON(
      {
        "client_id": "TEST-CLIENT",
        "events": [
          {
            "name": "test-event",
            "params": {
              "order": 0
            },
            "timestamp_micros": $0
          },
          {
            "name": "other-event",
            "params": {
              "order": 1
            },
            "timestamp_micros": $1
          }
    ],
    "user_properties": {
          "user": {"value": "property"}
    }
  }
  )JSON",
                                      std::to_string(event0->timestamp_micros().count()),
                                      std::to_string(event1->timestamp_micros().count()));
  std::string body2 = fxl::Substitute(R"JSON(
      {
        "client_id": "TEST-CLIENT",
        "events": [
          {
            "name": "test-event",
            "params": {
              "order": 2
            },
            "timestamp_micros": $0
          }
    ],
    "user_properties": {
          "user": {"value": "property"}
    }
  }
  )JSON",
                                      std::to_string(event2->timestamp_micros().count()));

  std::vector<std::unique_ptr<Event>> events;
  events.push_back(std::move(event0));
  events.push_back(std::move(event1));
  events.push_back(std::move(event2));
  std::vector<std::string> data{body1, body2};

  client_.AddEvents(std::move(events), batch_size);
  client_.expectEqData(data);
}

TEST_F(ClientTest, AddEventsDoublesBatchSize) {
  size_t batch_size = 2;
  client_.SetUserProperty("user", "property");

  auto event0 = std::make_unique<MockEvent>("test-event");
  event0->SetParameter("order", 0);
  auto event1 = std::make_unique<MockEvent>("other-event");
  event1->SetParameter("order", 1);
  auto event2 = std::make_unique<MockEvent>("test-event");
  event2->SetParameter("order", 2);
  auto event3 = std::make_unique<MockEvent>("test-event");
  event3->SetParameter("order", 3);

  std::string body1 = fxl::Substitute(R"JSON(
      {
        "client_id": "TEST-CLIENT",
        "events": [
          {
            "name": "test-event",
            "params": {
              "order": 0
            },
            "timestamp_micros": $0
          },
          {
            "name": "other-event",
            "params": {
              "order": 1
            },
            "timestamp_micros": $1
          }
    ],
    "user_properties": {
          "user": {"value": "property"}
    }
  }
  )JSON",
                                      std::to_string(event0->timestamp_micros().count()),
                                      std::to_string(event1->timestamp_micros().count()));
  std::string body2 = fxl::Substitute(R"JSON(
      {
        "client_id": "TEST-CLIENT",
        "events": [
          {
            "name": "test-event",
            "params": {
              "order": 2
            },
            "timestamp_micros": $0
          },
          {
            "name": "test-event",
            "params": {
              "order": 3
            },
            "timestamp_micros": $1
          }
    ],
    "user_properties": {
          "user": {"value": "property"}
    }
  }
  )JSON",
                                      std::to_string(event2->timestamp_micros().count()),
                                      std::to_string(event3->timestamp_micros().count()));

  std::vector<std::unique_ptr<Event>> events;
  events.push_back(std::move(event0));
  events.push_back(std::move(event1));
  events.push_back(std::move(event2));
  events.push_back(std::move(event3));
  std::vector<std::string> data{body1, body2};
  client_.AddEvents(std::move(events), batch_size);
  client_.expectEqData(data);
}

// When there is a null event mixed in a vector of events, the library should
// not crash. Note that the number of events in each batch might be affected.
// We allow this behavior since (1) we do not expect having null events
// (2) the difference made on the batch will eventually lead to the same
// result.
TEST_F(ClientTest, NullInEvents) {
  size_t batch_size = 2;
  client_.SetUserProperty("user", "property");

  auto event0 = std::make_unique<MockEvent>("test-event");
  event0->SetParameter("order", 0);
  auto event1 = std::make_unique<MockEvent>("other-event");
  event1->SetParameter("order", 1);
  auto event2 = std::make_unique<MockEvent>("test-event");
  event2->SetParameter("order", 2);
  std::unique_ptr<Event> event_null(nullptr);
  std::string body1 = fxl::Substitute(R"JSON(
      {
        "client_id": "TEST-CLIENT",
        "events": [
          {
            "name": "test-event",
            "params": {
              "order": 0
            },
            "timestamp_micros": $0
          },
          {
            "name": "other-event",
            "params": {
              "order": 1
            },
            "timestamp_micros": $1
          }
    ],
    "user_properties": {
          "user": {"value": "property"}
    }
  }
  )JSON",
                                      std::to_string(event0->timestamp_micros().count()),
                                      std::to_string(event1->timestamp_micros().count()));
  std::string body2 = fxl::Substitute(R"JSON(
      {
        "client_id": "TEST-CLIENT",
        "events": [
          {
            "name": "test-event",
            "params": {
              "order": 2
            },
            "timestamp_micros": $0
          }
    ],
    "user_properties": {
          "user": {"value": "property"}
    }
  }
  )JSON",
                                      std::to_string(event2->timestamp_micros().count()));

  std::vector<std::unique_ptr<Event>> events;
  events.push_back(std::move(event0));
  events.push_back(std::move(event1));
  events.push_back(std::move(event_null));
  events.push_back(std::move(event2));
  std::vector<std::string> data{body1, body2};

  client_.AddEvents(std::move(events), batch_size);
  client_.expectEqData(data);
}

TEST_F(ClientTest, AddEventsToBatch) {
  client_.SetUserProperty("user", "property");

  auto event0 = std::make_unique<MockEvent>("test-event");
  event0->SetParameter("order", 0);
  auto event1 = std::make_unique<MockEvent>("other-event");
  event1->SetParameter("order", 1);
  auto event2 = std::make_unique<MockEvent>("test-event");
  event2->SetParameter("order", 2);
  std::string body1 = fxl::Substitute(R"JSON(
      {
        "client_id": "TEST-CLIENT",
        "events": [
          {
            "name": "test-event",
            "params": {
              "order": 0
            },
            "timestamp_micros": $0
          },
          {
            "name": "other-event",
            "params": {
              "order": 1
            },
            "timestamp_micros": $1
          }
    ],
    "user_properties": {
          "user": {"value": "property"}
    }
  }
  )JSON",
                                      std::to_string(event0->timestamp_micros().count()),
                                      std::to_string(event1->timestamp_micros().count()));
  std::string body2 = fxl::Substitute(R"JSON(
      {
        "client_id": "TEST-CLIENT",
        "events": [
          {
            "name": "test-event",
            "params": {
              "order": 2
            },
            "timestamp_micros": $0
          }
    ],
    "user_properties": {
          "user": {"value": "property"}
    }
  }
  )JSON",
                                      std::to_string(event2->timestamp_micros().count()));

  std::vector<std::string> data{body1, body2};

  client_.AddEventToDefaultBatch(std::move(event0));
  client_.AddEventToDefaultBatch(std::move(event1));
  client_.AddEventToDefaultBatch(std::move(event2));
  client_.SendDefaultBatch();
  client_.expectEqData(data);
}

}  // namespace analytics::google_analytics_4
