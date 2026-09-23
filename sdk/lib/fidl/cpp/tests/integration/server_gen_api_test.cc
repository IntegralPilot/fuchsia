// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/test.basic.protocol/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/fidl/cpp/wire/server.h>
#include <zircon/fidl.h>
#include <zircon/syscalls.h>

#include <thread>
#include <vector>

#include <gtest/gtest.h>
#include <sanitizer/lsan_interface.h>

#include "src/lib/testing/predicates/status.h"

namespace {

using ::test_basic_protocol::ValueEcho;
using ::test_basic_protocol::ValueEvent;

constexpr char kExpectedReply[] = "test";

TEST(Server, SyncReply) {
  struct SyncServer : fidl::Server<ValueEcho> {
    void Echo(EchoRequest& request, EchoCompleter::Sync& completer) override {
      EXPECT_TRUE(completer.is_reply_needed());
      completer.Reply(request.s());
      EXPECT_FALSE(completer.is_reply_needed());
    }
  };

  auto server = std::make_unique<SyncServer>();
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  auto endpoints = fidl::Endpoints<ValueEcho>::Create();

  fidl::BindServer(loop.dispatcher(), std::move(endpoints.server), server.get());

  std::thread call([&] {
    fidl::WireResult result = fidl::WireCall(endpoints.client)->Echo(kExpectedReply);
    EXPECT_OK(result.status());
    EXPECT_EQ(result.value().s.get(), kExpectedReply);
    loop.Quit();
  });
  EXPECT_STATUS(ZX_ERR_CANCELED, loop.Run());
  call.join();
}

TEST(Server, AsyncReply) {
  struct AsyncServer : fidl::Server<ValueEcho> {
    void Echo(EchoRequest& request, EchoCompleter::Sync& completer) override {
      worker_loop_ = std::make_unique<async::Loop>(&kAsyncLoopConfigNeverAttachToThread);
      async::PostTask(worker_loop_->dispatcher(),
                      [request = request.s(), completer = completer.ToAsync()]() mutable {
                        EXPECT_TRUE(completer.is_reply_needed());
                        completer.Reply(request);
                        EXPECT_FALSE(completer.is_reply_needed());
                      });
      EXPECT_FALSE(completer.is_reply_needed());
      ASSERT_OK(worker_loop_->StartThread());
    }
    std::unique_ptr<async::Loop> worker_loop_;
  };

  auto server = std::make_unique<AsyncServer>();
  async::Loop main_loop(&kAsyncLoopConfigNeverAttachToThread);
  auto endpoints = fidl::Endpoints<ValueEcho>::Create();

  fidl::BindServer(main_loop.dispatcher(), std::move(endpoints.server), server.get());

  std::thread call([&] {
    auto result = fidl::WireCall(endpoints.client)->Echo(kExpectedReply);
    EXPECT_OK(result.status());
    EXPECT_EQ(result.value().s.get(), kExpectedReply);
    main_loop.Quit();
  });
  EXPECT_STATUS(ZX_ERR_CANCELED, main_loop.Run());
  call.join();
}

TEST(Server, EventSendingAfterPeerClosed) {
  struct Server : fidl::Server<ValueEvent> {};
  Server server;
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  auto endpoints = fidl::Endpoints<ValueEvent>::Create();
  fidl::ServerBinding<ValueEvent> binding{loop.dispatcher(), std::move(endpoints.server), &server,
                                          fidl::kIgnoreBindingClosure};

  // Close the peer endpoint.
  endpoints.client.reset();

  {
    fit::result<fidl::OneWayError> result =
        fidl::SendEvent(binding)->OnValueEvent({{.s = "hello"}});
    EXPECT_TRUE(result.is_ok());
  }

  ASSERT_OK(loop.RunUntilIdle());

  {
    fit::result<fidl::OneWayError> result =
        fidl::SendEvent(binding)->OnValueEvent({{.s = "hello"}});
    EXPECT_TRUE(result.is_ok());
  }
}

TEST(Server, EventSendingAfterUnrelatedError) {
  struct Server : fidl::Server<ValueEvent> {};
  Server server;
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  auto endpoints = fidl::Endpoints<ValueEvent>::Create();
  fidl::ServerBinding<ValueEvent> binding{loop.dispatcher(), std::move(endpoints.server), &server,
                                          fidl::kIgnoreBindingClosure};

  // Send a 1 byte message which will cause the server binding to teardown.
  uint8_t byte = 0;
  endpoints.client.channel().write(0, &byte, sizeof(byte), nullptr, 0);

  {
    fit::result<fidl::OneWayError> result =
        fidl::SendEvent(binding)->OnValueEvent({{.s = "hello"}});
    EXPECT_TRUE(result.is_ok());
  }

  ASSERT_OK(loop.RunUntilIdle());

  {
    fit::result<fidl::OneWayError> result =
        fidl::SendEvent(binding)->OnValueEvent({{.s = "hello"}});
    ASSERT_FALSE(result.is_ok());
    EXPECT_EQ(result.error_value().reason(), fidl::Reason::kCanceledDueToOtherError);
    EXPECT_EQ(result.error_value().underlying_reason(), fidl::Reason::kUnexpectedMessage);
  }
}

TEST(Server, EventSendingOnMovedFromBindingRef) {
  struct Server : fidl::Server<ValueEvent> {};
  Server server;
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  zx::result endpoints = fidl::CreateEndpoints<ValueEvent>();
  ASSERT_OK(endpoints.status_value());

  fidl::ServerBindingRef<ValueEvent> binding_ref =
      fidl::BindServer(loop.dispatcher(), std::move(endpoints->server), &server);
  fidl::ServerBindingRef<ValueEvent> binding_ref_new = std::move(binding_ref);

  auto send_event_on_moved_from_ref = [&] {
#if __has_feature(address_sanitizer) || __has_feature(leak_sanitizer) || \
    __has_feature(hwaddress_sanitizer)
    // Disable LSAN for this thread while in scope. It is expected to leak by way
    // of a crash.
    __lsan::ScopedDisabler _;
#endif
    (void)fidl::SendEvent(binding_ref)->OnValueEvent({{.s = "hello"}});
  };
  ASSERT_DEATH(send_event_on_moved_from_ref(), "");
}

}  // namespace
