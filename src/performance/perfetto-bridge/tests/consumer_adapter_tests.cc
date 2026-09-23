// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/wire/internal/transport.h>
#include <lib/fit/function.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-engine/instrumentation.h>
#include <lib/trace-provider/provider.h>
#include <lib/zx/time.h>

#include <deque>
#include <optional>

#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <perfetto/ext/base/thread_task_runner.h>
#include <perfetto/ext/tracing/core/consumer.h>
#include <perfetto/ext/tracing/core/trace_packet.h>
#include <perfetto/ext/tracing/core/tracing_service.h>
#include <perfetto/tracing/platform.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/performance/perfetto-bridge/consumer_adapter.h"

#include <third_party/perfetto/protos/perfetto/common/trace_stats.gen.h>
#include <third_party/perfetto/protos/perfetto/config/chrome/chrome_config.gen.h>
#include <third_party/perfetto/protos/perfetto/config/data_source_config.gen.h>
#include <third_party/perfetto/protos/perfetto/config/trace_config.gen.h>
#include <third_party/perfetto/protos/perfetto/config/track_event/track_event_config.gen.h>

using testing::ElementsAre;

namespace {

constexpr size_t kTestBufSize = 100;
constexpr size_t kTestBufUtilizationThreshold =
    static_cast<size_t>(kTestBufSize * ConsumerAdapter::kConsumerUtilizationReadThreshold);

// Adapts Perfetto's TaskRunner interface to use a Zircon dispatcher.
class TestPerfettoTaskRunner : public perfetto::base::TaskRunner {
 public:
  explicit TestPerfettoTaskRunner(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {
    FX_DCHECK(dispatcher_);
  }
  ~TestPerfettoTaskRunner() override = default;

  TestPerfettoTaskRunner(TestPerfettoTaskRunner&) = delete;
  void operator=(TestPerfettoTaskRunner&) = delete;

  // TaskRunner implementation.
  void PostTask(std::function<void()> closure) override {
    FX_CHECK(async::PostTask(dispatcher_, std::move(closure)) == ZX_OK);
  }
  void PostDelayedTask(std::function<void()> closure, uint32_t delay_ms) override {
    FX_CHECK(async::PostDelayedTask(dispatcher_, closure, zx::msec(delay_ms)) == ZX_OK);
  }
  void AddFileDescriptorWatch(perfetto::base::PlatformHandle, std::function<void()>) override {}
  void RemoveFileDescriptorWatch(perfetto::base::PlatformHandle) override {}
  bool RunsTasksOnCurrentThread() const override { return true; }

 private:
  async_dispatcher_t* dispatcher_;
};

// Simulates interactions with Perfetto's ConsumerEndpoint.
// Behavior is GMock-like, except that expectations can be set as needed and not
// necessarily up front, and there is additional logic for capturing the TraceConfig.
class FakeConsumerEndpoint : public perfetto::ConsumerEndpoint {
 public:
  enum class CallType {
    ENABLE_TRACING,
    START_TRACING,
    DISABLE_TRACING,
    FLUSH,
    READ_BUFFERS,
    GET_TRACE_STATS,
  };

  // |on_destroy_cb| is invoked on destruction.
  explicit FakeConsumerEndpoint(fit::closure on_destroy_cb)
      : on_destroy_cb_(std::move(on_destroy_cb)) {
    // The endpoint is immediately invoked with ENABLE_TRACING.
    ExpectCall(CallType::ENABLE_TRACING);
  }
  ~FakeConsumerEndpoint() override {
    FX_CHECK(expected_calls_.empty());
    on_destroy_cb_();
  }

  // Trace configuration, stored from EnableTracing.
  const perfetto::TraceConfig& config() { return config_; }

  // Flush completion callback.
  const std::function<void(bool)>& flush_cb() { return flush_cb_; }

  // Registers an expectation for a call to ConsumerEndpoint.
  void ExpectCall(CallType type) { expected_calls_.push_back(type); }

 private:
  void RecordObservedCall(CallType type) {
    FX_CHECK(!expected_calls_.empty() && expected_calls_.front() == type)
        << "Unexpected call: " << static_cast<int>(type);
    expected_calls_.pop_front();
  }

  // perfetto::ConsumerEndpoint implementation.
  void EnableTracing(const perfetto::TraceConfig& config, perfetto::base::ScopedFile) override {
    config_ = config;
    RecordObservedCall(CallType::ENABLE_TRACING);
  }
  void StartTracing() override { RecordObservedCall(CallType::START_TRACING); }
  void DisableTracing() override { RecordObservedCall(CallType::DISABLE_TRACING); }
  void Flush(uint32_t timeout_ms, FlushCallback callback, perfetto::FlushFlags flags) override {
    flush_cb_ = callback;
    RecordObservedCall(CallType::FLUSH);
  }
  void ReadBuffers() override { RecordObservedCall(CallType::READ_BUFFERS); }
  void GetTraceStats() override { RecordObservedCall(CallType::GET_TRACE_STATS); }

  // Unused methods from perfetto::ConsumerEndpoint.
  void ChangeTraceConfig(const perfetto::TraceConfig&) override { FX_NOTREACHED(); }
  void FreeBuffers() override { FX_NOTREACHED(); }
  void Detach(const std::string& key) override { FX_NOTREACHED(); }
  void Attach(const std::string& key) override { FX_NOTREACHED(); }
  void ObserveEvents(uint32_t events_mask) override { FX_NOTREACHED(); }
  void QueryServiceState(QueryServiceStateArgs, QueryServiceStateCallback) override {
    FX_NOTREACHED();
  }
  void QueryCapabilities(QueryCapabilitiesCallback) override { FX_NOTREACHED(); }
  void SaveTraceForBugreport(SaveTraceForBugreportCallback) override { FX_NOTREACHED(); }
  void CloneSession(CloneSessionArgs) override { FX_NOTREACHED(); }

  perfetto::TraceConfig config_;
  std::deque<CallType> expected_calls_;
  std::function<void(bool)> flush_cb_;
  fit::closure on_destroy_cb_;
};

// Simulates interactions with the Fuchsia trace controller/observer
// and ensures that contexts are managed properly.
class FakeFuchsiaTracing : public FuchsiaTracing {
 public:
  FakeFuchsiaTracing() = default;
  ~FakeFuchsiaTracing() override = default;

  FakeFuchsiaTracing(FakeFuchsiaTracing&) = delete;
  void operator=(FakeFuchsiaTracing&) = delete;

  void ChangeTraceState(trace_state_t state) {
    FX_CHECK(observe_cb_);
    observe_cb_(state);
  }

  trace::ProviderConfig* provider_config() { return &provider_config_; }
  const trace::GetKnownCategoriesCallback& get_known_categories_callback() {
    return get_known_categories_callback_;
  }

  const std::vector<std::string>& GetReceivedBlobs() { return blobs_; }

 private:
  // FuchsiaTracing implementation.
  void StartObserving(fit::function<void(trace_state_t)> observe_cb) override {
    FX_CHECK(observe_cb);
    FX_CHECK(!observe_cb_);
    observe_cb_ = std::move(observe_cb);
  }
  void AcquireProlongedContext() override {
    FX_CHECK(!has_prolonged_context_);
    has_prolonged_context_ = true;
  }
  void ReleaseProlongedContext() override {
    FX_CHECK(has_prolonged_context_);
    has_prolonged_context_ = false;
  }
  void AcquireWriteContext() override {
    FX_CHECK(!has_write_context_);
    has_write_context_ = true;
  }
  bool HasWriteContext() override { return has_write_context_; }
  void WriteBlob(const char* data, size_t size) override {
    FX_CHECK(HasWriteContext());
    blobs_.emplace_back(data, size);
  }
  void ReleaseWriteContext() override { has_write_context_ = false; }
  trace::ProviderConfig GetProviderConfig() override { return provider_config_; }
  void SetGetKnownCategoriesCallback(trace::GetKnownCategoriesCallback callback) override {
    get_known_categories_callback_ = std::move(callback);
  }

  fit::function<void(trace_state_t)> observe_cb_;
  bool has_write_context_ = false;
  bool has_prolonged_context_ = false;
  std::vector<std::string> blobs_;
  trace::ProviderConfig provider_config_;
  trace::GetKnownCategoriesCallback get_known_categories_callback_;
};

class ConsumerAdapterTest : public gtest::TestLoopFixture {
 public:
  ConsumerAdapterTest() : perfetto_task_runner_(dispatcher()) {
    auto fuchsia_tracing_owned = std::make_unique<FakeFuchsiaTracing>();
    fuchsia_tracing_ = fuchsia_tracing_owned.get();
    consumer_adapter_.emplace(
        [this](perfetto::Consumer* consumer) { return ConnectConsumer(consumer); },
        std::move(fuchsia_tracing_owned), &perfetto_task_runner_);
  }
  ~ConsumerAdapterTest() override {
    consumer_adapter_.reset();
    RunLoopUntilIdle();
  }

  void WaitForGetStats() { RunLoopFor(zx::sec(1)); }

  std::unique_ptr<perfetto::ConsumerEndpoint> ConnectConsumer(perfetto::Consumer* consumer) {
    zx_status_t status = async::PostTask(dispatcher(), [this, consumer]() {
      FX_DCHECK(consumer);
      consumer_ = consumer;
    });
    FX_DCHECK(status == ZX_OK) << "Error calling PostTask: " << zx_status_get_string(status);

    std::unique_ptr mock_endpoint =
        std::make_unique<FakeConsumerEndpoint>([this]() { mock_endpoint_ = nullptr; });
    mock_endpoint_ = reinterpret_cast<FakeConsumerEndpoint*>(mock_endpoint.get());
    return mock_endpoint;
  }

  FakeFuchsiaTracing* fuchsia_tracing_;
  TestPerfettoTaskRunner perfetto_task_runner_;
  FakeConsumerEndpoint* mock_endpoint_ = nullptr;
  perfetto::Consumer* consumer_ = nullptr;
  std::optional<ConsumerAdapter> consumer_adapter_;
};

perfetto::TraceStats MakeTraceStats(size_t buffer_size, size_t bytes_written, size_t bytes_read,
                                    size_t bytes_overwritten) {
  perfetto::TraceStats stats;
  auto* buffer_stats = stats.add_buffer_stats();

  buffer_stats->set_buffer_size(buffer_size);
  buffer_stats->set_bytes_written(bytes_written);
  buffer_stats->set_bytes_read(bytes_read);
  buffer_stats->set_bytes_overwritten(bytes_overwritten);
  return stats;
}

struct ExpectedDataSource {
  std::string name;
  std::optional<std::string> provider_filter;
  std::vector<std::string> enabled_categories;
};

TEST_F(ConsumerAdapterTest, TraceConfig) {
  ASSERT_FALSE(mock_endpoint_);
  fuchsia_tracing_->provider_config()->categories = {"foo*", "bar", "provider1/category1",
                                                     "provider1/category2", "provider2/categoryX"};
  fuchsia_tracing_->ChangeTraceState(TRACE_STARTED);
  RunLoopUntilIdle();
  ASSERT_TRUE(mock_endpoint_);

  const std::vector<ExpectedDataSource> kExpectedDataSources = {
      {.name = "track_event", .enabled_categories = {"foo*", "bar"}},
      {.name = "track_event", .provider_filter = "provider2", .enabled_categories = {"categoryX"}},
      {.name = "track_event",
       .provider_filter = "provider1",
       .enabled_categories = {"category1", "category2"}},
  };

  const auto& actual_data_sources = mock_endpoint_->config().data_sources();
  ASSERT_EQ(kExpectedDataSources.size(), actual_data_sources.size());

  for (size_t i = 0; i < kExpectedDataSources.size(); ++i) {
    const auto& expected_data_source = kExpectedDataSources[i];
    const auto& actual_data_source = actual_data_sources[i];

    ASSERT_EQ(expected_data_source.name, actual_data_source.config().name());
    if (expected_data_source.provider_filter) {
      EXPECT_EQ(std::vector{expected_data_source.provider_filter.value()},
                actual_data_source.producer_name_filter());
    }

    ASSERT_EQ(expected_data_source.name, "track_event");
    perfetto::protos::gen::TrackEventConfig track_event_config;
    ASSERT_TRUE(
        track_event_config.ParseFromString(actual_data_source.config().track_event_config_raw()));
    EXPECT_EQ(expected_data_source.enabled_categories, track_event_config.enabled_categories());
    EXPECT_THAT(track_event_config.disabled_categories(), ElementsAre("*"));
  }
}

TEST_F(ConsumerAdapterTest, BufferUtilizationTriggersRead) {
  ASSERT_FALSE(mock_endpoint_);
  fuchsia_tracing_->ChangeTraceState(TRACE_STARTED);
  RunLoopUntilIdle();
  ASSERT_TRUE(mock_endpoint_);

  // 0% utilization - no flush.
  mock_endpoint_->ExpectCall(FakeConsumerEndpoint::CallType::GET_TRACE_STATS);
  WaitForGetStats();
  RunLoopUntilIdle();
  consumer_->OnTraceStats(true, MakeTraceStats(kTestBufSize, 0, 0, 0));
  RunLoopUntilIdle();

  // Just below the read threshold - no flush.
  mock_endpoint_->ExpectCall(FakeConsumerEndpoint::CallType::GET_TRACE_STATS);
  WaitForGetStats();
  RunLoopUntilIdle();
  consumer_->OnTraceStats(true,
                          MakeTraceStats(kTestBufSize, kTestBufUtilizationThreshold - 1, 0, 0));

  // We've hit the read threshold - flush it!
  mock_endpoint_->ExpectCall(FakeConsumerEndpoint::CallType::GET_TRACE_STATS);
  WaitForGetStats();
  mock_endpoint_->ExpectCall(FakeConsumerEndpoint::CallType::READ_BUFFERS);
  consumer_->OnTraceStats(true, MakeTraceStats(kTestBufSize, kTestBufUtilizationThreshold, 0, 0));
  RunLoopUntilIdle();
}

// Strips the two byte prelude header from a small serialized proto string
// so that tests can focus only on the payload contents.
std::string StripProtoPrelude(const std::string& input) { return input.substr(2); }

TEST_F(ConsumerAdapterTest, PollStatsAndReadBuffers) {
  ASSERT_FALSE(mock_endpoint_);
  fuchsia_tracing_->ChangeTraceState(TRACE_STARTED);
  RunLoopUntilIdle();
  ASSERT_TRUE(mock_endpoint_);

  // Simulate that we're hit the utilization threshold to trigger a read.
  mock_endpoint_->ExpectCall(FakeConsumerEndpoint::CallType::GET_TRACE_STATS);
  mock_endpoint_->ExpectCall(FakeConsumerEndpoint::CallType::READ_BUFFERS);
  WaitForGetStats();
  consumer_->OnTraceStats(true, MakeTraceStats(kTestBufSize, kTestBufUtilizationThreshold, 0, 0));
  RunLoopUntilIdle();

  // Simulate multiple packets * multiple numbers of slices * multiple calls
  // to OnTraceData.
  std::string call1_packet1_slice1 = "c1p1s1";
  std::string call1_packet1_slice2 = "c1p1s2";
  std::string call1_packet2_slice = "c1p2s1";
  std::string call2_packet_slice = "c2p1s1";
  perfetto::TracePacket call1_packet1, call1_packet2, call2_packet;
  call1_packet1.AddSlice(call1_packet1_slice1.data(), call1_packet1_slice1.size());
  call1_packet1.AddSlice(call1_packet1_slice2.data(), call1_packet1_slice2.size());
  call1_packet2.AddSlice(call1_packet2_slice.data(), call1_packet2_slice.size());
  call2_packet.AddSlice(call2_packet_slice.data(), call2_packet_slice.size());
  std::vector<perfetto::TracePacket> packets;
  packets.emplace_back(std::move(call1_packet1));
  packets.emplace_back(std::move(call1_packet2));
  consumer_->OnTraceData(std::move(packets), true);
  packets.clear();

  packets.emplace_back(std::move(call2_packet));
  consumer_->OnTraceData(std::move(packets), false);
  RunLoopUntilIdle();

  EXPECT_EQ(fuchsia_tracing_->GetReceivedBlobs().size(), 3u);
  EXPECT_EQ(StripProtoPrelude(fuchsia_tracing_->GetReceivedBlobs()[0]), "c1p1s1c1p1s2");
  EXPECT_EQ(StripProtoPrelude(fuchsia_tracing_->GetReceivedBlobs()[1]), "c1p2s1");
  EXPECT_EQ(StripProtoPrelude(fuchsia_tracing_->GetReceivedBlobs()[2]), "c2p1s1");

  // Back to polling.
  mock_endpoint_->ExpectCall(FakeConsumerEndpoint::CallType::GET_TRACE_STATS);
  WaitForGetStats();
}

TEST_F(ConsumerAdapterTest, TeardownTasks) {
  ASSERT_FALSE(mock_endpoint_);
  fuchsia_tracing_->ChangeTraceState(TRACE_STARTED);
  RunLoopUntilIdle();
  ASSERT_TRUE(mock_endpoint_);

  mock_endpoint_->ExpectCall(FakeConsumerEndpoint::CallType::FLUSH);
  fuchsia_tracing_->ChangeTraceState(TRACE_STOPPING);
  RunLoopUntilIdle();

  mock_endpoint_->ExpectCall(FakeConsumerEndpoint::CallType::DISABLE_TRACING);
  mock_endpoint_->flush_cb()(true);
  RunLoopUntilIdle();

  mock_endpoint_->ExpectCall(FakeConsumerEndpoint::CallType::READ_BUFFERS);
  consumer_->OnTracingDisabled({});
  RunLoopUntilIdle();

  mock_endpoint_->ExpectCall(FakeConsumerEndpoint::CallType::GET_TRACE_STATS);
  consumer_->OnTraceData({}, false);
  RunLoopUntilIdle();

  consumer_->OnTraceStats(true,
                          MakeTraceStats(kTestBufSize, kTestBufUtilizationThreshold - 1, 0, 0));
  RunLoopUntilIdle();

  EXPECT_EQ(mock_endpoint_, nullptr);
}

}  // namespace
