// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.serversuite/cpp/common_types.h>

#include "src/lib/testing/predicates/status.h"
#include "src/tests/fidl/dynsuite/channel_util/channel.h"
#include "src/tests/fidl/dynsuite/server_suite/harness/harness.h"
#include "src/tests/fidl/dynsuite/server_suite/harness/ordinals.h"

namespace server_suite {
namespace {

using namespace ::channel_util;

const uint32_t kMaxVecBytesInMsg =
    ZX_CHANNEL_MAX_MSG_BYTES - sizeof(fidl_message_header_t) - sizeof(fidl_vector_t);
const uint32_t kMaxVecHandlesInMsg = ZX_CHANNEL_MAX_MSG_HANDLES;

// The server should accept a request with the maximum number of bytes.
CLOSED_SERVER_TEST(26, RequestMatchesByteLimit) {
  uint32_t count = kMaxVecBytesInMsg;
  Header header = {.txid = kTwoWayTxid, .ordinal = kOrdinal_ClosedTarget_ByteVectorSize};
  Bytes request = {header, vector_header(count), repeat(0x00).times(count)};
  Bytes expected_response = {header, uint32(count), padding(4)};
  ASSERT_OK(client_end().write(request));
  ASSERT_OK(client_end().read_and_check(expected_response));
}

// The serve should accept a request with the maximum number of handles.
CLOSED_SERVER_TEST(27, RequestMatchesHandleLimit) {
  uint32_t count = kMaxVecHandlesInMsg;
  Handles handles;
  for (uint32_t i = 0; i < count; i++) {
    zx::event event;
    ASSERT_OK(zx::event::create(0, &event));
    handles.push_back(Handle{.handle = event.release(), .type = ZX_OBJ_TYPE_EVENT});
  }

  Header header = {.txid = kTwoWayTxid, .ordinal = kOrdinal_ClosedTarget_HandleVectorSize};
  Message request = {
      Bytes{
          header,
          vector_header(count),
          repeat(0xff).times(count * sizeof(zx_handle_t)),
      },
      handles,
  };
  ExpectedMessage expected_response = {
      {header, uint32(count), padding(4)},
      ExpectedHandles{},
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_OK(client_end().read_and_check(expected_response));
}

// The server should be able to send a response with the maximum number of bytes.
CLOSED_SERVER_TEST(28, ResponseMatchesByteLimit) {
  uint32_t count = kMaxVecBytesInMsg;
  Header header = {.txid = kTwoWayTxid, .ordinal = kOrdinal_ClosedTarget_CreateNByteVector};
  Bytes request = {header, uint32(count), padding(4)};
  Bytes expected_response = {header, vector_header(count), repeat(0x00).times(count)};
  ASSERT_OK(client_end().write(request));
  ASSERT_OK(client_end().read_and_check(expected_response));
}

// The server should tear down when it tries to send a response with too many bytes.
CLOSED_SERVER_TEST(30, ResponseExceedsByteLimit) {
  uint32_t count = kMaxVecBytesInMsg + 1;
  Bytes request = {
      Header{.txid = kTwoWayTxid, .ordinal = kOrdinal_ClosedTarget_CreateNByteVector},
      {uint32(count), padding(4)},
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kWriteFailure);
}

// The server should be able to send a response with the maximum number of handles.
CLOSED_SERVER_TEST(29, ResponseMatchesHandleLimit) {
  uint32_t count = kMaxVecHandlesInMsg;
  ExpectedHandles expected_handles;
  for (uint32_t i = 0; i < count; i++) {
    expected_handles.push_back(ExpectedHandle{
        .type = ZX_OBJ_TYPE_EVENT,
        .rights = ZX_DEFAULT_EVENT_RIGHTS,
    });
  }

  Header header = {.txid = kTwoWayTxid, .ordinal = kOrdinal_ClosedTarget_CreateNHandleVector};
  Message request = {
      Bytes{header, uint32(count), padding(4)},
      Handles{},
  };
  ExpectedMessage expected_response = {
      Bytes{
          header,
          vector_header(count),
          repeat(0xff).times(count * sizeof(zx_handle_t)),
      },
      expected_handles,
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_OK(client_end().read_and_check(expected_response));
}

// The server should tear down when it tries to send a response with too many handles.
CLOSED_SERVER_TEST(31, ResponseExceedsHandleLimit) {
  uint32_t count = kMaxVecHandlesInMsg + 1;
  Bytes request = {
      Header{.txid = kTwoWayTxid, .ordinal = kOrdinal_ClosedTarget_CreateNHandleVector},
      {uint32(count), padding(4)},
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kWriteFailure);
}

}  // namespace
}  // namespace server_suite
