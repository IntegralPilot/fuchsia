// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/types.h>

#include "src/lib/testing/predicates/status.h"
#include "src/tests/fidl/dynsuite/channel_util/channel.h"
#include "src/tests/fidl/dynsuite/server_suite/harness/harness.h"
#include "src/tests/fidl/dynsuite/server_suite/harness/ordinals.h"

namespace server_suite {
namespace {

using namespace ::channel_util;

// Check that the test runner is set up correctly without doing anything else.
CLOSED_SERVER_TEST(1, Setup) {}

// Check that the test disabling mechanism works.
CLOSED_SERVER_TEST(107, IgnoreDisabled) { FAIL() << "This test should be skipped!"; }

// The server should receive a one-way method request.
CLOSED_SERVER_TEST(2, OneWayNoPayload) {
  Bytes request = Header{.txid = 0, .ordinal = kOrdinal_ClosedTarget_OneWayNoPayload};
  ASSERT_OK(client_end().write(request));
  ASSERT_RUNNER_EVENT(RunnerEvent::kOnReceivedClosedTargetOneWayNoPayload);
}

// The server should reply to a two-way method request (no payload).
CLOSED_SERVER_TEST(3, TwoWayNoPayload) {
  Bytes bytes = Header{.txid = kTwoWayTxid, .ordinal = kOrdinal_ClosedTarget_TwoWayNoPayload};
  ASSERT_OK(client_end().write(bytes));
  ASSERT_OK(client_end().read_and_check(bytes));
}

// The server should reply to a two-way method request (struct payload).
CLOSED_SERVER_TEST(6, TwoWayStructPayload) {
  Bytes bytes = {
      Header{.txid = kTwoWayTxid, .ordinal = kOrdinal_ClosedTarget_TwoWayStructPayload},
      {uint8(0xab), padding(7)},
  };
  ASSERT_OK(client_end().write(bytes));
  ASSERT_OK(client_end().read_and_check(bytes));
}

// The server should reply to a two-way method request (table payload).
CLOSED_SERVER_TEST(7, TwoWayTablePayload) {
  Bytes bytes = {
      Header{.txid = kTwoWayTxid, .ordinal = kOrdinal_ClosedTarget_TwoWayTablePayload},
      table_max_ordinal(1),
      pointer_present(),
      inline_envelope(uint8(0xab)),
  };
  ASSERT_OK(client_end().write(bytes));
  ASSERT_OK(client_end().read_and_check(bytes));
}

// The server should reply to a two-way method request (union payload).
CLOSED_SERVER_TEST(8, TwoWayUnionPayload) {
  Bytes bytes = {
      Header{.txid = kTwoWayTxid, .ordinal = kOrdinal_ClosedTarget_TwoWayUnionPayload},
      union_ordinal(1),
      inline_envelope(uint8(0xab)),
  };
  ASSERT_OK(client_end().write(bytes));
  ASSERT_OK(client_end().read_and_check(bytes));
}

// The server should reply to a fallible method (success).
CLOSED_SERVER_TEST(4, TwoWayResultWithPayload) {
  Bytes bytes = {
      Header{.txid = kTwoWayTxid, .ordinal = kOrdinal_ClosedTarget_TwoWayResult},
      union_ordinal(1),
      out_of_line_envelope(24, 0),
      string_header(3),
      {{'a', 'b', 'c'}, padding(5)},
  };
  ASSERT_OK(client_end().write(bytes));
  ASSERT_OK(client_end().read_and_check(bytes));
}

// The server should reply to a fallible method (error).
CLOSED_SERVER_TEST(5, TwoWayResultWithError) {
  Bytes bytes = {
      Header{.txid = kTwoWayTxid, .ordinal = kOrdinal_ClosedTarget_TwoWayResult},
      union_ordinal(2),
      inline_envelope(uint32(0xab)),
  };
  ASSERT_OK(client_end().write(bytes));
  ASSERT_OK(client_end().read_and_check(bytes));
}

}  // namespace
}  // namespace server_suite
