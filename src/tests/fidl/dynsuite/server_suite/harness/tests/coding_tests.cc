// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.serversuite/cpp/common_types.h>

#include "src/lib/testing/predicates/status.h"
#include "src/tests/fidl/dynsuite/channel_util/bytes.h"
#include "src/tests/fidl/dynsuite/server_suite/harness/harness.h"
#include "src/tests/fidl/dynsuite/server_suite/harness/ordinals.h"

namespace server_suite {
namespace {

using namespace ::channel_util;

// The server should tear down when it fails to decode a request.
CLOSED_SERVER_TEST(15, BadPayloadEncoding) {
  Bytes request = {
      Header{.txid = kTwoWayTxid, .ordinal = kOrdinal_ClosedTarget_TwoWayResult},
      // Ordinal 3 is unknown in the FIDL schema, and the union is strict.
      union_ordinal(3),
      out_of_line_envelope(0),
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kDecodeFailure);
}

// The server should reject V1 messages (no payload).
CLOSED_SERVER_TEST(105, V1TwoWayNoPayload) {
  Bytes request = Header{
      .txid = kTwoWayTxid,
      .at_rest_flags = {0, 0},  // at-rest flags without V2 bit set
      .ordinal = kOrdinal_ClosedTarget_TwoWayNoPayload,
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kIncompatibleFormat);
}

// The server should reject V1 messages (struct payload).
CLOSED_SERVER_TEST(106, V1TwoWayStructPayload) {
  Bytes request = {
      Header{
          .txid = kTwoWayTxid,
          .at_rest_flags = {0, 0},  // at-rest flags without V2 bit set
          .ordinal = kOrdinal_ClosedTarget_TwoWayStructPayload,
      },
      {int8(0), padding(7)},
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kIncompatibleFormat);
}

}  // namespace
}  // namespace server_suite
