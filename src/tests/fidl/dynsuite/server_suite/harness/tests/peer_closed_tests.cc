// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/testing/predicates/status.h"
#include "src/tests/fidl/dynsuite/server_suite/harness/harness.h"
#include "src/tests/fidl/dynsuite/server_suite/harness/ordinals.h"

namespace server_suite {
namespace {

using namespace ::channel_util;

// When sending an event, if channel_write returns PEER_CLOSED, the bindings
// should hide it and return successfully. This helps prevent race conditions.
OPEN_SERVER_TEST(109, EventSendingDoNotReportPeerClosed) {
  client_end().reset();
  ASSERT_RESULT_OK(runner()->SendOpenTargetStrictEvent());
  // If sending the event fails, the Runner will panic instead of tearing down normally.
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kPeerClosed);
}

// When sending a reply, if channel_write returns PEER_CLOSED, the bindings
// should hide it and return successfully. This helps prevent race conditions.
CLOSED_SERVER_TEST(110, ReplySendingDoNotReportPeerClosed) {
  Bytes request = Header{.txid = kTwoWayTxid, .ordinal = kOrdinal_ClosedTarget_TwoWayNoPayload};
  ASSERT_OK(client_end().write(request));
  client_end().reset();
  // If sending the reply fails, the Runner will panic instead of tearing down normally.
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kPeerClosed);
}

// The server should drain out messages buffered by a client, even when the
// client closed their endpoint right away after writing those messages.
CLOSED_SERVER_TEST(111, ReceiveOneWayNoPayloadFromPeerClosedChannel) {
  Bytes request = Header{.txid = 0, .ordinal = kOrdinal_ClosedTarget_OneWayNoPayload};
  ASSERT_OK(client_end().write(request));
  client_end().reset();
  ASSERT_RUNNER_EVENT(RunnerEvent::kOnReceivedClosedTargetOneWayNoPayload);
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kPeerClosed);
}

// This test isn't really necessary, since the test fixture does this implicitly
// at the end of all tests that don't call ASSERT_SERVER_TEARDOWN themselves.
// We include it here just to be explicit that this behavior is covered.
CLOSED_SERVER_TEST(113, ServerTearsDownWhenPeerClosed) {
  client_end().reset();
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kPeerClosed);
}

}  // namespace
}  // namespace server_suite
