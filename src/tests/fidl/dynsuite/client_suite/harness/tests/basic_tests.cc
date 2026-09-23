// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.clientsuite/cpp/common_types.h>
#include <fidl/fidl.clientsuite/cpp/natural_types.h>

#include "src/tests/fidl/dynsuite/channel_util/bytes.h"
#include "src/tests/fidl/dynsuite/channel_util/channel.h"
#include "src/tests/fidl/dynsuite/client_suite/harness/harness.h"
#include "src/tests/fidl/dynsuite/client_suite/harness/ordinals.h"

namespace client_suite {
namespace {

using namespace ::channel_util;

// Check that the test runner is set up correctly without doing anything else.
CLIENT_TEST(1, Setup) {}

// The client should call a two-way method and receive the empty response.
CLIENT_TEST(2, TwoWayNoPayload) {
  Bytes bytes = Header{.txid = kTxidNotKnown, .ordinal = kOrdinal_ClosedTarget_TwoWayNoPayload};
  runner()->CallTwoWayNoPayload({{.target = TakeClosedClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().success().has_value());
  });
  ASSERT_OK(server_end().read_and_check_unknown_txid(bytes, &bytes.txid()));
  ASSERT_NE(bytes.txid(), 0u);
  ASSERT_OK(server_end().write(bytes));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a two-way method and receive the struct response.
CLIENT_TEST(42, TwoWayStructPayload) {
  Header header = {.txid = kTxidNotKnown, .ordinal = kOrdinal_ClosedTarget_TwoWayStructPayload};
  fidl_clientsuite::NonEmptyPayload payload = {{.some_field = 42}};
  Bytes expected_request = header;
  Bytes response = {header, encode(payload)};
  runner()
      ->CallTwoWayStructPayload({{.target = TakeClosedClient()}})
      .ThenExactlyOnce([&](auto result) {
        MarkCallbackRun();
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result.value().success().has_value());
        ASSERT_EQ(result.value().success().value(), payload);
      });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a two-way method and receive the table response.
CLIENT_TEST(43, TwoWayTablePayload) {
  Header header = {.txid = kTxidNotKnown, .ordinal = kOrdinal_ClosedTarget_TwoWayTablePayload};
  fidl_clientsuite::TablePayload payload = {{.some_field = 42}};
  Bytes expected_request = header;
  Bytes response = {header, encode(payload)};
  runner()
      ->CallTwoWayTablePayload({{.target = TakeClosedClient()}})
      .ThenExactlyOnce([&](auto result) {
        MarkCallbackRun();
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result.value().success().has_value());
        ASSERT_EQ(result.value().success().value(), payload);
      });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a two-way method and receive the union response.
CLIENT_TEST(44, TwoWayUnionPayload) {
  Header header = {.txid = kTxidNotKnown, .ordinal = kOrdinal_ClosedTarget_TwoWayUnionPayload};
  fidl_clientsuite::UnionPayload payload = fidl_clientsuite::UnionPayload::WithSomeVariant(320494);
  Bytes expected_request = header;
  Bytes response = {header, encode(payload)};
  runner()
      ->CallTwoWayUnionPayload({{.target = TakeClosedClient()}})
      .ThenExactlyOnce([&](auto result) {
        MarkCallbackRun();
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result.value().success().has_value());
        ASSERT_EQ(result.value().success().value(), payload);
      });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a fallible two-way method and receive the success response.
CLIENT_TEST(45, TwoWayResultWithPayload) {
  Header header = {.txid = kTxidNotKnown, .ordinal = kOrdinal_ClosedTarget_TwoWayStructPayloadErr};
  int32_t field = 390023;
  fidl_clientsuite::NonEmptyPayload payload = {{.some_field = field}};
  Bytes expected_request = header;
  Bytes response = {header, union_ordinal(kResultUnionSuccess), inline_envelope(int32(field))};
  runner()
      ->CallTwoWayStructPayloadErr({{.target = TakeClosedClient()}})
      .ThenExactlyOnce(
          [&](fidl::Result<fidl_clientsuite::Runner::CallTwoWayStructPayloadErr>& result) {
            MarkCallbackRun();
            ASSERT_TRUE(result.is_ok()) << result.error_value();
            ASSERT_TRUE(result.value().success().has_value());
            ASSERT_EQ(result.value().success().value(), payload);
          });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a fallible two-way method and receive the error response.
CLIENT_TEST(46, TwoWayResultWithError) {
  Header header = {.txid = kTxidNotKnown, .ordinal = kOrdinal_ClosedTarget_TwoWayStructPayloadErr};
  int32_t error = 90240;
  Bytes expected_request = header;
  Bytes response = {header, union_ordinal(kResultUnionDomainError), inline_envelope(int32(error))};
  runner()
      ->CallTwoWayStructPayloadErr({{.target = TakeClosedClient()}})
      .ThenExactlyOnce(
          [&](fidl::Result<fidl_clientsuite::Runner::CallTwoWayStructPayloadErr>& result) {
            MarkCallbackRun();
            ASSERT_TRUE(result.is_ok()) << result.error_value();
            ASSERT_TRUE(result.value().application_error().has_value());
            ASSERT_EQ(result.value().application_error().value(), error);
          });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a two-way method with a struct request and receive the response.
CLIENT_TEST(52, TwoWayStructRequest) {
  Header header = {.txid = kTxidNotKnown, .ordinal = kOrdinal_ClosedTarget_TwoWayStructRequest};
  fidl_clientsuite::NonEmptyPayload payload = {{.some_field = 390023}};
  Bytes expected_request = {header, encode(payload)};
  Bytes response = header;
  runner()
      ->CallTwoWayStructRequest({{.target = TakeClosedClient(), .request = payload}})
      .ThenExactlyOnce(
          [&](fidl::Result<fidl_clientsuite::Runner::CallTwoWayStructRequest>& result) {
            MarkCallbackRun();
            ASSERT_TRUE(result.is_ok()) << result.error_value();
            ASSERT_EQ(result.value(), fidl_clientsuite::EmptyResultClassification::WithSuccess({}));
          });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a two-way method with a table request and receive the response.
CLIENT_TEST(53, TwoWayTableRequest) {
  Header header = {.txid = kTxidNotKnown, .ordinal = kOrdinal_ClosedTarget_TwoWayTableRequest};
  fidl_clientsuite::TablePayload payload = {{.some_field = 390023}};
  Bytes expected_request = {header, encode(payload)};
  Bytes response = header;
  runner()
      ->CallTwoWayTableRequest({{.target = TakeClosedClient(), .request = payload}})
      .ThenExactlyOnce([&](auto result) {
        MarkCallbackRun();
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_EQ(result.value(), fidl_clientsuite::EmptyResultClassification::WithSuccess({}));
      });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a two-way method with a union request and receive the response.
CLIENT_TEST(54, TwoWayUnionRequest) {
  Header header = {.txid = kTxidNotKnown, .ordinal = kOrdinal_ClosedTarget_TwoWayUnionRequest};
  fidl_clientsuite::UnionPayload payload = fidl_clientsuite::UnionPayload::WithSomeVariant(390023);
  Bytes expected_request = {header, encode(payload)};
  Bytes response = header;
  runner()
      ->CallTwoWayUnionRequest({{.target = TakeClosedClient(), .request = payload}})
      .ThenExactlyOnce([&](auto result) {
        MarkCallbackRun();
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_EQ(result.value(), fidl_clientsuite::EmptyResultClassification::WithSuccess({}));
      });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a one-way method with an empty request.
CLIENT_TEST(48, OneWayNoRequest) {
  Bytes expected_request = Header{.txid = 0, .ordinal = kOrdinal_ClosedTarget_OneWayNoRequest};
  runner()->CallOneWayNoRequest({{.target = TakeClosedClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_EQ(result.value(), fidl_clientsuite::EmptyResultClassification::WithSuccess({}));
  });
  ASSERT_OK(server_end().read_and_check(expected_request));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a one-way method with a struct request.
CLIENT_TEST(49, OneWayStructRequest) {
  fidl_clientsuite::NonEmptyPayload payload = {{.some_field = 390023}};
  Bytes expected_request = {
      Header{.txid = 0, .ordinal = kOrdinal_ClosedTarget_OneWayStructRequest},
      encode(payload),
  };
  runner()
      ->CallOneWayStructRequest({{.target = TakeClosedClient(), .request = payload}})
      .ThenExactlyOnce(
          [&](fidl::Result<fidl_clientsuite::Runner::CallOneWayStructRequest>& result) {
            MarkCallbackRun();
            ASSERT_TRUE(result.is_ok()) << result.error_value();
            ASSERT_EQ(result.value(), fidl_clientsuite::EmptyResultClassification::WithSuccess({}));
          });
  ASSERT_OK(server_end().read_and_check(expected_request));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a one-way method with a table request.
CLIENT_TEST(50, OneWayTableRequest) {
  fidl_clientsuite::TablePayload payload = {{.some_field = 390023}};
  Bytes expected_request = {
      Header{.txid = 0, .ordinal = kOrdinal_ClosedTarget_OneWayTableRequest},
      encode(payload),
  };
  runner()
      ->CallOneWayTableRequest({{.target = TakeClosedClient(), .request = payload}})
      .ThenExactlyOnce([&](auto result) {
        MarkCallbackRun();
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_EQ(result.value(), fidl_clientsuite::EmptyResultClassification::WithSuccess({}));
      });
  ASSERT_OK(server_end().read_and_check(expected_request));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a one-way method with a union request.
CLIENT_TEST(51, OneWayUnionRequest) {
  fidl_clientsuite::UnionPayload payload = fidl_clientsuite::UnionPayload::WithSomeVariant(390023);
  Bytes expected_request = {
      Header{.txid = 0, .ordinal = kOrdinal_ClosedTarget_OneWayUnionRequest},
      encode(payload),
  };
  runner()
      ->CallOneWayUnionRequest({{.target = TakeClosedClient(), .request = payload}})
      .ThenExactlyOnce([&](auto result) {
        MarkCallbackRun();
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_EQ(result.value(), fidl_clientsuite::EmptyResultClassification::WithSuccess({}));
      });
  ASSERT_OK(server_end().read_and_check(expected_request));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should receive an event with no payload.
CLIENT_TEST(55, ReceiveEventNoPayload) {
  Bytes event = Header{.txid = kOneWayTxid, .ordinal = kOrdinal_ClosedTarget_OnEventNoPayload};
  auto reporter = ReceiveClosedEvents();
  ASSERT_NE(reporter, nullptr);
  ASSERT_OK(server_end().write(event));
  WAIT_UNTIL([reporter]() { return reporter->NumReceivedEvents(); });
  ASSERT_EQ(reporter->NumReceivedEvents(), 1u);
  auto reporter_event = reporter->TakeNextEvent();
  ASSERT_TRUE(reporter_event.on_event_no_payload().has_value());
}

// The client should receive an event with a struct.
CLIENT_TEST(56, ReceiveEventStructPayload) {
  fidl_clientsuite::NonEmptyPayload payload = {{.some_field = 9098607}};
  Bytes bytes_in = {
      Header{.txid = kOneWayTxid, .ordinal = kOrdinal_ClosedTarget_OnEventStructPayload},
      encode(payload),
  };
  auto reporter = ReceiveClosedEvents();
  ASSERT_NE(reporter, nullptr);
  ASSERT_OK(server_end().write(bytes_in));
  WAIT_UNTIL([reporter]() { return reporter->NumReceivedEvents(); });
  ASSERT_EQ(reporter->NumReceivedEvents(), 1u);
  auto reporter_event = reporter->TakeNextEvent();
  ASSERT_TRUE(reporter_event.on_event_struct_payload().has_value());
  EXPECT_EQ(reporter_event.on_event_struct_payload().value(), payload);
}

// The client should receive an event with a table.
CLIENT_TEST(57, ReceiveEventTablePayload) {
  fidl_clientsuite::TablePayload payload = {{.some_field = 9098607}};
  Bytes event = {
      Header{.txid = kOneWayTxid, .ordinal = kOrdinal_ClosedTarget_OnEventTablePayload},
      encode(payload),
  };
  auto reporter = ReceiveClosedEvents();
  ASSERT_NE(reporter, nullptr);
  ASSERT_OK(server_end().write(event));
  WAIT_UNTIL([reporter]() { return reporter->NumReceivedEvents(); });
  ASSERT_EQ(reporter->NumReceivedEvents(), 1u);
  auto reporter_event = reporter->TakeNextEvent();
  ASSERT_TRUE(reporter_event.on_event_table_payload().has_value());
  EXPECT_EQ(reporter_event.on_event_table_payload().value(), payload);
}

// The client should receive an event with a union.
CLIENT_TEST(58, ReceiveEventUnionPayload) {
  fidl_clientsuite::UnionPayload payload = fidl_clientsuite::UnionPayload::WithSomeVariant(87662);
  Bytes event = {
      Header{.txid = kOneWayTxid, .ordinal = kOrdinal_ClosedTarget_OnEventUnionPayload},
      encode(payload),
  };
  auto reporter = ReceiveClosedEvents();
  ASSERT_NE(reporter, nullptr);
  ASSERT_OK(server_end().write(event));
  WAIT_UNTIL([reporter]() { return reporter->NumReceivedEvents(); });
  ASSERT_EQ(reporter->NumReceivedEvents(), 1u);
  auto reporter_event = reporter->TakeNextEvent();
  ASSERT_TRUE(reporter_event.on_event_union_payload().has_value());
  EXPECT_EQ(reporter_event.on_event_union_payload().value(), payload);
}

// The client should fail to call a two-way method after the server closes the channel.
CLIENT_TEST(3, GracefulFailureDuringCallAfterPeerClose) {
  server_end().get().reset();
  runner()->CallTwoWayNoPayload({{.target = TakeClosedClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().fidl_error().has_value());
    ASSERT_EQ(result.value().fidl_error().value(),
              fidl_clientsuite::FidlErrorKind::kChannelPeerClosed);
  });
  WAIT_UNTIL_CALLBACK_RUN();
}

}  // namespace
}  // namespace client_suite
