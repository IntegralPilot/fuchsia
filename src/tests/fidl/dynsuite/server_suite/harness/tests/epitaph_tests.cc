// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.serversuite/cpp/common_types.h>

#include "src/lib/testing/predicates/status.h"
#include "src/tests/fidl/dynsuite/server_suite/harness/harness.h"
#include "src/tests/fidl/dynsuite/server_suite/harness/ordinals.h"

namespace server_suite {
namespace {

using namespace ::channel_util;

// The server should be able to send an epitaph.
CLOSED_SERVER_TEST(24, ServerSendsEpitaph) {
  zx_status_t epitaph = 456;
  Bytes expected = {
      Header{.txid = 0, .ordinal = kOrdinalEpitaph},
      {int32(epitaph), padding(4)},
  };
  ASSERT_RESULT_OK(runner()->ShutdownWithEpitaph({epitaph}));
  ASSERT_OK(client_end().read_and_check(expected));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kVoluntaryShutdown);
}

// It is not permissible to send epitaphs to servers.
CLOSED_SERVER_TEST(25, ServerReceivesEpitaphInvalid) {
  Bytes request = {
      Header{.txid = 0, .ordinal = kOrdinalEpitaph},
      {int32(456), padding(4)},
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kUnexpectedMessage);
}

}  // namespace
}  // namespace server_suite
