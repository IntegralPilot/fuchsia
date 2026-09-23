// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.serversuite/cpp/common_types.h>
#include <fidl/fidl.serversuite/cpp/natural_types.h>
#include <lib/zx/eventpair.h>
#include <zircon/fidl.h>

#include "src/lib/testing/predicates/status.h"
#include "src/tests/fidl/dynsuite/channel_util/bytes.h"
#include "src/tests/fidl/dynsuite/channel_util/channel.h"
#include "src/tests/fidl/dynsuite/server_suite/harness/harness.h"
#include "src/tests/fidl/dynsuite/server_suite/harness/ordinals.h"

namespace server_suite {
namespace {

using namespace ::channel_util;

// The server should send a strict event.
OPEN_SERVER_TEST(32, SendStrictEvent) {
  Bytes expected_event = Header{.txid = 0, .ordinal = kOrdinal_OpenTarget_StrictEvent};
  ASSERT_RESULT_OK(runner()->SendOpenTargetStrictEvent());
  ASSERT_OK(client_end().read_and_check(expected_event));
}

// The server should send a flexible event.
OPEN_SERVER_TEST(33, SendFlexibleEvent) {
  Bytes expected_event = Header{
      .txid = 0,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_FlexibleEvent,
  };
  ASSERT_RESULT_OK(runner()->SendOpenTargetFlexibleEvent());
  ASSERT_OK(client_end().read_and_check(expected_event));
}

// The server should receive a strict one-way method.
OPEN_SERVER_TEST(34, ReceiveStrictOneWay) {
  Bytes request = Header{.txid = 0, .ordinal = kOrdinal_OpenTarget_StrictOneWay};
  ASSERT_OK(client_end().write(request));
  ASSERT_RUNNER_EVENT(RunnerEvent::kOnReceivedOpenTargetStrictOneWay);
}

// The server should receive a one-way method, despite the schema (strict)
// not matching the dynamic flags (flexible).
OPEN_SERVER_TEST(35, ReceiveStrictOneWayMismatchedStrictness) {
  Bytes request = Header{
      .txid = 0,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_StrictOneWay,
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_RUNNER_EVENT(RunnerEvent::kOnReceivedOpenTargetStrictOneWay);
}

// The server should receive a flexible one-way method.
OPEN_SERVER_TEST(36, ReceiveFlexibleOneWay) {
  Bytes request = Header{
      .txid = 0,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_FlexibleOneWay,
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_RUNNER_EVENT(RunnerEvent::kOnReceivedOpenTargetFlexibleOneWay);
}

// The server should receive a one-way method, despite the schema (flexible)
// not matching the dynamic flags (strict).
OPEN_SERVER_TEST(37, ReceiveFlexibleOneWayMismatchedStrictness) {
  Bytes request = Header{.txid = 0, .ordinal = kOrdinal_OpenTarget_FlexibleOneWay};
  ASSERT_OK(client_end().write(request));
  ASSERT_RUNNER_EVENT(RunnerEvent::kOnReceivedOpenTargetFlexibleOneWay);
}

// The server should reply to a strict two-way method.
OPEN_SERVER_TEST(38, StrictTwoWayResponse) {
  Bytes bytes = Header{.txid = kTwoWayTxid, .ordinal = kOrdinal_OpenTarget_StrictTwoWay};
  ASSERT_OK(client_end().write(bytes));
  ASSERT_OK(client_end().read_and_check(bytes));
}

// The server should reply to a two-way method, despite the schema (strict)
// not matching the request's dynamic flags (flexible).
OPEN_SERVER_TEST(39, StrictTwoWayResponseMismatchedStrictness) {
  Bytes request = Header{
      .txid = kTwoWayTxid,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_StrictTwoWay,
  };
  Bytes expected_response = Header{
      .txid = kTwoWayTxid,
      .ordinal = kOrdinal_OpenTarget_StrictTwoWay,
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_OK(client_end().read_and_check(expected_response));
}

// The server should reply to a strict two-way method (nonempty).
OPEN_SERVER_TEST(63, StrictTwoWayNonEmptyResponse) {
  Bytes bytes = {
      Header{.txid = kTwoWayTxid, .ordinal = kOrdinal_OpenTarget_StrictTwoWayFields},
      encode(fidl_serversuite::OpenTargetStrictTwoWayFieldsRequest(504230)),
  };
  ASSERT_OK(client_end().write(bytes));
  ASSERT_OK(client_end().read_and_check(bytes));
}

// The server should reply to a strict fallible two-way method (success).
OPEN_SERVER_TEST(40, StrictTwoWayErrorSyntaxResponse) {
  Bytes bytes = {
      Header{.txid = kTwoWayTxid, .ordinal = kOrdinal_OpenTarget_StrictTwoWayErr},
      union_ordinal(kResultUnionSuccess),
      inline_envelope({0x00}),
  };
  ASSERT_OK(client_end().write(bytes));
  ASSERT_OK(client_end().read_and_check(bytes));
}

// The server should reply to a fallible two-way method (success), despite the
// schema (strict) not matching the request's dynamic flags (flexible).
OPEN_SERVER_TEST(41, StrictTwoWayErrorSyntaxResponseMismatchedStrictness) {
  Bytes payload = {
      union_ordinal(kResultUnionSuccess),
      inline_envelope({0x00}),
  };
  Bytes request = {
      Header{
          .txid = kTwoWayTxid,
          .dynamic_flags = kDynamicFlagsFlexible,
          .ordinal = kOrdinal_OpenTarget_StrictTwoWayErr,
      },
      payload,
  };
  Bytes expected_response = {
      Header{
          .txid = kTwoWayTxid,
          .ordinal = kOrdinal_OpenTarget_StrictTwoWayErr,
      },
      payload,
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_OK(client_end().read_and_check(expected_response));
}

// The server should reply to a strict fallible two-way method (nonempty success).
OPEN_SERVER_TEST(64, StrictTwoWayErrorSyntaxNonEmptyResponse) {
  Bytes bytes = {
      Header{.txid = kTwoWayTxid, .ordinal = kOrdinal_OpenTarget_StrictTwoWayFieldsErr},
      union_ordinal(kResultUnionSuccess),
      inline_envelope(int32(406601)),
  };
  ASSERT_OK(client_end().write(bytes));
  ASSERT_OK(client_end().read_and_check(bytes));
}

// The server should reply to a flexible two-way method.
OPEN_SERVER_TEST(42, FlexibleTwoWayResponse) {
  Header header = {
      .txid = kTwoWayTxid,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_FlexibleTwoWay,
  };
  Bytes request = header;
  Bytes expected_response = {header, union_ordinal(kResultUnionSuccess), inline_envelope({0x00})};
  ASSERT_OK(client_end().write(request));
  ASSERT_OK(client_end().read_and_check(expected_response));
}

// The server should reply to a two-way method, despite the schema (flexible)
// not matching the request's dynamic flags (strict).
OPEN_SERVER_TEST(43, FlexibleTwoWayResponseMismatchedStrictness) {
  Bytes request = Header{
      .txid = kTwoWayTxid,
      .ordinal = kOrdinal_OpenTarget_FlexibleTwoWay,
  };
  Bytes expected_response = {
      Header{
          .txid = kTwoWayTxid,
          .dynamic_flags = kDynamicFlagsFlexible,
          .ordinal = kOrdinal_OpenTarget_FlexibleTwoWay,
      },
      union_ordinal(kResultUnionSuccess),
      inline_envelope({0x00}),
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_OK(client_end().read_and_check(expected_response));
}

// The server should reply to a flexible two-way method (nonempty).
OPEN_SERVER_TEST(44, FlexibleTwoWayNonEmptyResponse) {
  Header header = {
      .txid = kTwoWayTxid,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_FlexibleTwoWayFields,
  };
  Bytes payload = int32(3023950);
  Bytes request = {header, payload, padding(4)};
  Bytes expected_response = {header, union_ordinal(kResultUnionSuccess), inline_envelope(payload)};
  ASSERT_OK(client_end().write(request));
  ASSERT_OK(client_end().read_and_check(expected_response));
}

// The server should reply to a flexible fallible two-way method (success).
OPEN_SERVER_TEST(45, FlexibleTwoWayErrorSyntaxResponseSuccessResult) {
  Bytes bytes = {
      Header{
          .txid = kTwoWayTxid,
          .dynamic_flags = kDynamicFlagsFlexible,
          .ordinal = kOrdinal_OpenTarget_FlexibleTwoWayErr,
      },
      union_ordinal(kResultUnionSuccess),
      inline_envelope({0x00}),
  };
  ASSERT_OK(client_end().write(bytes));
  ASSERT_OK(client_end().read_and_check(bytes));
}

// The server should reply to a flexible fallible two-way method (error).
OPEN_SERVER_TEST(46, FlexibleTwoWayErrorSyntaxResponseErrorResult) {
  Bytes bytes = {
      Header{
          .txid = kTwoWayTxid,
          .dynamic_flags = kDynamicFlagsFlexible,
          .ordinal = kOrdinal_OpenTarget_FlexibleTwoWayErr,
      },
      union_ordinal(kResultUnionDomainError),
      inline_envelope(int32(60602293)),
  };
  ASSERT_OK(client_end().write(bytes));
  ASSERT_OK(client_end().read_and_check(bytes));
}

// The server should reply to a flexible fallible two-way method (nonempty success).
OPEN_SERVER_TEST(47, FlexibleTwoWayErrorSyntaxNonEmptyResponseSuccessResult) {
  Bytes bytes = {
      Header{
          .txid = kTwoWayTxid,
          .dynamic_flags = kDynamicFlagsFlexible,
          .ordinal = kOrdinal_OpenTarget_FlexibleTwoWayFieldsErr,
      },
      union_ordinal(kResultUnionSuccess),
      inline_envelope(int32(406601)),
  };
  ASSERT_OK(client_end().write(bytes));
  ASSERT_OK(client_end().read_and_check(bytes));
}

// The server should reply to a flexible fallible two-way method (nonempty, error).
OPEN_SERVER_TEST(48, FlexibleTwoWayErrorSyntaxNonEmptyResponseErrorResult) {
  Bytes bytes = {
      Header{
          .txid = kTwoWayTxid,
          .dynamic_flags = kDynamicFlagsFlexible,
          .ordinal = kOrdinal_OpenTarget_FlexibleTwoWayFieldsErr,
      },
      union_ordinal(kResultUnionDomainError),
      inline_envelope(int32(60602293)),
  };
  ASSERT_OK(client_end().write(bytes));
  ASSERT_OK(client_end().read_and_check(bytes));
}

// The server should tear down when it receives an unknown strict one-way method.
OPEN_SERVER_TEST(49, UnknownStrictOneWayOpenProtocol) {
  Bytes request = Header{.txid = 0, .ordinal = kOrdinalFakeUnknownMethod};
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kUnexpectedMessage);
}

// The server should run the unknown method handler when it receives an unknown
// flexible one-way method.
OPEN_SERVER_TEST(50, UnknownFlexibleOneWayOpenProtocol) {
  Bytes request = Header{
      .txid = 0,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinalFakeUnknownMethod,
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_RUNNER_EVENT(RunnerEvent::kOnReceivedUnknownMethod);
  auto info = std::get<fidl_serversuite::UnknownMethodInfo>(runner_event_payload());
  ASSERT_EQ(info.ordinal(), kOrdinalFakeUnknownMethod);
  ASSERT_EQ(info.unknown_method_type(), fidl_serversuite::UnknownMethodType::kOneWay);
}

// The server should close handles in an unknown flexible one-way method request.
OPEN_SERVER_TEST(51, UnknownFlexibleOneWayHandleOpenProtocol) {
  zx::eventpair event1, event2;
  ASSERT_OK(zx::eventpair::create(0, &event1, &event2));

  Message request = {
      Bytes{
          Header{
              .txid = 0,
              .dynamic_flags = kDynamicFlagsFlexible,
              .ordinal = kOrdinalFakeUnknownMethod,
          },
          {handle_present(), padding(4)},
      },
      Handles{
          {.handle = event1.release(), .type = ZX_OBJ_TYPE_EVENTPAIR},
      },
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_RUNNER_EVENT(RunnerEvent::kOnReceivedUnknownMethod);
  auto info = std::get<fidl_serversuite::UnknownMethodInfo>(runner_event_payload());
  ASSERT_EQ(info.ordinal(), kOrdinalFakeUnknownMethod);
  ASSERT_EQ(info.unknown_method_type(), fidl_serversuite::UnknownMethodType::kOneWay);
  ASSERT_OK(event2.wait_one(ZX_EVENTPAIR_PEER_CLOSED, zx::time::infinite_past(), nullptr));
}

// The server should tear down when it receives an unknown strict two-way method.
OPEN_SERVER_TEST(52, UnknownStrictTwoWayOpenProtocol) {
  Bytes request = Header{.txid = kTwoWayTxid, .ordinal = kOrdinalFakeUnknownMethod};
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kUnexpectedMessage);
}

// The server should send an automatic reply and run the unknown method handler
// when it receives an unknown flexible two-way method.
OPEN_SERVER_TEST(53, UnknownFlexibleTwoWayOpenProtocol) {
  Header header = {
      .txid = kTwoWayTxid,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinalFakeUnknownMethod,
  };
  Bytes request = header;
  Bytes expected_response = {
      header,
      union_ordinal(kResultUnionFrameworkError),
      inline_envelope(framework_err_unknown_method()),
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_OK(client_end().read_and_check(expected_response));
  ASSERT_RUNNER_EVENT(RunnerEvent::kOnReceivedUnknownMethod);
  auto info = std::get<fidl_serversuite::UnknownMethodInfo>(runner_event_payload());
  ASSERT_EQ(info.ordinal(), kOrdinalFakeUnknownMethod);
  ASSERT_EQ(info.unknown_method_type(), fidl_serversuite::UnknownMethodType::kTwoWay);
}

// The server should close handles in an unknown flexible two-way method request.
OPEN_SERVER_TEST(54, UnknownFlexibleTwoWayHandleOpenProtocol) {
  zx::eventpair event1, event2;
  ASSERT_OK(zx::eventpair::create(0, &event1, &event2));

  Header header = {
      .txid = kTwoWayTxid,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinalFakeUnknownMethod,
  };
  Message request = {
      Bytes{header, handle_present(), padding(4)},
      Handles{{.handle = event1.release(), .type = ZX_OBJ_TYPE_EVENTPAIR}},
  };
  ExpectedMessage expected_response = {
      Bytes{
          header,
          union_ordinal(kResultUnionFrameworkError),
          inline_envelope(framework_err_unknown_method()),
      },
      ExpectedHandles{},
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_OK(client_end().read_and_check(expected_response));
  ASSERT_RUNNER_EVENT(RunnerEvent::kOnReceivedUnknownMethod);
  auto info = std::get<fidl_serversuite::UnknownMethodInfo>(runner_event_payload());
  ASSERT_EQ(info.ordinal(), kOrdinalFakeUnknownMethod);
  ASSERT_EQ(info.unknown_method_type(), fidl_serversuite::UnknownMethodType::kTwoWay);
  ASSERT_OK(event2.wait_one(ZX_EVENTPAIR_PEER_CLOSED, zx::time::infinite_past(), nullptr));
}

// The ajar server should tear down when it receives an unknown strict one-way method.
AJAR_SERVER_TEST(55, UnknownStrictOneWayAjarProtocol) {
  Bytes request = Header{.txid = 0, .ordinal = kOrdinalFakeUnknownMethod};
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kUnexpectedMessage);
}

// The ajar server should run the unknown method handler when it receives an unknown
// flexible one-way method.
AJAR_SERVER_TEST(56, UnknownFlexibleOneWayAjarProtocol) {
  Bytes request = Header{
      .txid = 0,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinalFakeUnknownMethod,
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_RUNNER_EVENT(RunnerEvent::kOnReceivedUnknownMethod);
  auto info = std::get<fidl_serversuite::UnknownMethodInfo>(runner_event_payload());
  ASSERT_EQ(info.ordinal(), kOrdinalFakeUnknownMethod);
  ASSERT_EQ(info.unknown_method_type(), fidl_serversuite::UnknownMethodType::kOneWay);
}

// The ajar server should tear down when it receives an unknown strict two-way method.
AJAR_SERVER_TEST(57, UnknownStrictTwoWayAjarProtocol) {
  Bytes request = Header{.txid = kTwoWayTxid, .ordinal = kOrdinalFakeUnknownMethod};
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kUnexpectedMessage);
}

// The ajar server should tear down when it receives an unknown flexible two-way method.
AJAR_SERVER_TEST(58, UnknownFlexibleTwoWayAjarProtocol) {
  Bytes request = Header{
      .txid = kTwoWayTxid,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinalFakeUnknownMethod,
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kUnexpectedMessage);
}

// The closed server should tear down when it receives an unknown strict one-way method.
CLOSED_SERVER_TEST(59, UnknownStrictOneWayClosedProtocol) {
  Bytes request = Header{.txid = 0, .ordinal = kOrdinalFakeUnknownMethod};
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kUnexpectedMessage);
}

// The closed server should tear down when it receives an unknown flexible one-way method.
CLOSED_SERVER_TEST(60, UnknownFlexibleOneWayClosedProtocol) {
  Bytes request = Header{
      .txid = 0,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinalFakeUnknownMethod,
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kUnexpectedMessage);
}

// The closed server should tear down when it receives an unknown strict two-way method.
CLOSED_SERVER_TEST(61, UnknownStrictTwoWayClosedProtocol) {
  Bytes request = Header{.txid = kTwoWayTxid, .ordinal = kOrdinalFakeUnknownMethod};
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kUnexpectedMessage);
}

// The closed server should tear down when it receives an unknown flexible two-way method.
CLOSED_SERVER_TEST(62, UnknownFlexibleTwoWayClosedProtocol) {
  Bytes request = Header{
      .txid = kTwoWayTxid,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinalFakeUnknownMethod,
  };
  ASSERT_OK(client_end().write(request));
  ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kUnexpectedMessage);
}

}  // namespace
}  // namespace server_suite
