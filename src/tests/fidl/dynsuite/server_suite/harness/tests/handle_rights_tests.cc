// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.serversuite/cpp/common_types.h>
#include <lib/zx/port.h>

#include "src/lib/testing/predicates/status.h"
#include "src/tests/fidl/dynsuite/channel_util/channel.h"
#include "src/tests/fidl/dynsuite/server_suite/harness/harness.h"
#include "src/tests/fidl/dynsuite/server_suite/harness/ordinals.h"

namespace server_suite {
namespace {

using namespace ::channel_util;

// The server should tear down when the request is missing a handle.
CLOSED_SERVER_TEST(16, ClientSendsTooFewHandles) {
  Bytes request = {
      Header{.txid = kTwoWayTxid, .ordinal = kOrdinal_ClosedTarget_GetSignalableEventRights},
      {handle_present(), padding(4)},
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kDecodeFailure);
}

// The server should tear down when it receives the wrong handle type.
CLOSED_SERVER_TEST(17, ClientSendsWrongHandleType) {
  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));

  Message request = {
      Bytes{
          Header{.txid = kTwoWayTxid, .ordinal = kOrdinal_ClosedTarget_GetSignalableEventRights},
          {handle_present(), padding(4)},
      },
      Handles{
          {.handle = port.release(), .type = ZX_OBJ_TYPE_PORT},
      },
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kDecodeFailure);
}

// When a handle with too many rights is sent, the rights should be reduced.
CLOSED_SERVER_TEST(18, ClientSendsTooManyRights) {
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  // Validate that more rights than just ZX_RIGHT_SIGNAL are present.
  zx_info_handle_basic_t info;
  ASSERT_OK(event.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof info, nullptr, nullptr));
  ASSERT_EQ(info.rights, ZX_DEFAULT_EVENT_RIGHTS);
  static_assert(ZX_DEFAULT_EVENT_RIGHTS & ZX_RIGHT_SIGNAL);
  static_assert(ZX_DEFAULT_EVENT_RIGHTS & ~ZX_RIGHT_SIGNAL);

  Header header = {.txid = kTwoWayTxid, .ordinal = kOrdinal_ClosedTarget_GetSignalableEventRights};
  Message request = {
      Bytes{header, handle_present(), padding(4)},
      Handles{{.handle = event.release(), .type = ZX_OBJ_TYPE_EVENT}},
  };
  ExpectedMessage expected_response = {
      Bytes{header, uint32(ZX_RIGHT_SIGNAL), padding(4)},
      ExpectedHandles{},
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_OK(client_end().read_and_check(expected_response));
}

// The server should tear down when it receives a handle with too few rights.
CLOSED_SERVER_TEST(19, ClientSendsTooFewRights) {
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  zx::event reduced_rights_event;
  ASSERT_OK(event.replace(ZX_RIGHT_TRANSFER, &reduced_rights_event));

  Message request = {
      Bytes{
          Header{.txid = kTwoWayTxid, .ordinal = kOrdinal_ClosedTarget_GetSignalableEventRights},
          {handle_present(), padding(4)},
      },
      Handles{
          {.handle = reduced_rights_event.release(), .type = ZX_OBJ_TYPE_EVENT},
      },
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kDecodeFailure);
}

// The server should handle ZX_OBJ_TYPE_NONE and ZX_RIGHT_SAME_RIGHTS correctly.
// ZX_OBJ_TYPE_NONE means "any object type is allowed".
// ZX_RIGHT_SAME_RIGHTS means "any rights are allowed".
CLOSED_SERVER_TEST(20, ClientSendsObjectOverPlainHandle) {
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  Header header = {.txid = kTwoWayTxid, .ordinal = kOrdinal_ClosedTarget_GetHandleRights};
  Message request = {
      Bytes{header, handle_present(), padding(4)},
      Handles{{.handle = event.release(), .type = ZX_OBJ_TYPE_EVENT}},
  };
  ExpectedMessage expected_response = {
      Bytes{header, uint32(ZX_DEFAULT_EVENT_RIGHTS), padding(4)},
      ExpectedHandles{},
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_OK(client_end().read_and_check(expected_response));
}

// The server should tear down when it tries to send the wrong handle type.
CLOSED_SERVER_TEST(21, ServerSendsWrongHandleType) {
  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));

  Message request = {
      Bytes{
          Header{.txid = kTwoWayTxid,
                 .ordinal = kOrdinal_ClosedTarget_EchoAsTransferableSignalableEvent},
          {handle_present(), padding(4)},
      },
      Handles{
          {.handle = port.release(), .type = ZX_OBJ_TYPE_PORT},
      },
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kWriteFailure);
}

// When the server sends a handle with too many rights, the rights should be reduced.
CLOSED_SERVER_TEST(22, ServerSendsTooManyRights) {
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  // Validate that more rights than just ZX_RIGHT_SIGNAL are present.
  zx_info_handle_basic_t info;
  ASSERT_OK(event.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(ZX_DEFAULT_EVENT_RIGHTS, info.rights);
  static_assert(ZX_DEFAULT_EVENT_RIGHTS & ZX_RIGHT_SIGNAL);
  static_assert(ZX_DEFAULT_EVENT_RIGHTS & ~ZX_RIGHT_SIGNAL);

  Bytes bytes = {
      Header{.txid = kTwoWayTxid,
             .ordinal = kOrdinal_ClosedTarget_EchoAsTransferableSignalableEvent},
      {handle_present(), padding(4)},
  };
  Message request = {
      bytes,
      Handles{{.handle = event.release(), .type = ZX_OBJ_TYPE_EVENT}},
  };
  ExpectedMessage expected_response = {
      bytes,
      ExpectedHandles{{
          .koid = info.koid,
          .type = ZX_OBJ_TYPE_EVENT,
          .rights = ZX_RIGHT_SIGNAL | ZX_RIGHT_TRANSFER,
      }},
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_OK(client_end().read_and_check(expected_response));
}

// The server should tear down when it tries to send a handle with too few rights.
CLOSED_SERVER_TEST(23, ServerSendsTooFewRights) {
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  zx::event reduced_rights_event;
  ASSERT_OK(event.replace(ZX_RIGHT_TRANSFER, &reduced_rights_event));

  Message request = {
      Bytes{
          Header{.txid = kTwoWayTxid,
                 .ordinal = kOrdinal_ClosedTarget_EchoAsTransferableSignalableEvent},
          {handle_present(), padding(4)},
      },
      Handles{
          {.handle = reduced_rights_event.release(), .type = ZX_OBJ_TYPE_EVENT},
      },
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kWriteFailure);
}

}  // namespace
}  // namespace server_suite
