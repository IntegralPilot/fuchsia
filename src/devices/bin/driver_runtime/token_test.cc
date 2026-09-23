// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fdf/cpp/protocol.h>
#include <lib/fit/defer.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/eventpair.h>

#include <zxtest/zxtest.h>

#include "src/devices/bin/driver_runtime/dispatcher_coordinator.h"
#include "src/devices/bin/driver_runtime/runtime_test_case.h"
#include "src/devices/bin/driver_runtime/thread_context.h"

class TokenTest : public RuntimeTestCase {
 public:
  void SetUp() override;
  void TearDown() override;

 protected:
  fdf_testing::internal::DriverRuntimeEnv runtime_env;

  fdf::Dispatcher dispatcher_local_;
  libsync::Completion dispatcher_local_shutdown_completion_;

  fdf::Dispatcher dispatcher_remote_;
  libsync::Completion dispatcher_remote_shutdown_completion_;

  fdf::Arena arena_{nullptr};

  zx::channel token_local_, token_remote_;
};

void TokenTest::SetUp() {
  // Make sure each test starts with exactly one thread.
  driver_runtime::GetDispatcherCoordinator().Reset();
  ASSERT_EQ(ZX_OK, driver_runtime::GetDispatcherCoordinator().Start(0));

  {
    thread_context::PushDriver(CreateFakeDriver());
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    auto dispatcher = fdf::SynchronizedDispatcher::Create(
        {}, "local",
        [&](fdf_dispatcher_t* dispatcher) { dispatcher_local_shutdown_completion_.Signal(); });
    ASSERT_FALSE(dispatcher.is_error());

    dispatcher_local_ = std::move(*dispatcher);
  }

  {
    thread_context::PushDriver(CreateFakeDriver());
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    auto dispatcher = fdf::SynchronizedDispatcher::Create(
        {}, "remote",
        [&](fdf_dispatcher_t* dispatcher) { dispatcher_remote_shutdown_completion_.Signal(); });
    ASSERT_FALSE(dispatcher.is_error());

    dispatcher_remote_ = std::move(*dispatcher);
  }

  arena_ = fdf::Arena('TEST');

  ASSERT_OK(zx::channel::create(0, &token_local_, &token_remote_));
}

void TokenTest::TearDown() {
  dispatcher_remote_.ShutdownAsync();
  dispatcher_remote_shutdown_completion_.Wait();

  dispatcher_local_.ShutdownAsync();
  dispatcher_local_shutdown_completion_.Wait();
}

class ProtocolTest : public TokenTest {
 public:
  void SetUp() override;

 protected:
  // Checks that the peer of |channel| has closed by reading from it.
  void VerifyPeerClosed(fdf::Channel& channel, fdf::Dispatcher& dispatcher);

  fdf::Channel fdf_local_, fdf_remote_;
};

void ProtocolTest::SetUp() {
  TokenTest::SetUp();

  auto fdf_channels = fdf::ChannelPair::Create(0);
  ASSERT_OK(fdf_channels.status_value());
  fdf_local_ = std::move(fdf_channels->end0);
  fdf_remote_ = std::move(fdf_channels->end1);
}

void ProtocolTest::VerifyPeerClosed(fdf::Channel& channel, fdf::Dispatcher& dispatcher) {
  libsync::Completion read_completion;
  auto channel_read = std::make_unique<fdf::ChannelRead>(
      channel.get(), 0,
      [&read_completion](fdf_dispatcher_t* dispatcher, fdf::ChannelRead* channel_read,
                         zx_status_t status) {
        ASSERT_EQ(ZX_ERR_PEER_CLOSED, status);
        read_completion.Signal();
      });
  // Registering a channel read may fail if the peer is closed quickly enough.
  zx_status_t status = channel_read->Begin(dispatcher.get());
  ASSERT_TRUE((status == ZX_OK) || (status == ZX_ERR_PEER_CLOSED));
  if (status == ZX_OK) {
    read_completion.Wait();
  }
}

// Tests that trying to synchronously receive a client that has not connected returns
// ZX_ERR_NOT_FOUND
TEST_F(ProtocolTest, ReceiveWithoutConnect) {
  zx::result<fdf::Channel> res = fdf::ProtocolReceive(std::move(token_remote_));
  ASSERT_TRUE(res.is_error());
  ASSERT_EQ(res.error_value(), ZX_ERR_NOT_FOUND);
}

// Tests receiving a client connect synchronously
TEST_F(ProtocolTest, ConnectThenReceive) {
  ASSERT_OK(fdf::ProtocolConnect(std::move(token_local_), std::move(fdf_remote_)));
  ASSERT_OK(fdf::ProtocolReceive(std::move(token_remote_)));
}

struct Conn {
  zx::channel token_local;
  zx::channel token_remote;
  fdf::Channel fdf_local;
  fdf::Channel fdf_remote;

  // We will transfer |fdf_remote|, so save the handle value here to compare it when received.
  fdf_handle_t fdf_remote_handle_value;
};

// Tests requesting a protocol connection, and the token peer is dropped
// before the protocol is registered.
TEST_F(ProtocolTest, ConnectThenPeerClosed) {
  ASSERT_OK(fdf::ProtocolConnect(std::move(token_local_), std::move(fdf_remote_)));
  token_remote_.reset();
  VerifyPeerClosed(fdf_local_, dispatcher_local_);
}

// Tests the token peer closing, then the protocol connection being requested.
TEST_F(ProtocolTest, PeerClosedThenConnect) {
  token_remote_.reset();
  ASSERT_OK(fdf::ProtocolConnect(std::move(token_local_), std::move(fdf_remote_)));
  VerifyPeerClosed(fdf_local_, dispatcher_local_);
}

//
// API Errors
//

TEST_F(ProtocolTest, ConnectWrongTokenType) {
  zx::eventpair bad_token_local, bad_token_remote;
  ASSERT_OK(zx::eventpair::create(0, &bad_token_local, &bad_token_remote));

  ASSERT_EQ(ZX_ERR_BAD_HANDLE, fdf_token_transfer(bad_token_local.release(), fdf_local_.release()));
}

void NotCalledHandler(fdf_dispatcher_t* dispatcher, fdf_token_t* protocol, zx_status_t status,
                      fdf_handle_t channel) {
  ASSERT_TRUE(false);
}

TEST_F(ProtocolTest, ReceiveWrongTokenType) {
  zx::eventpair bad_token_local, bad_token_remote;
  ASSERT_OK(zx::eventpair::create(0, &bad_token_local, &bad_token_remote));

  fdf_handle_t handle;
  ASSERT_EQ(ZX_ERR_BAD_HANDLE, fdf_token_receive(bad_token_remote.release(), &handle));
}

TEST_F(ProtocolTest, ConnectBadFdfHandle) {
  fdf::Channel invalid_;
  ASSERT_EQ(ZX_ERR_BAD_HANDLE, fdf::ProtocolConnect(std::move(token_local_), std::move(invalid_)));
}

TEST_F(ProtocolTest, RegisterFailureDoesNotCallHandlerOnShutdown) {
  fdf::Dispatcher temp_dispatcher;
  libsync::Completion shutdown_completion;
  {
    thread_context::PushDriver(CreateFakeDriver());
    auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

    auto dispatcher_result = fdf::SynchronizedDispatcher::Create(
        {}, "temp_dispatcher", [&](fdf_dispatcher_t* dispatcher) { shutdown_completion.Signal(); });
    ASSERT_FALSE(dispatcher_result.is_error());
    temp_dispatcher = std::move(*dispatcher_result);
  }

  bool handler_called = false;
  struct TestToken : public fdf_token_t {
    bool* handler_called;
  };
  TestToken token;
  token.handler_called = &handler_called;
  token.handler = [](fdf_dispatcher_t* dispatcher, fdf_token_t* token, zx_status_t status,
                     fdf_handle_t handle) {
    auto self = static_cast<TestToken*>(token);
    *(self->handler_called) = true;
  };

  // Replace the token handle with one lacking ZX_RIGHT_WAIT.
  zx_handle_t reduced_handle;
  ASSERT_OK(zx_handle_replace(token_local_.release(), ZX_DEFAULT_CHANNEL_RIGHTS & ~ZX_RIGHT_WAIT,
                              &reduced_handle));

  // Registering should fail because of missing rights.
  ASSERT_EQ(ZX_ERR_ACCESS_DENIED, driver_runtime::DispatcherCoordinator::TokenRegister(
                                      reduced_handle, temp_dispatcher.get(), &token));

  // Shutdown the dispatcher.
  temp_dispatcher.ShutdownAsync();
  shutdown_completion.Wait();

  // The handler should NOT have been called.
  ASSERT_FALSE(handler_called);
}
