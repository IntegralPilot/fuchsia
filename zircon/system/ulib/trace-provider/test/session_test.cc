// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../session.h"

#include <fidl/fuchsia.tracing.provider/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/trace-provider/provider.h>
#include <lib/trace/event.h>

#include <zxtest/zxtest.h>

namespace trace {
namespace {

static constexpr size_t kBufferSize = 65535;
static const std::string kAlertName = "alert_name";
static const std::string kAlertNameMin = "a";
static const std::string kAlertNameMax = "alert_name_max";

class DummyProvider : public fidl::Server<fuchsia_tracing_provider::ProviderV2> {
 public:
  void Initialize(InitializeRequest& request, InitializeCompleter::Sync& completer) override {}
  void Start(StartRequest& request, StartCompleter::Sync& completer) override {}
  void Stop(StopCompleter::Sync& completer) override {}
  void GetKnownCategories(GetKnownCategoriesCompleter::Sync& completer) override {}
  void NotifyBufferSaved(NotifyBufferSavedRequest& request,
                         NotifyBufferSavedCompleter::Sync& completer) override {}
  void Terminate(TerminateCompleter::Sync& completer) override {}
  void Flush(FlushCompleter::Sync& completer) override { trace_engine_flush_buffer(); }
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_tracing_provider::ProviderV2> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }
};

// Tests that alerts are sent over FIDL events.
TEST(SessionTest, AlertSent) {
  async::Loop loop{&kAsyncLoopConfigNoAttachToCurrentThread};
  DummyProvider provider;
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_tracing_provider::ProviderV2>::Create();
  auto binding = fidl::BindServer(loop.dispatcher(), std::move(server_end), &provider);

  zx::vmo buffer;
  zx_status_t status = zx::vmo::create(kBufferSize, 0, &buffer);
  ASSERT_EQ(ZX_OK, status);

  std::vector<std::string> categories = {
      // Filter without wildcard
      "test_category",
      // Filter with wildcard
      "wildcard*",
      // Empty filter to make sure the wildcard matcher can handle the empty case
      ""};

  std::unique_ptr<internal::Session> session =
      internal::Session::InitializeEngine(loop.dispatcher(), TRACE_BUFFERING_MODE_CIRCULAR,
                                          std::move(buffer), categories, std::move(binding));

  // Create a client to listen for events.

  class EventHandler : public fidl::AsyncEventHandler<fuchsia_tracing_provider::ProviderV2> {
   public:
    void OnAlert(fidl::Event<fuchsia_tracing_provider::ProviderV2::OnAlert>& event) override {
      alert_name_ = event.name();
      alert_received_ = true;
    }
    // Let's check if we need to implement OnSaveBuffer.
    void OnSaveBuffer(
        fidl::Event<fuchsia_tracing_provider::ProviderV2::OnSaveBuffer>& event) override {}

    void handle_unknown_event(
        fidl::UnknownEventMetadata<fuchsia_tracing_provider::ProviderV2> metadata) override {}

    std::string alert_name_;
    bool alert_received_ = false;
  };

  EventHandler event_handler;
  fidl::Client<fuchsia_tracing_provider::ProviderV2> client(std::move(client_end),
                                                            loop.dispatcher(), &event_handler);

  // Not started yet.
  TRACE_ALERT("test_category", kAlertName.c_str());
  loop.RunUntilIdle();
  ASSERT_FALSE(event_handler.alert_received_);

  session->StartEngine(TRACE_START_CLEAR_ENTIRE_BUFFER, []() {});
  loop.RunUntilIdle();

  // Alert with enabled category.
  TRACE_ALERT("test_category", kAlertName.c_str());
  loop.RunUntilIdle();
  ASSERT_TRUE(event_handler.alert_received_);
  ASSERT_EQ(kAlertName, event_handler.alert_name_);

  // Reset
  event_handler.alert_received_ = false;
  event_handler.alert_name_.clear();

  // Alert name neither min nor max length.
  TRACE_ALERT("wildcard_category", kAlertName.c_str());
  loop.RunUntilIdle();
  ASSERT_TRUE(event_handler.alert_received_);
  ASSERT_EQ(kAlertName, event_handler.alert_name_);

  // Reset
  event_handler.alert_received_ = false;
  event_handler.alert_name_.clear();

  // Alert name of min length (1).
  TRACE_ALERT("test_category", kAlertNameMin.c_str());
  loop.RunUntilIdle();
  ASSERT_TRUE(event_handler.alert_received_);
  ASSERT_EQ(kAlertNameMin, event_handler.alert_name_);

  // Reset
  event_handler.alert_received_ = false;
  event_handler.alert_name_.clear();

  // Alert name of max length (14).
  TRACE_ALERT("wildcard_category", kAlertNameMax.c_str());
  loop.RunUntilIdle();
  ASSERT_TRUE(event_handler.alert_received_);
  ASSERT_EQ(kAlertNameMax, event_handler.alert_name_);

  // Reset
  event_handler.alert_received_ = false;
  event_handler.alert_name_.clear();

  // Alert with disabled category.
  TRACE_ALERT("other_category", kAlertName.c_str());
  loop.RunUntilIdle();
  ASSERT_FALSE(event_handler.alert_received_);

  loop.Shutdown();
}

// Tests that flush are send over fidl.
TEST(SessionTest, FlushSent) {
  async::Loop loop{&kAsyncLoopConfigNoAttachToCurrentThread};
  DummyProvider provider;
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_tracing_provider::ProviderV2>::Create();
  auto binding = fidl::BindServer(loop.dispatcher(), std::move(server_end), &provider);
  zx::vmo buffer;
  zx_status_t status = zx::vmo::create(kBufferSize, 0, &buffer);
  ASSERT_EQ(ZX_OK, status);

  std::vector<std::string> categories = {"test"};

  std::unique_ptr<internal::Session> session =
      internal::Session::InitializeEngine(loop.dispatcher(), TRACE_BUFFERING_MODE_STREAMING,
                                          std::move(buffer), categories, std::move(binding));
  // Create a client to listen for events.

  class EventHandler : public fidl::AsyncEventHandler<fuchsia_tracing_provider::ProviderV2> {
   public:
    void OnAlert(fidl::Event<fuchsia_tracing_provider::ProviderV2::OnAlert>& event) override {}
    // Let's check if we need to implement OnSaveBuffer.
    void OnSaveBuffer(
        fidl::Event<fuchsia_tracing_provider::ProviderV2::OnSaveBuffer>& event) override {
      buffer_saved_ = true;
    }

    void handle_unknown_event(
        fidl::UnknownEventMetadata<fuchsia_tracing_provider::ProviderV2> metadata) override {}

    bool buffer_saved_ = false;
  };

  EventHandler event_handler;
  fidl::Client<fuchsia_tracing_provider::ProviderV2> client(std::move(client_end),
                                                            loop.dispatcher(), &event_handler);

  loop.RunUntilIdle();

  session->StartEngine(TRACE_START_CLEAR_ENTIRE_BUFFER, []() {});
  loop.RunUntilIdle();

  TRACE_INSTANT("test", "event", TRACE_SCOPE_THREAD);

  // Send a flush
  ASSERT_TRUE(client->Flush().is_ok());
  loop.RunUntilIdle();
  // Now we should get a buffer save request.
  ASSERT_TRUE(event_handler.buffer_saved_);
  event_handler.buffer_saved_ = false;

  // But if we flush again with no additional data written
  // Send a flush
  ASSERT_TRUE(client->Flush().is_ok());
  loop.RunUntilIdle();

  // We shouldn't receive any notification
  ASSERT_FALSE(event_handler.buffer_saved_);

  loop.Shutdown();
}

}  // namespace
}  // namespace trace
