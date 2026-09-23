// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/test.transport/cpp/driver/wire.h>
#include <lib/driver/runtime/testing/cpp/sync_helpers.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fit/defer.h>
#include <lib/sync/cpp/completion.h>
#include <zircon/errors.h>

#include <memory>

#include <zxtest/zxtest.h>

#include "sdk/lib/fidl_driver/tests/transport/scoped_fake_driver.h"
#include "sdk/lib/fidl_driver/tests/transport/server_on_unbound_helper.h"

namespace {

class ExpectStringEventHandler : public fdf::WireAsyncEventHandler<test_transport::SendEventTest> {
 public:
  explicit ExpectStringEventHandler(std::string expected, libsync::Completion* received_event)
      : expected_(std::move(expected)), received_event_(received_event) {}

  void OnSendEvent(fidl::WireEvent<test_transport::SendEventTest::OnSendEvent>* event) final {
    EXPECT_EQ(event->s.get(), expected_);
    string_count_++;
    received_event_->Signal();
  }

  void on_fidl_error(fidl::UnbindInfo info) final {
    ADD_FAILURE("Unexpected error: %s", info.FormatDescription().c_str());
  }

  int string_count() const { return string_count_; }

 private:
  std::string expected_;
  int string_count_ = 0;
  libsync::Completion* received_event_;
};

TEST(DriverTransport, WireSendEvent) {
  fidl_driver_testing::ScopedFakeDriver driver;

  libsync::Completion dispatcher_shutdown;
  auto dispatcher = fdf::SynchronizedDispatcher::Create(
      fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "",
      [&](fdf_dispatcher_t* dispatcher) { dispatcher_shutdown.Signal(); });
  ASSERT_OK(dispatcher.status_value());

  auto channels = fdf::ChannelPair::Create(0);
  ASSERT_OK(channels.status_value());

  fdf::ServerEnd<test_transport::SendEventTest> server_end(std::move(channels->end0));
  fdf::ClientEnd<test_transport::SendEventTest> client_end(std::move(channels->end1));

  {
    libsync::Completion received_event;
    ExpectStringEventHandler event_handler("test", &received_event);
    fdf::WireSharedClient client(std::move(client_end), dispatcher->get(), &event_handler);

    fdf::Arena arena('ORIG');
    auto result = fdf::WireSendEvent(server_end).buffer(arena)->OnSendEvent("test");
    EXPECT_OK(result.status());

    received_event.Wait();
    EXPECT_EQ(1, event_handler.string_count());
  }

  dispatcher->ShutdownAsync();
  dispatcher_shutdown.Wait();
}

TEST(DriverTransport, WireSendEventFromBinding) {
  fidl_driver_testing::ScopedFakeDriver driver;

  libsync::Completion dispatcher_shutdown;
  auto dispatcher = fdf::SynchronizedDispatcher::Create(
      fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "",
      [&](fdf_dispatcher_t* dispatcher) { dispatcher_shutdown.Signal(); });
  ASSERT_OK(dispatcher.status_value());

  auto endpoints = fdf::CreateEndpoints<test_transport::SendEventTest>();
  ASSERT_OK(endpoints.status_value());

  class SendEventServer : public fdf::WireServer<test_transport::SendEventTest> {
  } server;
  std::optional<fdf::ServerBinding<test_transport::SendEventTest>> binding;
  {
    zx::result result = fdf::RunOnDispatcherSync(
        dispatcher->async_dispatcher(), [&, dispatcher = dispatcher->get()] {
          binding.emplace(dispatcher, std::move(endpoints->server), &server,
                          fidl::kIgnoreBindingClosure);
        });
    ASSERT_OK(result.status_value());
  }

  {
    libsync::Completion received_event;
    ExpectStringEventHandler event_handler("test", &received_event);
    fdf::WireSharedClient client(std::move(endpoints->client), dispatcher->get(), &event_handler);

    fdf::Arena arena('ORIG');
    auto result = fdf::WireSendEvent(*binding).buffer(arena)->OnSendEvent("test");
    EXPECT_OK(result.status());

    received_event.Wait();
    EXPECT_EQ(1, event_handler.string_count());
  }

  dispatcher->ShutdownAsync();
  dispatcher_shutdown.Wait();
}

}  // namespace
