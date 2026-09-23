// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.clientsuite/cpp/common_types.h>
#include <fidl/fidl.clientsuite/cpp/natural_types.h>

#include "src/tests/fidl/dynsuite/channel_util/bytes.h"
#include "src/tests/fidl/dynsuite/client_suite/harness/harness.h"
#include "src/tests/fidl/dynsuite/client_suite/harness/ordinals.h"

namespace client_suite {
namespace {

using namespace ::channel_util;

// The client should call a strict one-way method.
CLIENT_TEST(4, OneWayStrictSend) {
  Bytes expected_request = Header{.txid = kOneWayTxid, .ordinal = kOrdinal_OpenTarget_StrictOneWay};
  runner()->CallStrictOneWay({{.target = TakeOpenClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().success().has_value());
  });
  ASSERT_OK(server_end().read_and_check(expected_request));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a flexible one-way method.
CLIENT_TEST(5, OneWayFlexibleSend) {
  Bytes expected_request = Header{
      .txid = kOneWayTxid,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_FlexibleOneWay,
  };
  runner()->CallFlexibleOneWay({{.target = TakeOpenClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().success().has_value());
  });
  ASSERT_OK(server_end().read_and_check(expected_request));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a strict two-way method and receive the response.
CLIENT_TEST(6, TwoWayStrictSend) {
  Bytes bytes = Header{.txid = kTxidNotKnown, .ordinal = kOrdinal_OpenTarget_StrictTwoWay};
  runner()->CallStrictTwoWay({{.target = TakeOpenClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().success().has_value());
  });
  ASSERT_OK(server_end().read_and_check_unknown_txid(bytes, &bytes.txid()));
  ASSERT_NE(bytes.txid(), 0u);
  ASSERT_OK(server_end().write(bytes));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a strict two-way method and receive the response,
// despite the schema (strict) not matching the response's dynamic flags (flexible).
CLIENT_TEST(7, TwoWayStrictSendMismatchedStrictness) {
  Bytes expected_request = Header{
      .txid = kTxidNotKnown,
      .ordinal = kOrdinal_OpenTarget_StrictTwoWay,
  };
  Bytes response = Header{
      .txid = kTxidNotKnown,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_StrictTwoWay,
  };
  runner()->CallStrictTwoWay({{.target = TakeOpenClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().success().has_value());
  });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a strict two-way method and receive the nonempty response.
CLIENT_TEST(38, TwoWayStrictSendNonEmptyPayload) {
  Header header = {.txid = kTxidNotKnown, .ordinal = kOrdinal_OpenTarget_StrictTwoWayFields};
  fidl_clientsuite::NonEmptyPayload payload = {{.some_field = 541768}};
  Bytes expected_request = header;
  Bytes response = {header, encode(payload)};
  runner()
      ->CallStrictTwoWayFields({{.target = TakeOpenClient()}})
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

// The client should call a strict fallible two-way method and receive the success response.
CLIENT_TEST(8, TwoWayStrictErrorSyntaxSendSuccessResponse) {
  Header header = {.txid = kTxidNotKnown, .ordinal = kOrdinal_OpenTarget_StrictTwoWayErr};
  Bytes expected_request = header;
  Bytes response = {header, union_ordinal(kResultUnionSuccess), inline_envelope({0x00})};
  runner()->CallStrictTwoWayErr({{.target = TakeOpenClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().success().has_value());
  });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a strict fallible two-way method and receive the error response.
CLIENT_TEST(9, TwoWayStrictErrorSyntaxSendErrorResponse) {
  Header header = {.txid = kTxidNotKnown, .ordinal = kOrdinal_OpenTarget_StrictTwoWayErr};
  int32_t error = 39243320;
  Bytes expected_request = header;
  Bytes response = {header, union_ordinal(kResultUnionDomainError), inline_envelope(int32(error))};
  runner()->CallStrictTwoWayErr({{.target = TakeOpenClient()}}).ThenExactlyOnce([&](auto result) {
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

// The client should tear down when it calls a strict fallible two-way method and
// receives an "unknown method" response (with strict dynamic flag).
CLIENT_TEST(10, TwoWayStrictErrorSyntaxSendUnknownMethodResponse) {
  Header header = {.txid = kTxidNotKnown, .ordinal = kOrdinal_OpenTarget_StrictTwoWayErr};
  Bytes expected_request = header;
  Bytes response = {
      header,
      union_ordinal(kResultUnionFrameworkError),
      inline_envelope(int32(ZX_ERR_NOT_SUPPORTED)),
  };
  runner()->CallStrictTwoWayErr({{.target = TakeOpenClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().fidl_error().has_value());
    ASSERT_EQ(result.value().fidl_error().value(), fidl_clientsuite::FidlErrorKind::kDecodingError);
  });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should tear down when it calls a strict fallible two-way method
// and receives an "unknown method" response (with flexible dynamic flag).
CLIENT_TEST(11, TwoWayStrictErrorSyntaxSendMismatchedStrictnessUnknownMethodResponse) {
  Bytes expected_request = Header{
      .txid = kTxidNotKnown,
      .ordinal = kOrdinal_OpenTarget_StrictTwoWayErr,
  };
  Bytes response = {
      Header{
          .txid = kTxidNotKnown,
          .dynamic_flags = kDynamicFlagsFlexible,
          .ordinal = kOrdinal_OpenTarget_StrictTwoWayErr,
      },
      union_ordinal(kResultUnionFrameworkError),
      inline_envelope(int32(ZX_ERR_NOT_SUPPORTED)),
  };
  runner()->CallStrictTwoWayErr({{.target = TakeOpenClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().fidl_error().has_value());
    ASSERT_EQ(result.value().fidl_error().value(), fidl_clientsuite::FidlErrorKind::kDecodingError);
  });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a strict fallible two-way method and receive the
// nonempty success response.
CLIENT_TEST(39, TwoWayStrictErrorSyntaxSendNonEmptyPayload) {
  Header header = {.txid = kTxidNotKnown, .ordinal = kOrdinal_OpenTarget_StrictTwoWayFieldsErr};
  fidl_clientsuite::NonEmptyPayload payload = {{.some_field = 394966}};
  Bytes expected_request = header;
  Bytes response = {header, union_ordinal(kResultUnionSuccess), inline_envelope(int32(394966))};
  runner()
      ->CallStrictTwoWayFieldsErr({{.target = TakeOpenClient()}})
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

// The client should call a flexible two-way method and receive the empty response.
CLIENT_TEST(12, TwoWayFlexibleSendSuccessResponse) {
  Header header = {
      .txid = kTxidNotKnown,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_FlexibleTwoWay,
  };
  Bytes expected_request = header;
  Bytes response = {header, union_ordinal(kResultUnionSuccess), inline_envelope({0x00})};
  runner()->CallFlexibleTwoWay({{.target = TakeOpenClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().success().has_value());
  });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should tear down when it calls a flexible two-way method and receives
// a domain error response, which is invalid for a method without error syntax.
CLIENT_TEST(13, TwoWayFlexibleSendErrorResponse) {
  Header header = {
      .txid = kTxidNotKnown,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_FlexibleTwoWay,
  };
  Bytes expected_request = header;
  Bytes response = {
      header,
      union_ordinal(kResultUnionDomainError),
      inline_envelope(int32(39205950)),
  };
  runner()->CallFlexibleTwoWay({{.target = TakeOpenClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().fidl_error().has_value());
    ASSERT_EQ(result.value().fidl_error().value(), fidl_clientsuite::FidlErrorKind::kDecodingError);
  });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a flexible two-way method and accept the "unknown method" response.
CLIENT_TEST(14, TwoWayFlexibleSendUnknownMethodResponse) {
  Header header = {
      .txid = kTxidNotKnown,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_FlexibleTwoWay,
  };
  Bytes expected_request = header;
  Bytes response = {
      header,
      union_ordinal(kResultUnionFrameworkError),
      inline_envelope(int32(ZX_ERR_NOT_SUPPORTED)),
  };
  runner()->CallFlexibleTwoWay({{.target = TakeOpenClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().fidl_error().has_value());
    ASSERT_EQ(result.value().fidl_error().value(), fidl_clientsuite::FidlErrorKind::kUnknownMethod);
  });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// TODO(https://fxbug.dev/42083366): This test is incorrect. The client should tear down
// because the response message is inconsistent. Once fixed, comment should be:
// > The client should tear down when it calls a flexible two-way method and
// > receives an "unknown method" response (with strict dynamic flag).
CLIENT_TEST(15, TwoWayFlexibleSendMismatchedStrictnessUnknownMethodResponse) {
  Bytes expected_request = Header{
      .txid = kTxidNotKnown,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_FlexibleTwoWay,
  };
  Bytes response = {
      Header{.txid = kTxidNotKnown, .ordinal = kOrdinal_OpenTarget_FlexibleTwoWay},
      union_ordinal(kResultUnionFrameworkError),
      inline_envelope(int32(ZX_ERR_NOT_SUPPORTED)),
  };
  runner()->CallFlexibleTwoWay({{.target = TakeOpenClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().fidl_error().has_value());
    ASSERT_EQ(result.value().fidl_error().value(), fidl_clientsuite::FidlErrorKind::kUnknownMethod);
  });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should tear down when it calls a flexible two-way method and
// receives a framework error response other than "unsupported method".
CLIENT_TEST(16, TwoWayFlexibleSendOtherFrameworkErrResponse) {
  Header header = {
      .txid = kTxidNotKnown,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_FlexibleTwoWay,
  };
  Bytes expected_request = header;
  Bytes response = {
      header,
      union_ordinal(kResultUnionFrameworkError),
      inline_envelope(int32(ZX_ERR_ACCESS_DENIED)),
  };
  runner()->CallFlexibleTwoWay({{.target = TakeOpenClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().fidl_error().has_value());
    ASSERT_EQ(result.value().fidl_error().value(), fidl_clientsuite::FidlErrorKind::kDecodingError);
  });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a flexible two-way method and receive the nonempty response.
CLIENT_TEST(17, TwoWayFlexibleSendNonEmptyPayloadSuccessResponse) {
  Header header = {
      .txid = kTxidNotKnown,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_FlexibleTwoWayFields,
  };
  int32_t some_field = 302340665;
  fidl_clientsuite::NonEmptyPayload payload = {{.some_field = some_field}};
  Bytes expected_request = header;
  Bytes response = {header, union_ordinal(kResultUnionSuccess), inline_envelope(int32(some_field))};
  runner()
      ->CallFlexibleTwoWayFields({{.target = TakeOpenClient()}})
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

// The client should call a flexible two-way method whose response is nonempty,
// and accept the "unknown method" response.
CLIENT_TEST(18, TwoWayFlexibleSendNonEmptyPayloadUnknownMethodResponse) {
  Header header = {
      .txid = kTxidNotKnown,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_FlexibleTwoWayFields,
  };
  Bytes expected_request = header;
  Bytes response = {
      header,
      union_ordinal(kResultUnionFrameworkError),
      inline_envelope(int32(ZX_ERR_NOT_SUPPORTED)),
  };
  runner()
      ->CallFlexibleTwoWayFields({{.target = TakeOpenClient()}})
      .ThenExactlyOnce([&](auto result) {
        MarkCallbackRun();
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result.value().fidl_error().has_value());
        ASSERT_EQ(result.value().fidl_error().value(),
                  fidl_clientsuite::FidlErrorKind::kUnknownMethod);
      });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a flexible fallible two-way method and receive the success response.
CLIENT_TEST(19, TwoWayFlexibleErrorSyntaxSendSuccessResponse) {
  Header header = {
      .txid = kTxidNotKnown,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_FlexibleTwoWayErr,
  };
  Bytes expected_request = header;
  Bytes response = {header, union_ordinal(kResultUnionSuccess), inline_envelope({0x00})};
  runner()->CallFlexibleTwoWayErr({{.target = TakeOpenClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().success().has_value());
  });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a flexible fallible two-way method and receive the error response.
CLIENT_TEST(20, TwoWayFlexibleErrorSyntaxSendErrorResponse) {
  Header header = {
      .txid = kTxidNotKnown,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_FlexibleTwoWayErr,
  };
  int32_t error = 1456681;
  Bytes expected_request = header;
  Bytes response = {header, union_ordinal(kResultUnionDomainError), inline_envelope(int32(error))};
  runner()->CallFlexibleTwoWayErr({{.target = TakeOpenClient()}}).ThenExactlyOnce([&](auto result) {
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

// The client should call a flexible fallible two-way method accept the "unknown method" response.
CLIENT_TEST(21, TwoWayFlexibleErrorSyntaxSendUnknownMethodResponse) {
  Header header = {
      .txid = kTxidNotKnown,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_FlexibleTwoWayErr,
  };
  Bytes expected_request = header;
  Bytes response = {
      header,
      union_ordinal(kResultUnionFrameworkError),
      inline_envelope(int32(ZX_ERR_NOT_SUPPORTED)),
  };
  runner()->CallFlexibleTwoWayErr({{.target = TakeOpenClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().fidl_error().has_value());
    ASSERT_EQ(result.value().fidl_error().value(), fidl_clientsuite::FidlErrorKind::kUnknownMethod);
  });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// TODO(https://fxbug.dev/42083366): This test is incorrect. The client should tear down
// because the response message is inconsistent. Once fixed, comment should be:
// > The client should tear down when it calls a flexible fallible two-way method
// > and receives an "unknown method" response (with strict dynamic flag).
CLIENT_TEST(22, TwoWayFlexibleErrorSyntaxSendMismatchedStrictnessUnknownMethodResponse) {
  Bytes expected_request = Header{
      .txid = kTxidNotKnown,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_FlexibleTwoWayErr,
  };
  Bytes response = {
      Header{.txid = kTxidNotKnown, .ordinal = kOrdinal_OpenTarget_FlexibleTwoWayErr},
      union_ordinal(kResultUnionFrameworkError),
      inline_envelope(int32(ZX_ERR_NOT_SUPPORTED)),
  };
  runner()->CallFlexibleTwoWayErr({{.target = TakeOpenClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().fidl_error().has_value());
    ASSERT_EQ(result.value().fidl_error().value(), fidl_clientsuite::FidlErrorKind::kUnknownMethod);
  });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should tear down when it calls a flexible fallible two-way method
// and receives a framework error response other than "unsupported method".
CLIENT_TEST(23, TwoWayFlexibleErrorSyntaxSendOtherFrameworkErrResponse) {
  Header header = {
      .txid = kTxidNotKnown,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_FlexibleTwoWayErr,
  };
  Bytes expected_request = header;
  Bytes response = {
      header,
      union_ordinal(kResultUnionFrameworkError),
      inline_envelope(int32(ZX_ERR_ACCESS_DENIED)),
  };
  runner()->CallFlexibleTwoWayErr({{.target = TakeOpenClient()}}).ThenExactlyOnce([&](auto result) {
    MarkCallbackRun();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().fidl_error().has_value());
    ASSERT_EQ(result.value().fidl_error().value(), fidl_clientsuite::FidlErrorKind::kDecodingError);
  });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should call a flexible fallible two-way method and receive
// the nonempty success response.
CLIENT_TEST(24, TwoWayFlexibleErrorSyntaxSendNonEmptyPayloadSuccessResponse) {
  Header header = {
      .txid = kTxidNotKnown,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_FlexibleTwoWayFieldsErr,
  };
  int32_t some_field = 670705054;
  fidl_clientsuite::NonEmptyPayload payload = {{.some_field = some_field}};
  Bytes expected_request = header;
  Bytes response = {header, union_ordinal(kResultUnionSuccess), inline_envelope(int32(some_field))};
  runner()
      ->CallFlexibleTwoWayFieldsErr({{.target = TakeOpenClient()}})
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

// The client should call a flexible fallible two-way method whose response is
// nonempty, and accept the "unknown method" response.
CLIENT_TEST(25, TwoWayFlexibleErrorSyntaxSendNonEmptyPayloadUnknownMethodResponse) {
  Header header = {
      .txid = kTxidNotKnown,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_FlexibleTwoWayFieldsErr,
  };
  Bytes expected_request = header;
  Bytes response = {
      header,
      union_ordinal(kResultUnionFrameworkError),
      inline_envelope(int32(ZX_ERR_NOT_SUPPORTED)),
  };
  runner()
      ->CallFlexibleTwoWayFieldsErr({{.target = TakeOpenClient()}})
      .ThenExactlyOnce([&](auto result) {
        MarkCallbackRun();
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result.value().fidl_error().has_value());
        ASSERT_EQ(result.value().fidl_error().value(),
                  fidl_clientsuite::FidlErrorKind::kUnknownMethod);
      });
  ASSERT_OK(server_end().read_and_check_unknown_txid(expected_request, &response.txid()));
  ASSERT_NE(response.txid(), 0u);
  ASSERT_OK(server_end().write(response));
  WAIT_UNTIL_CALLBACK_RUN();
}

// The client should receive a strict event.
CLIENT_TEST(26, ReceiveStrictEvent) {
  Bytes event = Header{.txid = kOneWayTxid, .ordinal = kOrdinal_OpenTarget_StrictEvent};
  auto reporter = ReceiveOpenEvents();
  ASSERT_NE(reporter, nullptr);
  ASSERT_OK(server_end().write(event));
  WAIT_UNTIL([reporter]() { return reporter->NumReceivedEvents(); });
  ASSERT_EQ(reporter->NumReceivedEvents(), 1u);
  auto reporter_event = reporter->TakeNextEvent();
  ASSERT_TRUE(reporter_event.strict_event().has_value());
  ASSERT_FALSE(server_end().is_signal_present(ZX_CHANNEL_PEER_CLOSED));
}

// The client should receive an event, despite the schema (strict) not matching
// the dynamic flags (flexible).
CLIENT_TEST(27, ReceiveStrictEventMismatchedStrictness) {
  Bytes event = Header{
      .txid = kOneWayTxid,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_StrictEvent,
  };
  auto reporter = ReceiveOpenEvents();
  ASSERT_NE(reporter, nullptr);
  ASSERT_OK(server_end().write(event));
  WAIT_UNTIL([reporter]() { return reporter->NumReceivedEvents(); });
  ASSERT_EQ(reporter->NumReceivedEvents(), 1u);
  auto reporter_event = reporter->TakeNextEvent();
  ASSERT_TRUE(reporter_event.strict_event().has_value());
  ASSERT_FALSE(server_end().is_signal_present(ZX_CHANNEL_PEER_CLOSED));
}

// The client should receive a flexible event.
CLIENT_TEST(28, ReceiveFlexibleEvent) {
  Bytes event = Header{
      .txid = kOneWayTxid,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinal_OpenTarget_FlexibleEvent,
  };
  auto reporter = ReceiveOpenEvents();
  ASSERT_NE(reporter, nullptr);
  ASSERT_OK(server_end().write(event));
  WAIT_UNTIL([reporter]() { return reporter->NumReceivedEvents(); });
  ASSERT_EQ(reporter->NumReceivedEvents(), 1u);
  auto reporter_event = reporter->TakeNextEvent();
  ASSERT_TRUE(reporter_event.flexible_event().has_value());
  ASSERT_FALSE(server_end().is_signal_present(ZX_CHANNEL_PEER_CLOSED));
}

// The client should receive an event, despite the schema (flexible) not
// matching the dynamic flags (strict).
CLIENT_TEST(29, ReceiveFlexibleEventMismatchedStrictness) {
  Bytes event = Header{.txid = kOneWayTxid, .ordinal = kOrdinal_OpenTarget_FlexibleEvent};
  auto reporter = ReceiveOpenEvents();
  ASSERT_NE(reporter, nullptr);
  ASSERT_OK(server_end().write(event));
  WAIT_UNTIL([reporter]() { return reporter->NumReceivedEvents(); });
  ASSERT_EQ(reporter->NumReceivedEvents(), 1u);
  auto reporter_event = reporter->TakeNextEvent();
  ASSERT_TRUE(reporter_event.flexible_event().has_value());
  ASSERT_FALSE(server_end().is_signal_present(ZX_CHANNEL_PEER_CLOSED));
}

// The open client should tear down when it receives an unknown strict event.
CLIENT_TEST(30, UnknownStrictEventOpenProtocol) {
  Bytes event = Header{.txid = kOneWayTxid, .ordinal = kOrdinalFakeUnknownMethod};
  auto reporter = ReceiveOpenEvents();
  ASSERT_NE(reporter, nullptr);
  ASSERT_OK(server_end().write(event));
  WAIT_UNTIL([reporter]() { return reporter->NumReceivedEvents(); });
  ASSERT_EQ(reporter->NumReceivedEvents(), 1u);
  auto reporter_event = reporter->TakeNextEvent();
  ASSERT_TRUE(reporter_event.fidl_error().has_value());
  ASSERT_EQ(reporter_event.fidl_error().value(),
            fidl_clientsuite::FidlErrorKind::kUnexpectedMessage);

  // TODO(https://fxbug.dev/42159083, https://fxbug.dev/42153903): Clients should close the channel
  // when they receive an unsupported unknown event, but many of them don't
  // actually.
  // ASSERT_TRUE(server_end().is_signal_present(ZX_CHANNEL_PEER_CLOSED));
}

// The open client should accept an unknown flexible event.
CLIENT_TEST(31, UnknownFlexibleEventOpenProtocol) {
  Bytes event = Header{
      .txid = kOneWayTxid,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinalFakeUnknownMethod,
  };
  auto reporter = ReceiveOpenEvents();
  ASSERT_NE(reporter, nullptr);
  ASSERT_OK(server_end().write(event));
  WAIT_UNTIL([reporter]() { return reporter->NumReceivedEvents(); });
  ASSERT_EQ(reporter->NumReceivedEvents(), 1u);
  auto reporter_event = reporter->TakeNextEvent();
  ASSERT_TRUE(reporter_event.unknown_event().has_value());
  ASSERT_EQ(reporter_event.unknown_event().value(),
            fidl_clientsuite::UnknownEvent(kOrdinalFakeUnknownMethod));
  ASSERT_FALSE(server_end().is_signal_present(ZX_CHANNEL_PEER_CLOSED));
}

// The ajar client should tear down when it receives an unknown strict event.
CLIENT_TEST(32, UnknownStrictEventAjarProtocol) {
  Bytes event = Header{.txid = kOneWayTxid, .ordinal = kOrdinalFakeUnknownMethod};
  auto reporter = ReceiveAjarEvents();
  ASSERT_NE(reporter, nullptr);
  ASSERT_OK(server_end().write(event));
  WAIT_UNTIL([reporter]() { return reporter->NumReceivedEvents(); });
  ASSERT_EQ(reporter->NumReceivedEvents(), 1u);
  auto reporter_event = reporter->TakeNextEvent();
  ASSERT_TRUE(reporter_event.fidl_error().has_value());
  ASSERT_EQ(reporter_event.fidl_error().value(),
            fidl_clientsuite::FidlErrorKind::kUnexpectedMessage);

  // TODO(https://fxbug.dev/42159083, https://fxbug.dev/42153903): Clients should close the channel
  // when they receive an unsupported unknown event, but many of them don't
  // actually.
  // ASSERT_TRUE(server_end().is_signal_present(ZX_CHANNEL_PEER_CLOSED));
}

// The ajar client should accept an unknown flexible event.
CLIENT_TEST(33, UnknownFlexibleEventAjarProtocol) {
  Bytes event = Header{
      .txid = kOneWayTxid,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinalFakeUnknownMethod,
  };
  auto reporter = ReceiveAjarEvents();
  ASSERT_NE(reporter, nullptr);
  ASSERT_OK(server_end().write(event));
  WAIT_UNTIL([reporter]() { return reporter->NumReceivedEvents(); });
  ASSERT_EQ(reporter->NumReceivedEvents(), 1u);
  auto reporter_event = reporter->TakeNextEvent();
  ASSERT_TRUE(reporter_event.unknown_event().has_value());
  ASSERT_EQ(reporter_event.unknown_event().value(),
            fidl_clientsuite::UnknownEvent(kOrdinalFakeUnknownMethod));
  ASSERT_FALSE(server_end().is_signal_present(ZX_CHANNEL_PEER_CLOSED));
}

// The closed client should tear down when it receives an unknown strict event.
CLIENT_TEST(34, UnknownStrictEventClosedProtocol) {
  Bytes event = Header{.txid = kOneWayTxid, .ordinal = kOrdinalFakeUnknownMethod};
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
  // when they receive an unsupported unknown event, but many of them don't
  // actually.
  // ASSERT_TRUE(server_end().is_signal_present(ZX_CHANNEL_PEER_CLOSED));
}

// The closed client should tear down when it receives an unknown flexible event.
CLIENT_TEST(35, UnknownFlexibleEventClosedProtocol) {
  Bytes event = Header{
      .txid = kOneWayTxid,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinalFakeUnknownMethod,
  };
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
  // when they receive an unsupported unknown event, but many of them don't
  // actually.
  // ASSERT_TRUE(server_end().is_signal_present(ZX_CHANNEL_PEER_CLOSED));
}

// The client should tear down when it receives an unsolicited strict message
// with nonzero txid and an unknown ordinal.
CLIENT_TEST(36, UnknownStrictServerInitiatedTwoWay) {
  Bytes bytes = Header{.txid = kTwoWayTxid, .ordinal = kOrdinalFakeUnknownMethod};
  auto reporter = ReceiveOpenEvents();
  ASSERT_NE(reporter, nullptr);
  ASSERT_OK(server_end().write(bytes));
  WAIT_UNTIL([reporter]() { return reporter->NumReceivedEvents(); });
  ASSERT_EQ(reporter->NumReceivedEvents(), 1u);
  auto reporter_event = reporter->TakeNextEvent();
  ASSERT_TRUE(reporter_event.fidl_error().has_value());
  ASSERT_EQ(reporter_event.fidl_error().value(),
            fidl_clientsuite::FidlErrorKind::kUnexpectedMessage);

  // TODO(https://fxbug.dev/42159083, https://fxbug.dev/42153903): Clients should close the channel
  // when they receive an unsupported unknown event, but many of them don't
  // actually.
  // ASSERT_TRUE(server_end().is_signal_present(ZX_CHANNEL_PEER_CLOSED));
}

// The client should tear down when it receives an unsolicited flexible message
// with nonzero txid and an unknown ordinal.
CLIENT_TEST(37, UnknownFlexibleServerInitiatedTwoWay) {
  Bytes bytes = Header{
      .txid = kTwoWayTxid,
      .dynamic_flags = kDynamicFlagsFlexible,
      .ordinal = kOrdinalFakeUnknownMethod,
  };
  auto reporter = ReceiveOpenEvents();
  ASSERT_NE(reporter, nullptr);
  ASSERT_OK(server_end().write(bytes));
  WAIT_UNTIL([reporter]() { return reporter->NumReceivedEvents(); });
  ASSERT_EQ(reporter->NumReceivedEvents(), 1u);
  auto reporter_event = reporter->TakeNextEvent();
  ASSERT_TRUE(reporter_event.fidl_error().has_value());
  ASSERT_EQ(reporter_event.fidl_error().value(),
            fidl_clientsuite::FidlErrorKind::kUnexpectedMessage);

  // TODO(https://fxbug.dev/42159083, https://fxbug.dev/42153903): Clients should close the channel
  // when they receive an unsupported unknown event, but many of them don't
  // actually.
  // ASSERT_TRUE(server_end().is_signal_present(ZX_CHANNEL_PEER_CLOSED));
}

}  // namespace
}  // namespace client_suite
