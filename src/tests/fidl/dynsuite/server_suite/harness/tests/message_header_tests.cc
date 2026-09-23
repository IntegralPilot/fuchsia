// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/fidl.h>
#include <zircon/types.h>

#include "src/lib/testing/predicates/status.h"
#include "src/tests/fidl/dynsuite/channel_util/bytes.h"
#include "src/tests/fidl/dynsuite/channel_util/channel.h"
#include "src/tests/fidl/dynsuite/server_suite/harness/harness.h"
#include "src/tests/fidl/dynsuite/server_suite/harness/ordinals.h"

namespace server_suite {
namespace {

using namespace ::channel_util;

// The server should tear down when it receives a one-way request with nonzero txid.
CLOSED_SERVER_TEST(9, OneWayWithNonZeroTxid) {
  Bytes request = Header{.txid = kTwoWayTxid, .ordinal = kOrdinal_ClosedTarget_OneWayNoPayload};
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kUnexpectedMessage);
}

// The server should tear down when it receives a two-way request with zero txid.
CLOSED_SERVER_TEST(10, TwoWayNoPayloadWithZeroTxid) {
  Bytes request = Header{.txid = 0, .ordinal = kOrdinal_ClosedTarget_TwoWayNoPayload};
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kUnexpectedMessage);
}

// The closed server should tear down when it receives a request with an unknown ordinal.
CLOSED_SERVER_TEST(11, UnknownOrdinalCausesClose) {
  Bytes request = Header{.txid = 0, .ordinal = kOrdinalFakeUnknownMethod};
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kUnexpectedMessage);
}

// The server should tear down when it receives a request with an invalid magic number.
CLOSED_SERVER_TEST(12, BadMagicNumberCausesClose) {
  Bytes request = Header{
      .txid = kTwoWayTxid,
      .magic_number = kBadMagicNumber,
      .ordinal = kOrdinal_ClosedTarget_TwoWayNoPayload,
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kIncompatibleFormat);
}

// The server should ignore unrecognized at-rest flags.
CLOSED_SERVER_TEST(13, IgnoresUnrecognizedAtRestFlags) {
  Bytes request = Header{
      .txid = kTwoWayTxid,
      .at_rest_flags = {0xff, 0xff},
      .ordinal = kOrdinal_ClosedTarget_TwoWayNoPayload,
  };
  Bytes expected_response = Header{
      .txid = kTwoWayTxid,
      .ordinal = kOrdinal_ClosedTarget_TwoWayNoPayload,
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_OK(client_end().read_and_check(expected_response));
}

// The server should ignore unrecognized dynamic flags.
CLOSED_SERVER_TEST(14, IgnoresUnrecognizedDynamicFlags) {
  Bytes request = Header{
      .txid = kTwoWayTxid,
      .dynamic_flags = 0x7f,  // all bits set except FLEXIBLE (most significant bit)
      .ordinal = kOrdinal_ClosedTarget_TwoWayNoPayload,
  };
  Bytes expected_response = Header{
      .txid = kTwoWayTxid,
      .ordinal = kOrdinal_ClosedTarget_TwoWayNoPayload,
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_OK(client_end().read_and_check(expected_response));
}

}  // namespace
}  // namespace server_suite
