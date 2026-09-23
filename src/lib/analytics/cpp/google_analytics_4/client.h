// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ANALYTICS_CPP_GOOGLE_ANALYTICS_4_CLIENT_H_
#define SRC_LIB_ANALYTICS_CPP_GOOGLE_ANALYTICS_4_CLIENT_H_

#include <memory>
#include <string>
#include <string_view>

#include "src/lib/analytics/cpp/google_analytics_4/batch.h"
#include "src/lib/analytics/cpp/google_analytics_4/event.h"
#include "src/lib/analytics/cpp/google_analytics_4/measurement.h"

namespace analytics::google_analytics_4 {

// This is an abstract class for a GA4 client, where the actual HTTP communications are
// left unimplemented. This is because to provide non-blocking HTTP communications, we have to rely
// on certain async mechanism (such as message loop), which is usually chosen by the embedding app.
// To use this class, the embedding app only needs to implement the SendData() method.
class Client {
 public:
  explicit Client(size_t batch_size = kMeasurementEventMaxCount);
  Client(const Client&) = delete;

  virtual ~Client();

  // Set the query parameters needed by the GA4 Measurement Protocol.
  // We do not escape or validate the parameters. The tool analytics implementer is responsible
  // for the correctness of the parameters.
  void SetQueryParameters(std::string_view measurement_id, std::string_view key);

  void SetClientId(std::string client_id);
  // Add user property shared by all metrics.
  void SetUserProperty(std::string name, Value value);

  void AddEvent(std::unique_ptr<Event> event_ptr);
  void AddEvents(std::vector<std::unique_ptr<Event>> event_ptrs,
                 size_t batch_size = kMeasurementEventMaxCount);

  // Instead of sending an event immediately, add the event to a local
  // buffer. When the number of events in the buffer reaches the batch
  // size limit, the client will send all the events in batch and then
  // clear the buffer.
  void AddEventToDefaultBatch(std::unique_ptr<Event> event_ptr);

  // Send all the events in the local buffer/batch, regardless of
  // the number of events.
  void SendDefaultBatch();

 protected:
  auto& url() { return url_; }

 private:
  bool IsReady() const;
  virtual void SendData(std::string body) = 0;
  void AddEventsDirectly(std::vector<std::unique_ptr<Event>> event_ptrs);
  void AddEventsInLoop(std::vector<std::unique_ptr<Event>> event_ptrs, size_t batch_size);

  std::string client_id_;
  std::string url_;
  // Stores shared parameters
  std::map<std::string, Value> user_properties_;
  Batch batch_;
};

}  // namespace analytics::google_analytics_4

#endif  // SRC_LIB_ANALYTICS_CPP_GOOGLE_ANALYTICS_4_CLIENT_H_
