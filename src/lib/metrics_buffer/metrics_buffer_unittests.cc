// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.metrics/cpp/markers.h>
#include <fidl/fuchsia.metrics/cpp/wire.h>
#include <fuchsia/metrics/cpp/fidl.h>
#include <lib/async-loop/default.h>
#include <lib/async-loop/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/sys/cpp/outgoing_directory.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <lib/zx/time.h>
#include <zircon/types.h>

#include <cstdint>
#include <memory>
#include <vector>

#include <gtest/gtest.h>

#include "lib/sync/completion.h"
#include "lib/sys/cpp/service_directory.h"
#include "log.h"
#include "src/lib/metrics_buffer/metrics_buffer.h"

namespace {

// project_id, metric_id, bucket_index, count
using Histograms =
    std::unordered_map<uint32_t,
                       std::unordered_map<uint32_t, std::unordered_map<uint32_t, uint32_t>>>;

class ServerAndClient {
 public:
  ServerAndClient() : loop_(&kAsyncLoopConfigNeverAttachToThread), dispatcher_(loop_.dispatcher()) {
    // Server end
    zx_status_t status = loop_.StartThread("MetricsBufferTest");
    ZX_ASSERT(status == ZX_OK);

    auto [aux_svc_client, aux_svc_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    sync_completion_t server_create_done;
    status = async::PostTask(dispatcher_, [this, aux_svc_server = std::move(aux_svc_server),
                                           &server_create_done]() mutable {
      outgoing_aux_service_directory_parent_.emplace();
      zx_status_t status =
          outgoing_aux_service_directory_parent_
              ->AddPublicService<fuchsia::metrics::MetricEventLoggerFactory>(
                  [this](
                      fidl::InterfaceRequest<fuchsia::metrics::MetricEventLoggerFactory> request) {
                    fidl::ServerEnd<fuchsia_metrics::MetricEventLoggerFactory> llcpp_server_end(
                        request.TakeChannel());
                    auto logger_factory = std::make_unique<MetricEventLoggerFactory>(this);
                    // Ignore the result - logger_factory will be destroyed and the channel will be
                    // closed on error.
                    fidl::BindServer(loop_.dispatcher(), std::move(llcpp_server_end),
                                     std::move(logger_factory));
                  });
      outgoing_aux_service_directory_ =
          outgoing_aux_service_directory_parent_->GetOrCreateDirectory("svc");
      ZX_ASSERT(outgoing_aux_service_directory_);
      status = outgoing_aux_service_directory_->Serve(fuchsia_io::wire::kPermReadable,
                                                      std::move(aux_svc_server), dispatcher_);
      ZX_ASSERT(status == ZX_OK);
      sync_completion_signal(&server_create_done);
    });
    ZX_ASSERT(status == ZX_OK);
    sync_completion_wait(&server_create_done, ZX_TIME_INFINITE);

    // Client end
    ZX_ASSERT(!aux_service_directory_);
    aux_service_directory_ = std::make_shared<sys::ServiceDirectory>(aux_svc_client.TakeChannel());
    ZX_ASSERT(aux_service_directory_);
  }

  ~ServerAndClient() {
    aux_service_directory_ = nullptr;
    zx::event server_delete_done;
    zx_status_t status = zx::event::create(0, &server_delete_done);
    ZX_ASSERT(status == ZX_OK);
    status = async::PostTask(dispatcher_, [this, &server_delete_done] {
      outgoing_aux_service_directory_parent_.reset();
      zx_status_t status = server_delete_done.signal(0, ZX_EVENT_SIGNALED);
      ZX_ASSERT(status == ZX_OK);
    });
    ZX_ASSERT(status == ZX_OK);
    zx_signals_t pending;
    status = server_delete_done.wait_one(ZX_EVENT_SIGNALED, zx::time::infinite(), &pending);
    ZX_ASSERT(status == ZX_OK);
  }

  void WaitUntilEventCountAtLeast(uint32_t count) {
    while (true) {
      uint64_t actual_count;
      {  // scope lock
        std::lock_guard<std::mutex> lock(lock_);
        actual_count = event_count_;
      }
      if (actual_count >= count) {
        return;
      }
      zx::nanosleep(zx::deadline_after(zx::msec(1)));
    }
  }

  void WaitUntilBucketUpdateCountAtLeast(uint32_t count) {
    while (true) {
      uint64_t actual_count;
      {  // scope lock
        std::lock_guard<std::mutex> lock(lock_);
        actual_count = bucket_update_count_;
      }
      if (actual_count >= count) {
        return;
      }
      zx::nanosleep(zx::deadline_after(zx::msec(1)));
    }
  }

  uint32_t logger_factory_message_count() {
    std::lock_guard<std::mutex> lock(lock_);
    return logger_factory_message_count_;
  }

  uint32_t logger_message_count() {
    std::lock_guard<std::mutex> lock(lock_);
    return logger_message_count_;
  }

  uint32_t aggregated_events_count() {
    std::lock_guard<std::mutex> lock(lock_);
    return aggregated_events_count_;
  }

  uint64_t max_count_per_aggregated_event() {
    std::lock_guard<std::mutex> lock(lock_);
    return max_count_per_aggregated_event_;
  }

  uint64_t event_count() {
    std::lock_guard<std::mutex> lock(lock_);
    return event_count_;
  }

  struct LastEvent {
    LastEvent() = default;
    LastEvent(const LastEvent& to_copy) = default;
    LastEvent& operator=(const LastEvent& to_copy) = default;
    LastEvent(LastEvent&& to_move) = default;
    LastEvent& operator=(LastEvent&& to_move) = default;
    ~LastEvent() = default;

    uint32_t project_id = 0;
    uint32_t metric_id = 0;
    std::vector<uint32_t> event_codes;
    uint64_t count = 0;
  };
  const LastEvent GetLastEvent() {
    std::lock_guard<std::mutex> lock(lock_);
    return last_event_;
  }

  struct LastString {
    LastString() = default;
    LastString(const LastString& to_copy) = default;
    LastString& operator=(const LastString& to_copy) = default;
    LastString(LastString& to_move) = default;
    LastString& operator=(LastString&& to_move) = default;

    uint32_t project_id = 0;
    uint32_t metric_id = 0;
    std::vector<uint32_t> event_codes;
    std::string string_value;
  };
  const LastString GetLastString() {
    std::lock_guard<std::mutex> lock(lock_);
    return last_string_;
  }

  const Histograms GetHistograms() {
    std::lock_guard<std::mutex> lock(lock_);
    // copy
    return histograms_;
  }

  std::vector<uint32_t> GetLastHistogramEventCodes() {
    std::lock_guard<std::mutex> lock(lock_);
    return last_histogram_event_codes_;
  }

  void IncLoggerFactoryMessageCount() {
    std::lock_guard<std::mutex> lock(lock_);
    ++logger_factory_message_count_;
  }

  void IncLoggerMessageCount() {
    std::lock_guard<std::mutex> lock(lock_);
    ++logger_message_count_;
  }

  void RecordAggregatedEvent(uint32_t project_id, uint32_t metric_id,
                             std::vector<uint32_t> event_codes, uint64_t count) {
    std::lock_guard<std::mutex> lock(lock_);
    last_event_.project_id = project_id;
    last_event_.metric_id = metric_id;
    last_event_.event_codes = std::move(event_codes);
    last_event_.count = count;
    ++aggregated_events_count_;
    max_count_per_aggregated_event_ = std::max(max_count_per_aggregated_event_, count);
    // This must go last, as the test is relying on >= release semantics here.
    event_count_ += count;
  }

  void RecordString(uint32_t project_id, uint32_t metric_id, std::vector<uint32_t> event_codes,
                    std::string string_value) {
    std::lock_guard<std::mutex> lock(lock_);
    last_string_.project_id = project_id;
    last_string_.metric_id = metric_id;
    last_string_.event_codes = std::move(event_codes);
    last_string_.string_value = std::move(string_value);
    ++aggregated_events_count_;
    event_count_ += 1;
  }

  void RecordHistogram(uint32_t project_id, uint32_t metric_id, std::vector<uint32_t> event_codes,
                       std::vector<fuchsia_metrics::wire::HistogramBucket> buckets) {
    std::lock_guard<std::mutex> lock(lock_);
    for (auto& bucket : buckets) {
      histograms_[project_id][metric_id][bucket.index] += bucket.count;
      bucket_update_count_ += 1;
    }
    last_histogram_event_codes_ = std::move(event_codes);
    event_count_ += 1;
  }

  std::shared_ptr<sys::ServiceDirectory> aux_service_directory() {
    std::lock_guard<std::mutex> lock(lock_);
    return aux_service_directory_;
  }

 protected:
  class MetricEventLoggerFactory
      : public fidl::WireServer<fuchsia_metrics::MetricEventLoggerFactory> {
   public:
    explicit MetricEventLoggerFactory(ServerAndClient* parent) : parent_(parent) {}

    void CreateMetricEventLogger(CreateMetricEventLoggerRequestView request,
                                 CreateMetricEventLoggerCompleter::Sync& completer) override {
      parent_->IncLoggerFactoryMessageCount();
      auto logger =
          std::make_unique<MetricEventLogger>(parent_, request->project_spec.project_id());
      fidl::BindServer(parent_->dispatcher_, std::move(request->logger), std::move(logger));
      completer.ReplySuccess();
    }

    void CreateMetricEventLoggerWithExperiments(
        CreateMetricEventLoggerWithExperimentsRequestView request,
        CreateMetricEventLoggerWithExperimentsCompleter::Sync& completer) override {
      ZX_ASSERT_MSG(false, "message not expected");
      completer.ReplySuccess();
    }

   private:
    ServerAndClient* parent_ = nullptr;
  };

  class MetricEventLogger : public fidl::WireServer<fuchsia_metrics::MetricEventLogger> {
   public:
    MetricEventLogger(ServerAndClient* parent, uint32_t project_id)
        : parent_(parent), project_id_(project_id) {}

    void LogOccurrence(LogOccurrenceRequestView request,
                       LogOccurrenceCompleter::Sync& completer) override {
      ZX_ASSERT_MSG(false, "message not expected");
      completer.ReplySuccess();
    }

    void LogInteger(LogIntegerRequestView request, LogIntegerCompleter::Sync& completer) override {
      ZX_ASSERT_MSG(false, "message not expected");
      completer.ReplySuccess();
    }

    void LogIntegerHistogram(LogIntegerHistogramRequestView request,
                             LogIntegerHistogramCompleter::Sync& completer) override {
      ZX_ASSERT_MSG(false, "message not expected");
      completer.ReplySuccess();
    }

    void LogString(LogStringRequestView request, LogStringCompleter::Sync& completer) override {
      ZX_ASSERT_MSG(false, "message not expected");
      completer.ReplySuccess();
    }

    void LogMetricEvents(LogMetricEventsRequestView request,
                         LogMetricEventsCompleter::Sync& completer) override {
      parent_->IncLoggerMessageCount();
      for (auto& event : request->events) {
        if (event.payload.is_count()) {
          parent_->RecordAggregatedEvent(
              project_id_, event.metric_id,
              std::vector<uint32_t>(event.event_codes.begin(), event.event_codes.end()),
              event.payload.count());
        } else if (event.payload.is_string_value()) {
          parent_->RecordString(
              project_id_, event.metric_id,
              std::vector<uint32_t>(event.event_codes.begin(), event.event_codes.end()),
              std::string(event.payload.string_value().data(),
                          event.payload.string_value().size()));
        } else if (event.payload.is_histogram()) {
          parent_->RecordHistogram(
              project_id_, event.metric_id,
              std::vector<uint32_t>(event.event_codes.begin(), event.event_codes.end()),
              std::vector<fuchsia_metrics::wire::HistogramBucket>(event.payload.histogram().begin(),
                                                                  event.payload.histogram().end()));
        } else {
          ZX_PANIC("unexpected");
        }
      }
      completer.ReplySuccess();
    }

   private:
    ServerAndClient* parent_ = nullptr;
    fuchsia_metrics::wire::ProjectSpec project_spec_;
    uint32_t project_id_ = 0;
  };

  std::mutex lock_;
  uint32_t logger_factory_message_count_ = 0;
  uint32_t logger_message_count_ = 0;
  // the "count" of delivered aggregated events doesn't matter for this counter
  uint32_t aggregated_events_count_ = 0;
  uint64_t max_count_per_aggregated_event_ = 0;
  // the "count" of all delivered aggregated events is summed here
  uint64_t event_count_ = 0;
  LastEvent last_event_;

  LastString last_string_;

  Histograms histograms_;
  std::vector<uint32_t> last_histogram_event_codes_;
  uint64_t bucket_update_count_ = 0;

  // server end
  async::Loop loop_;
  async_dispatcher_t* dispatcher_ = nullptr;
  std::optional<sys::OutgoingDirectory> outgoing_aux_service_directory_parent_;
  vfs::PseudoDir* outgoing_aux_service_directory_ = nullptr;

  // client end
  std::shared_ptr<sys::ServiceDirectory> aux_service_directory_;
};

class MetricsBufferTest : public ::testing::Test {
 protected:
  MetricsBufferTest() { ResetState(); }

  ServerAndClient& state() { return *state_; }

  std::shared_ptr<sys::ServiceDirectory> aux_service_directory() {
    return state_->aux_service_directory();
  }

  void ResetState() {
    state_.reset();
    state_.emplace();
  }

 private:
  std::optional<ServerAndClient> state_;
};

TEST_F(MetricsBufferTest, Direct) {
  auto& s = state();

  auto metrics_buffer = cobalt::MetricsBuffer::Create(42, s.aux_service_directory());
  metrics_buffer->SetMinLoggingPeriod(zx::msec(10));
  auto e = s.GetLastEvent();
  EXPECT_EQ(0u, e.project_id);

  metrics_buffer->LogEvent(12, {1, 2, 3});
  s.WaitUntilEventCountAtLeast(1);
  e = s.GetLastEvent();
  EXPECT_EQ(42u, e.project_id);
  EXPECT_EQ(12u, e.metric_id);
  EXPECT_EQ(std::vector<uint32_t>({1, 2, 3}), e.event_codes);
  EXPECT_EQ(1u, e.count);

  metrics_buffer->LogEvent(13, {3, 2, 1});
  s.WaitUntilEventCountAtLeast(2);
  e = s.GetLastEvent();
  EXPECT_EQ(42u, e.project_id);
  EXPECT_EQ(13u, e.metric_id);
  EXPECT_EQ(std::vector<uint32_t>({3, 2, 1}), e.event_codes);
  EXPECT_EQ(1u, e.count);

  zx::nanosleep(zx::deadline_after(zx::msec(10)));
  EXPECT_EQ(2u, s.event_count());
  auto last_event = s.GetLastEvent();
  EXPECT_EQ(13u, last_event.metric_id);
  EXPECT_EQ(std::vector<uint32_t>({3, 2, 1}), last_event.event_codes);
}

TEST_F(MetricsBufferTest, ViaMetricBuffer) {
  auto& s = state();

  auto metrics_buffer = cobalt::MetricsBuffer::Create(42, s.aux_service_directory());
  metrics_buffer->SetMinLoggingPeriod(zx::msec(10));
  auto metric_buffer = metrics_buffer->CreateMetricBuffer(12);

  metric_buffer.LogEvent({1u});
  s.WaitUntilEventCountAtLeast(1);
  auto e = s.GetLastEvent();
  EXPECT_EQ(42u, e.project_id);
  EXPECT_EQ(12u, e.metric_id);
  EXPECT_EQ(std::vector<uint32_t>({1u}), e.event_codes);
  EXPECT_EQ(1u, e.count);

  metric_buffer.LogEvent({2u, 1u});
  s.WaitUntilEventCountAtLeast(2);
  zx::nanosleep(zx::deadline_after(zx::msec(10)));
  EXPECT_EQ(2u, s.event_count());
  e = s.GetLastEvent();
  EXPECT_EQ(42u, e.project_id);
  EXPECT_EQ(12u, e.metric_id);
  EXPECT_EQ(std::vector<uint32_t>({2u, 1u}), e.event_codes);
  EXPECT_EQ(1u, e.count);
}

TEST_F(MetricsBufferTest, BatchingHappens) {
  uint32_t success_count = 0;
  uint32_t failure_count = 0;
  constexpr uint32_t kMaxTries = 50;
  for (uint32_t attempt = 0; attempt < kMaxTries; ++attempt) {
    // This is intentionally doing ResetState() for attempt 0 despite already having a fresh state,
    // to ensure we cover a ResetState() while we already have one.
    ResetState();
    auto& s = state();

    auto metrics_buffer = cobalt::MetricsBuffer::Create(42, s.aux_service_directory());
    metrics_buffer->SetMinLoggingPeriod(zx::msec(10));
    // The first might not batch because the first will be sent asap.
    metrics_buffer->LogEvent(12u, {1u});
    // These two may or may not batch with the first, but typically we'll see at least some batching
    // given three events.
    metrics_buffer->LogEvent(12u, {1u});
    metrics_buffer->LogEvent(12u, {1u});
    s.WaitUntilEventCountAtLeast(2);
    zx::nanosleep(zx::deadline_after(zx::msec(10)));
    if (s.logger_message_count() >= 3) {
      printf("logger_message_count >= 3\n");
      ++failure_count;
      continue;
    }
    if (s.aggregated_events_count() >= 3) {
      printf("aggregated_events_count >= 3\n");
      ++failure_count;
      continue;
    }
    int64_t max_count = s.max_count_per_aggregated_event();
    if (max_count < 2) {
      printf("max_count < 2\n");
      ++failure_count;
      continue;
    }

    ++success_count;
    // These are basically comments at this point in the code...
    EXPECT_LT(s.logger_message_count(), 3u);
    EXPECT_LT(s.aggregated_events_count(), 3u);
    auto e = s.GetLastEvent();
    EXPECT_EQ(42u, e.project_id);
    EXPECT_EQ(12u, e.metric_id);
    EXPECT_EQ(std::vector<uint32_t>({1u}), e.event_codes);
    EXPECT_GE(e.count, 1u);
    break;
  }
  printf("success: %u went around again: %u\n", success_count, failure_count);
  EXPECT_EQ(1u, success_count);
  EXPECT_LT(failure_count, 50u);
}

TEST_F(MetricsBufferTest, Strings) {
  auto& s = state();

  auto metrics_buffer = cobalt::MetricsBuffer::Create(42, s.aux_service_directory());
  metrics_buffer->SetMinLoggingPeriod(zx::msec(10));
  auto string_buffer = metrics_buffer->CreateStringMetricBuffer(12);

  string_buffer.LogString({1u}, "the_string");
  string_buffer.LogString({1u}, "other_string");

  s.WaitUntilEventCountAtLeast(2);

  auto e = s.GetLastString();
  EXPECT_EQ(42u, e.project_id);
  EXPECT_EQ(12u, e.metric_id);
  EXPECT_TRUE(e.string_value == "the_string" || e.string_value == "other_string");
}

TEST_F(MetricsBufferTest, Histograms) {
  auto& s = state();

  auto metrics_buffer = cobalt::MetricsBuffer::Create(42, s.aux_service_directory());
  metrics_buffer->SetMinLoggingPeriod(zx::msec(10));

  const uint32_t kFooMetricId = 12;
  const uint32_t kFooIntBucketsFloor = 1;
  const uint32_t kFooIntBucketsNumBuckets = 12;
  const uint32_t kFooIntBucketsInitialStep = 1;
  const uint32_t kFooIntBucketsStepMultiplierFloat = 2;
  auto histogram_buffer =
      metrics_buffer->CreateHistogramMetricBuffer(COBALT_EXPONENTIAL_HISTOGRAM_INFO(kFoo));

  histogram_buffer.LogValue({13, 14}, 7);
  histogram_buffer.LogValue({13, 14}, 15);

  s.WaitUntilBucketUpdateCountAtLeast(2);

  auto histograms = s.GetHistograms();
  auto last_histogram_event_codes = s.GetLastHistogramEventCodes();

  EXPECT_EQ(2u, histograms[42][12].size());
  // buckets: underflow bucket, [1, 2), [2, 3), [3, 5), [5, 9), [9, 17)
  EXPECT_EQ(1u, histograms[42][12][4]);
  EXPECT_EQ(1u, histograms[42][12][5]);
}

}  // namespace
