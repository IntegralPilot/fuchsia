// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/test.basic.protocol/cpp/wire.h>
#include <fidl/test.basic.protocol/cpp/wire_test_base.h>

#include <type_traits>

#include <zxtest/zxtest.h>

namespace test = test_basic_protocol;

namespace {

TEST(SyncEventHandler, TestBase) {
  auto endpoints = fidl::Endpoints<test::TwoEvents>::Create();
  ASSERT_OK(fidl::WireSendEvent(endpoints.server)->EventA());

  struct EventHandler : public fidl::testing::WireSyncEventHandlerTestBase<test::TwoEvents> {
    void NotImplemented_(const std::string& name) override {
      EXPECT_EQ(std::string_view{"EventA"}, name);
      called = true;
    }
    bool called = false;
  };
  EventHandler event_handler;
  ASSERT_OK(event_handler.HandleOneEvent(endpoints.client));
  EXPECT_TRUE(event_handler.called);
}

TEST(SyncEventHandler, WaitOneFails) {
  auto endpoints = fidl::Endpoints<test::TwoEvents>::Create();
  ASSERT_OK(fidl::WireSendEvent(endpoints.server)->EventA());

  struct EventHandler : public fidl::testing::WireSyncEventHandlerTestBase<test::TwoEvents> {
    void NotImplemented_(const std::string& name) override { __builtin_unreachable(); }
  };
  EventHandler event_handler;
  zx::channel bad_endpoint;
  endpoints.client.channel().replace(ZX_RIGHT_NONE, &bad_endpoint);
  fidl::ClientEnd<test::TwoEvents> client_end(std::move(bad_endpoint));
  auto result = event_handler.HandleOneEvent(client_end.borrow());
  ASSERT_EQ(ZX_ERR_ACCESS_DENIED, result.status());
  ASSERT_EQ(fidl::Reason::kTransportError, result.reason());
}

TEST(SyncEventHandler, BufferTooSmall) {
  auto endpoints = fidl::Endpoints<test::TwoEvents>::Create();
  uint8_t bogus_message[ZX_CHANNEL_MAX_MSG_BYTES] = {};
  ASSERT_OK(endpoints.server.channel().write(0, bogus_message, sizeof(bogus_message), nullptr, 0));

  struct EventHandler : public fidl::testing::WireSyncEventHandlerTestBase<test::TwoEvents> {
    void NotImplemented_(const std::string& name) override { __builtin_unreachable(); }
  };

  EventHandler event_handler;
  auto result = event_handler.HandleOneEvent(endpoints.client);
  ASSERT_EQ(ZX_ERR_BUFFER_TOO_SMALL, result.status());
  ASSERT_EQ(fidl::Reason::kUnexpectedMessage, result.reason());
}

TEST(SyncEventHandler, ExhaustivenessRequired) {
  class EventHandlerNone : public fidl::WireSyncEventHandler<test::TwoEvents> {};
  class EventHandlerA : public fidl::WireSyncEventHandler<test::TwoEvents> {
    void EventA() override {}
  };
  class EventHandlerB : public fidl::WireSyncEventHandler<test::TwoEvents> {
    void EventB() override {}
  };
  class EventHandlerAll : public fidl::WireSyncEventHandler<test::TwoEvents> {
    void EventA() override {}
    void EventB() override {}
  };
  static_assert(std::is_abstract_v<EventHandlerNone>);
  static_assert(std::is_abstract_v<EventHandlerA>);
  static_assert(std::is_abstract_v<EventHandlerB>);
  static_assert(!std::is_abstract_v<EventHandlerAll>);
}

TEST(SyncEventHandler, HandleEvent) {
  auto endpoints = fidl::Endpoints<test::TwoEvents>::Create();
  ASSERT_OK(fidl::WireSendEvent(endpoints.server)->EventA());

  struct EventHandlerAll : public fidl::WireSyncEventHandler<test::TwoEvents> {
    void EventA() override { count += 1; }
    void EventB() override { ADD_FAILURE("Should not get EventB"); }
    int count = 0;
  };

  EventHandlerAll event_handler;
  ASSERT_OK(event_handler.HandleOneEvent(endpoints.client));
  EXPECT_EQ(1, event_handler.count);
}

TEST(SyncEventHandler, UnknownEvent) {
  auto endpoints = fidl::Endpoints<test::TwoEvents>::Create();
  constexpr uint64_t kUnknownOrdinal = 0x1234abcd1234abcdULL;
  static_assert(kUnknownOrdinal != test::TwoEvents::EventA::kOrdinal);
  static_assert(kUnknownOrdinal != test::TwoEvents::EventB::kOrdinal);
  fidl_message_header_t unknown_message = {
      .txid = 1,
      .at_rest_flags = {0, 0},
      .dynamic_flags = 0,
      .magic_number = kFidlWireFormatMagicNumberInitial,
      .ordinal = kUnknownOrdinal,
  };
  ASSERT_OK(
      endpoints.server.channel().write(0, &unknown_message, sizeof(unknown_message), nullptr, 0));

  class EventHandlerAll : public fidl::WireSyncEventHandler<test::TwoEvents> {
    void EventA() override { ADD_FAILURE("Should not get EventA"); }
    void EventB() override { ADD_FAILURE("Should not get EventB"); }
  };
  EventHandlerAll event_handler;
  fidl::Status status = event_handler.HandleOneEvent(endpoints.client);
  EXPECT_EQ(fidl::Reason::kUnexpectedMessage, status.reason());
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, status.status());
  EXPECT_SUBSTR(status.FormatDescription().c_str(), "unknown ordinal");
}

TEST(SyncEventHandler, Epitaph) {
  auto endpoints = fidl::Endpoints<test::TwoEvents>::Create();
  endpoints.server.Close(ZX_ERR_WRONG_TYPE);

  struct EventHandler : public fidl::testing::WireSyncEventHandlerTestBase<test::TwoEvents> {
    void NotImplemented_(const std::string&) override {}
  } event_handler;
  fidl::Status status = event_handler.HandleOneEvent(endpoints.client);
  EXPECT_EQ(ZX_ERR_WRONG_TYPE, status.status());
  EXPECT_EQ(fidl::Reason::kPeerClosedWhileReading, status.reason());
}

}  // namespace
