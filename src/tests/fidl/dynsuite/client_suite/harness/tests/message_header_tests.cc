// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.clientsuite/cpp/common_types.h>
#include <fidl/fidl.clientsuite/cpp/natural_types.h>
#include <zircon/types.h>

#include "src/tests/fidl/dynsuite/channel_util/bytes.h"
#include "src/tests/fidl/dynsuite/channel_util/channel.h"
#include "src/tests/fidl/dynsuite/client_suite/harness/harness.h"
#include "src/tests/fidl/dynsuite/client_suite/harness/ordinals.h"

namespace client_suite {
namespace {

using namespace ::channel_util;

// The client should tear down when it receives an event with an invalid magic number.
CLIENT_TEST(60, ReceiveEventBadMagicNumber) {
  Bytes event = Header{
      .txid = 0,
      .magic_number = kBadMagicNumber,
      .ordinal = kOrdinal_ClosedTarget_OnEventNoPayload,
  };
  auto reporter = ReceiveClosedEvents();
  ASSERT_NE(reporter, nullptr);
  ASSERT_OK(server_end().write(event));
  WAIT_UNTIL([reporter]() { return reporter->NumReceivedEvents(); });
  ASSERT_EQ(reporter->NumReceivedEvents(), 1u);
  auto reporter_event = reporter->TakeNextEvent();
  ASSERT_TRUE(reporter_event.fidl_error().has_value());
  ASSERT_EQ(reporter_event.fidl_error().value(), fidl_clientsuite::FidlErrorKind::kDecodingError);

  // TODO(https://fxbug.dev/42159083, https://fxbug.dev/42153903): Clients should close the channel
  // when an error occurs, but many of them don't actually.
  // ASSERT_TRUE(server_end().is_signal_present(ZX_CHANNEL_PEER_CLOSED));
}

// The client should tear down when it receives an event with nonzero txid.
CLIENT_TEST(61, ReceiveEventUnexpectedTxid) {
  Bytes event = Header{.txid = 123, .ordinal = kOrdinal_ClosedTarget_OnEventNoPayload};
  auto reporter = ReceiveClosedEvents();
  ASSERT_NE(reporter, nullptr);
  ASSERT_OK(server_end().write(event));
  WAIT_UNTIL([reporter]() { return reporter->NumReceivedEvents(); });
  ASSERT_EQ(reporter->NumReceivedEvents(), 1u);
  auto reporter_event = reporter->TakeNextEvent();
  ASSERT_TRUE(reporter_event.fidl_error().has_value());
  ASSERT_EQ(reporter_event.fidl_error().value(),
            fidl_clientsuite::FidlErrorKind::kUnexpectedMessage);

  // TODO(https://fxbug.dev/42159083, https://fxbug.dev/42153903): Clients should close the channel
  // when an error occurs, but many of them don't actually.
  // ASSERT_TRUE(server_end().is_signal_present(ZX_CHANNEL_PEER_CLOSED));
}

// The client should tear down when it receives an event with an unknown ordinal.
CLIENT_TEST(62, ReceiveEventUnknownOrdinal) {
  Bytes event = Header{.txid = 0, .ordinal = kOrdinalFakeUnknownMethod};
  auto reporter = ReceiveClosedEvents();
  ASSERT_NE(reporter, nullptr);
  ASSERT_OK(server_end().write(event));
  WAIT_UNTIL([reporter]() { return reporter->NumReceivedEvents(); });
  ASSERT_EQ(reporter->NumReceivedEvents(), 1u);
  auto reporter_event = reporter->TakeNextEvent();
  ASSERT_TRUE(reporter_event.fidl_error().has_value());
  ASSERT_EQ(reporter_event.fidl_error().value(),
            fidl_clientsuite::FidlErrorKind::kUnexpectedMessage);

  // TODO(https://fxbug.dev/42159083, https://fxbug.dev/42153903): Clients should close the channel
  // when an error occurs, but many of them don't actually.
  // ASSERT_TRUE(server_end().is_signal_present(ZX_CHANNEL_PEER_CLOSED));
}

// The client should tear down when it receives a response with an invalid magic number.
CLIENT_TEST(63, ReceiveResponseBadMagicNumber) {
  Bytes expected_request = Header{
      .txid = kTxidNotKnown,
      .ordinal = kOrdinal_ClosedTarget_TwoWayNoPayload,
  };
  Bytes response = Header{
      .txid = kTxidNotKnown,
      .magic_number = kBadMagicNumber,
      .ordinal = kOrdinal_ClosedTarget_TwoWayNoPayload,
  };
  runner()->CallTwoWayNoPayload({{.target = TakeClosedClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().fidl_error().has_value());
    EXPECT_EQ(result.value().fidl_error().value(), fidl_clientsuite::FidlErrorKind::kDecodingError);
  });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();

  // TODO(https://fxbug.dev/42159083, https://fxbug.dev/42153903): Clients should close the channel
  // when an error occurs, but many of them don't actually.
  // ASSERT_TRUE(server_end().is_signal_present(ZX_CHANNEL_PEER_CLOSED));
}

// The client should tear down when it receives a response with an unexpected txid.
CLIENT_TEST(64, ReceiveResponseUnexpectedTxid) {
  if (WaitFor(runner()->GetBindingsProperties()).value().io_style() ==
      fidl_clientsuite::IoStyle::kSync) {
    GTEST_SKIP() << "Skipping because sync bindings use zx_channel_call, so the thread would "
                    "remain blocked if we respond with a different txid";
  }

  // Note: The client won't choose wrong_txid (i.e. the test isn't flaky)
  // because async binding use incrementing txids from 1, and sync bindings use
  // zx_channel_call which uses a txid with the high bit set.
  zx_txid_t right_txid;
  zx_txid_t wrong_txid = 123;

  Bytes expected_request =
      Header{.txid = kTxidNotKnown, .ordinal = kOrdinal_ClosedTarget_TwoWayNoPayload};
  Bytes response = Header{.txid = wrong_txid, .ordinal = kOrdinal_ClosedTarget_TwoWayNoPayload};
  runner()->CallTwoWayNoPayload({{.target = TakeClosedClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().fidl_error().has_value());
    EXPECT_EQ(result.value().fidl_error().value(),
              fidl_clientsuite::FidlErrorKind::kUnexpectedMessage);
  });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &right_txid));
  ASSERT_NE(right_txid, 0u);
  ASSERT_NE(right_txid, wrong_txid);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();

  // TODO(https://fxbug.dev/42159083, https://fxbug.dev/42153903): Clients should close the channel
  // when an error occurs, but many of them don't actually.
  // ASSERT_TRUE(server_end().is_signal_present(ZX_CHANNEL_PEER_CLOSED));
}

// The client should tear down when it receives a response with an ordinal
// that is known but different from the request ordinal.
CLIENT_TEST(65, ReceiveResponseWrongOrdinalKnown) {
  Bytes expected_request =
      Header{.txid = kTxidNotKnown, .ordinal = kOrdinal_ClosedTarget_TwoWayNoPayload};
  Bytes response =
      Header{.txid = kTxidNotKnown, .ordinal = kOrdinal_ClosedTarget_TwoWayStructPayload};
  runner()->CallTwoWayNoPayload({{.target = TakeClosedClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().fidl_error().has_value());
    EXPECT_EQ(result.value().fidl_error().value(), fidl_clientsuite::FidlErrorKind::kDecodingError);
  });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();

  // TODO(https://fxbug.dev/42159083, https://fxbug.dev/42153903): Clients should close the channel
  // when an error occurs, but many of them don't actually.
  // ASSERT_TRUE(server_end().is_signal_present(ZX_CHANNEL_PEER_CLOSED));
}

// The client should tear down when it receives a response with an unknown ordinal.
CLIENT_TEST(67, ReceiveResponseWrongOrdinalUnknown) {
  Bytes expected_request =
      Header{.txid = kTxidNotKnown, .ordinal = kOrdinal_ClosedTarget_TwoWayNoPayload};
  Bytes response = Header{.txid = kTxidNotKnown, .ordinal = kOrdinalFakeUnknownMethod};
  runner()->CallTwoWayNoPayload({{.target = TakeClosedClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().fidl_error().has_value());
    EXPECT_EQ(result.value().fidl_error().value(), fidl_clientsuite::FidlErrorKind::kDecodingError);
  });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();

  // TODO(https://fxbug.dev/42159083, https://fxbug.dev/42153903): Clients should close the channel
  // when an error occurs, but many of them don't actually.
  // ASSERT_TRUE(server_end().is_signal_present(ZX_CHANNEL_PEER_CLOSED));
}

}  // namespace
}  // namespace client_suite
