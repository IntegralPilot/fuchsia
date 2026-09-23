// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/clientsuite/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/fidl/cpp/binding.h>
#include <lib/fidl/cpp/unknown_interactions_hlcpp.h>
#include <lib/sys/cpp/component_context.h>

#include <iostream>

#include "src/tests/fidl/dynsuite/client_suite/runners/hlcpp_util/error_util.h"

class RunnerServer : public fidl::clientsuite::Runner {
 public:
  RunnerServer() = default;

  void GetVersion(GetVersionCallback callback) override {
    callback(fidl::clientsuite::CLIENT_SUITE_VERSION);
  }

  void IsTestEnabled(fidl::clientsuite::Test test, IsTestEnabledCallback callback) override {
    switch (test) {
      // HLCPP Sync Client Bindings do not support events.
      case fidl::clientsuite::Test::RECEIVE_EVENT_NO_PAYLOAD:
      case fidl::clientsuite::Test::RECEIVE_EVENT_STRUCT_PAYLOAD:
      case fidl::clientsuite::Test::RECEIVE_EVENT_TABLE_PAYLOAD:
      case fidl::clientsuite::Test::RECEIVE_EVENT_UNION_PAYLOAD:
      case fidl::clientsuite::Test::RECEIVE_STRICT_EVENT:
      case fidl::clientsuite::Test::RECEIVE_STRICT_EVENT_MISMATCHED_STRICTNESS:
      case fidl::clientsuite::Test::RECEIVE_FLEXIBLE_EVENT:
      case fidl::clientsuite::Test::RECEIVE_FLEXIBLE_EVENT_MISMATCHED_STRICTNESS:
      case fidl::clientsuite::Test::RECEIVE_EVENT_BAD_MAGIC_NUMBER:
      case fidl::clientsuite::Test::RECEIVE_EVENT_UNEXPECTED_TXID:
      case fidl::clientsuite::Test::RECEIVE_EVENT_UNKNOWN_ORDINAL:
      case fidl::clientsuite::Test::UNKNOWN_STRICT_EVENT_OPEN_PROTOCOL:
      case fidl::clientsuite::Test::UNKNOWN_FLEXIBLE_EVENT_OPEN_PROTOCOL:
      case fidl::clientsuite::Test::UNKNOWN_STRICT_EVENT_AJAR_PROTOCOL:
      case fidl::clientsuite::Test::UNKNOWN_FLEXIBLE_EVENT_AJAR_PROTOCOL:
      case fidl::clientsuite::Test::UNKNOWN_STRICT_EVENT_CLOSED_PROTOCOL:
      case fidl::clientsuite::Test::UNKNOWN_FLEXIBLE_EVENT_CLOSED_PROTOCOL:
      case fidl::clientsuite::Test::UNKNOWN_STRICT_SERVER_INITIATED_TWO_WAY:
      case fidl::clientsuite::Test::UNKNOWN_FLEXIBLE_SERVER_INITIATED_TWO_WAY:
      // TODO(https://fxbug.dev/42067516): HLCPP bindings should reject responses with the
      // wrong ordinal.
      case fidl::clientsuite::Test::RECEIVE_RESPONSE_WRONG_ORDINAL_KNOWN:
      case fidl::clientsuite::Test::RECEIVE_RESPONSE_WRONG_ORDINAL_UNKNOWN:
      // TODO(https://fxbug.dev/42082198): HLCPP should validate header even when there's no payload.
      case fidl::clientsuite::Test::V1_TWO_WAY_NO_PAYLOAD:
      // TODO(https://fxbug.dev/42064467): Peer-closed errors should be
      // hidden from one-way calls.
      case fidl::clientsuite::Test::ONE_WAY_CALL_DO_NOT_REPORT_PEER_CLOSED:
      // TODO(https://fxbug.dev/42080230): Should validate magic number.
      case fidl::clientsuite::Test::RECEIVE_RESPONSE_BAD_MAGIC_NUMBER:
        callback(false);
        return;
      default:
        callback(true);
        return;
    }
  }

  void CheckAlive(CheckAliveCallback callback) override { callback(); }

  void GetBindingsProperties(GetBindingsPropertiesCallback callback) override {
    fidl::clientsuite::BindingsProperties properties;
    properties.set_io_style(fidl::clientsuite::IoStyle::SYNC);
    callback(std::move(properties));
  }

  void CallTwoWayNoPayload(fidl::InterfaceHandle<fidl::clientsuite::ClosedTarget> target,
                           CallTwoWayNoPayloadCallback callback) override {
    auto client = target.BindSync();
    auto status = client->TwoWayNoPayload();
    if (status == ZX_OK) {
      callback(fidl::clientsuite::EmptyResultClassification::WithSuccess({}));
    } else {
      callback(fidl::clientsuite::EmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void CallTwoWayStructPayload(fidl::InterfaceHandle<fidl::clientsuite::ClosedTarget> target,
                               CallTwoWayStructPayloadCallback callback) override {
    auto client = target.BindSync();
    int32_t some_field;
    auto status = client->TwoWayStructPayload(&some_field);
    if (status == ZX_OK) {
      fidl::clientsuite::NonEmptyPayload payload;
      payload.some_field = some_field;
      callback(fidl::clientsuite::NonEmptyResultClassification::WithSuccess(std::move(payload)));
    } else {
      callback(fidl::clientsuite::NonEmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void CallTwoWayTablePayload(fidl::InterfaceHandle<fidl::clientsuite::ClosedTarget> target,
                              CallTwoWayTablePayloadCallback callback) override {
    auto client = target.BindSync();
    fidl::clientsuite::TablePayload payload;
    auto status = client->TwoWayTablePayload(&payload);
    if (status == ZX_OK) {
      callback(fidl::clientsuite::TableResultClassification::WithSuccess(std::move(payload)));
    } else {
      callback(fidl::clientsuite::TableResultClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void CallTwoWayUnionPayload(fidl::InterfaceHandle<fidl::clientsuite::ClosedTarget> target,
                              CallTwoWayUnionPayloadCallback callback) override {
    auto client = target.BindSync();
    fidl::clientsuite::UnionPayload payload;
    auto status = client->TwoWayUnionPayload(&payload);
    if (status == ZX_OK) {
      callback(fidl::clientsuite::UnionResultClassification::WithSuccess(std::move(payload)));
    } else {
      callback(fidl::clientsuite::UnionResultClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void CallTwoWayStructPayloadErr(fidl::InterfaceHandle<fidl::clientsuite::ClosedTarget> target,
                                  CallTwoWayStructPayloadErrCallback callback) override {
    auto client = target.BindSync();
    fidl::clientsuite::ClosedTarget_TwoWayStructPayloadErr_Result result;
    auto status = client->TwoWayStructPayloadErr(&result);
    if (status == ZX_OK) {
      if (result.is_response()) {
        callback(fidl::clientsuite::NonEmptyResultWithErrorClassification::WithSuccess(
            std::move(result.response())));
      } else {
        ZX_ASSERT(result.is_err());
        callback(fidl::clientsuite::NonEmptyResultWithErrorClassification::WithApplicationError(
            std::move(result.err())));
      }
    } else {
      callback(fidl::clientsuite::NonEmptyResultWithErrorClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void CallTwoWayStructRequest(fidl::InterfaceHandle<fidl::clientsuite::ClosedTarget> target,
                               fidl::clientsuite::NonEmptyPayload request,
                               CallTwoWayStructRequestCallback callback) override {
    auto client = target.BindSync();
    auto status = client->TwoWayStructRequest(request.some_field);
    if (status == ZX_OK) {
      callback(fidl::clientsuite::EmptyResultClassification::WithSuccess({}));
    } else {
      callback(fidl::clientsuite::EmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void CallTwoWayTableRequest(fidl::InterfaceHandle<fidl::clientsuite::ClosedTarget> target,
                              fidl::clientsuite::TablePayload request,
                              CallTwoWayTableRequestCallback callback) override {
    auto client = target.BindSync();
    auto status = client->TwoWayTableRequest(std::move(request));
    if (status == ZX_OK) {
      callback(fidl::clientsuite::EmptyResultClassification::WithSuccess({}));
    } else {
      callback(fidl::clientsuite::EmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void CallTwoWayUnionRequest(fidl::InterfaceHandle<fidl::clientsuite::ClosedTarget> target,
                              fidl::clientsuite::UnionPayload request,
                              CallTwoWayUnionRequestCallback callback) override {
    auto client = target.BindSync();
    auto status = client->TwoWayUnionRequest(std::move(request));
    if (status == ZX_OK) {
      callback(fidl::clientsuite::EmptyResultClassification::WithSuccess({}));
    } else {
      callback(fidl::clientsuite::EmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void CallOneWayNoRequest(::fidl::InterfaceHandle<::fidl::clientsuite::ClosedTarget> target,
                           CallOneWayNoRequestCallback callback) override {
    auto client = target.BindSync();
    auto status = client->OneWayNoRequest();
    if (status == ZX_OK) {
      callback(fidl::clientsuite::EmptyResultClassification::WithSuccess({}));
    } else {
      callback(fidl::clientsuite::EmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void CallOneWayStructRequest(::fidl::InterfaceHandle<::fidl::clientsuite::ClosedTarget> target,
                               fidl::clientsuite::NonEmptyPayload request,
                               CallOneWayStructRequestCallback callback) override {
    auto client = target.BindSync();
    auto status = client->OneWayStructRequest(request.some_field);
    if (status == ZX_OK) {
      callback(fidl::clientsuite::EmptyResultClassification::WithSuccess({}));
    } else {
      callback(fidl::clientsuite::EmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void CallOneWayTableRequest(::fidl::InterfaceHandle<::fidl::clientsuite::ClosedTarget> target,
                              fidl::clientsuite::TablePayload request,
                              CallOneWayTableRequestCallback callback) override {
    auto client = target.BindSync();
    auto status = client->OneWayTableRequest(std::move(request));
    if (status == ZX_OK) {
      callback(fidl::clientsuite::EmptyResultClassification::WithSuccess({}));
    } else {
      callback(fidl::clientsuite::EmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void CallOneWayUnionRequest(::fidl::InterfaceHandle<::fidl::clientsuite::ClosedTarget> target,
                              fidl::clientsuite::UnionPayload request,
                              CallOneWayUnionRequestCallback callback) override {
    auto client = target.BindSync();
    auto status = client->OneWayUnionRequest(std::move(request));
    if (status == ZX_OK) {
      callback(fidl::clientsuite::EmptyResultClassification::WithSuccess({}));
    } else {
      callback(fidl::clientsuite::EmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void CallStrictOneWay(::fidl::InterfaceHandle<::fidl::clientsuite::OpenTarget> target,
                        CallStrictOneWayCallback callback) override {
    auto client = target.BindSync();
    auto status = client->StrictOneWay();
    if (status == ZX_OK) {
      callback(fidl::clientsuite::EmptyResultClassification::WithSuccess({}));
    } else {
      callback(fidl::clientsuite::EmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void CallFlexibleOneWay(::fidl::InterfaceHandle<::fidl::clientsuite::OpenTarget> target,
                          CallFlexibleOneWayCallback callback) override {
    auto client = target.BindSync();
    auto status = client->FlexibleOneWay();
    if (status == ZX_OK) {
      callback(fidl::clientsuite::EmptyResultClassification::WithSuccess({}));
    } else {
      callback(fidl::clientsuite::EmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void CallStrictTwoWay(::fidl::InterfaceHandle<::fidl::clientsuite::OpenTarget> target,
                        CallStrictTwoWayCallback callback) override {
    auto client = target.BindSync();
    auto status = client->StrictTwoWay();
    if (status == ZX_OK) {
      callback(fidl::clientsuite::EmptyResultClassification::WithSuccess({}));
    } else {
      callback(fidl::clientsuite::EmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void CallStrictTwoWayFields(::fidl::InterfaceHandle<::fidl::clientsuite::OpenTarget> target,
                              CallStrictTwoWayFieldsCallback callback) override {
    auto client = target.BindSync();
    int32_t result;
    auto status = client->StrictTwoWayFields(&result);
    if (status == ZX_OK) {
      callback(fidl::clientsuite::NonEmptyResultClassification::WithSuccess(
          fidl::clientsuite::NonEmptyPayload(result)));
    } else {
      callback(fidl::clientsuite::NonEmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void CallStrictTwoWayErr(::fidl::InterfaceHandle<::fidl::clientsuite::OpenTarget> target,
                           CallStrictTwoWayErrCallback callback) override {
    auto client = target.BindSync();
    fidl::clientsuite::OpenTarget_StrictTwoWayErr_Result result;
    auto status = client->StrictTwoWayErr(&result);
    if (status == ZX_OK) {
      if (result.is_response()) {
        callback(fidl::clientsuite::EmptyResultWithErrorClassification::WithSuccess({}));
      } else if (result.is_err()) {
        callback(fidl::clientsuite::EmptyResultWithErrorClassification::WithApplicationError(
            std::move(result.err())));
      } else {
        ZX_PANIC("invalid tag");
      }
    } else {
      callback(fidl::clientsuite::EmptyResultWithErrorClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void CallStrictTwoWayFieldsErr(::fidl::InterfaceHandle<::fidl::clientsuite::OpenTarget> target,
                                 CallStrictTwoWayFieldsErrCallback callback) override {
    auto client = target.BindSync();
    fidl::clientsuite::OpenTarget_StrictTwoWayFieldsErr_Result result;
    auto status = client->StrictTwoWayFieldsErr(&result);
    if (status == ZX_OK) {
      if (result.is_response()) {
        callback(fidl::clientsuite::NonEmptyResultWithErrorClassification::WithSuccess(
            std::move(result.response())));
      } else if (result.is_err()) {
        callback(fidl::clientsuite::NonEmptyResultWithErrorClassification::WithApplicationError(
            std::move(result.err())));
      } else {
        ZX_PANIC("invalid tag");
      }
    } else {
      callback(fidl::clientsuite::NonEmptyResultWithErrorClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void CallFlexibleTwoWay(::fidl::InterfaceHandle<::fidl::clientsuite::OpenTarget> target,
                          CallFlexibleTwoWayCallback callback) override {
    auto client = target.BindSync();
    fidl::clientsuite::OpenTarget_FlexibleTwoWay_Result result;
    auto status = client->FlexibleTwoWay(&result);
    if (status == ZX_OK) {
      if (result.is_response()) {
        callback(fidl::clientsuite::EmptyResultClassification::WithSuccess({}));
      } else if (result.is_framework_err()) {
        ZX_ASSERT(result.framework_err() == fidl::FrameworkErr::kUnknownMethod);
        callback(fidl::clientsuite::EmptyResultClassification::WithFidlError(
            fidl::clientsuite::FidlErrorKind::UNKNOWN_METHOD));
      } else {
        ZX_PANIC("invalid tag");
      }
    } else {
      callback(fidl::clientsuite::EmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void CallFlexibleTwoWayFields(::fidl::InterfaceHandle<::fidl::clientsuite::OpenTarget> target,
                                CallFlexibleTwoWayFieldsCallback callback) override {
    auto client = target.BindSync();
    fidl::clientsuite::OpenTarget_FlexibleTwoWayFields_Result result;
    auto status = client->FlexibleTwoWayFields(&result);
    if (status == ZX_OK) {
      if (result.is_response()) {
        callback(fidl::clientsuite::NonEmptyResultClassification::WithSuccess(
            std::move(result.response())));
      } else if (result.is_framework_err()) {
        ZX_ASSERT(result.framework_err() == fidl::FrameworkErr::kUnknownMethod);
        callback(fidl::clientsuite::NonEmptyResultClassification::WithFidlError(
            fidl::clientsuite::FidlErrorKind::UNKNOWN_METHOD));
      } else {
        ZX_PANIC("invalid tag");
      }
    } else {
      callback(fidl::clientsuite::NonEmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void CallFlexibleTwoWayErr(::fidl::InterfaceHandle<::fidl::clientsuite::OpenTarget> target,
                             CallFlexibleTwoWayErrCallback callback) override {
    auto client = target.BindSync();
    fidl::clientsuite::OpenTarget_FlexibleTwoWayErr_Result result;
    auto status = client->FlexibleTwoWayErr(&result);
    if (status == ZX_OK) {
      if (result.is_response()) {
        callback(fidl::clientsuite::EmptyResultWithErrorClassification::WithSuccess({}));
      } else if (result.is_err()) {
        callback(fidl::clientsuite::EmptyResultWithErrorClassification::WithApplicationError(
            std::move(result.err())));
      } else if (result.is_framework_err()) {
        ZX_ASSERT(result.framework_err() == fidl::FrameworkErr::kUnknownMethod);
        callback(fidl::clientsuite::EmptyResultWithErrorClassification::WithFidlError(
            fidl::clientsuite::FidlErrorKind::UNKNOWN_METHOD));
      } else {
        ZX_PANIC("invalid tag");
      }
    } else {
      callback(fidl::clientsuite::EmptyResultWithErrorClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void CallFlexibleTwoWayFieldsErr(::fidl::InterfaceHandle<::fidl::clientsuite::OpenTarget> target,
                                   CallFlexibleTwoWayFieldsErrCallback callback) override {
    auto client = target.BindSync();
    fidl::clientsuite::OpenTarget_FlexibleTwoWayFieldsErr_Result result;
    auto status = client->FlexibleTwoWayFieldsErr(&result);
    if (status == ZX_OK) {
      if (result.is_response()) {
        callback(fidl::clientsuite::NonEmptyResultWithErrorClassification::WithSuccess(
            std::move(result.response())));
      } else if (result.is_err()) {
        callback(fidl::clientsuite::NonEmptyResultWithErrorClassification::WithApplicationError(
            std::move(result.err())));
      } else if (result.is_framework_err()) {
        ZX_ASSERT(result.framework_err() == fidl::FrameworkErr::kUnknownMethod);
        callback(fidl::clientsuite::NonEmptyResultWithErrorClassification::WithFidlError(
            fidl::clientsuite::FidlErrorKind::UNKNOWN_METHOD));
      } else {
        ZX_PANIC("invalid tag");
      }
    } else {
      callback(fidl::clientsuite::NonEmptyResultWithErrorClassification::WithFidlError(
          clienttest_util::ClassifyError(status)));
    }
  }

  void ReceiveClosedEvents(
      ::fidl::InterfaceHandle<::fidl::clientsuite::ClosedTarget> target,
      ::fidl::InterfaceHandle<::fidl::clientsuite::ClosedTargetEventReporter> reporter,
      ReceiveClosedEventsCallback callback) override {
    ZX_PANIC("Events are not supported on HLCPP Sync Client Bindings");
  }

  void ReceiveAjarEvents(
      ::fidl::InterfaceHandle<::fidl::clientsuite::AjarTarget> target,
      ::fidl::InterfaceHandle<::fidl::clientsuite::AjarTargetEventReporter> reporter,
      ReceiveAjarEventsCallback callback) override {
    ZX_PANIC("Events are not supported on HLCPP Sync Client Bindings");
  }

  void ReceiveOpenEvents(
      ::fidl::InterfaceHandle<::fidl::clientsuite::OpenTarget> target,
      ::fidl::InterfaceHandle<::fidl::clientsuite::OpenTargetEventReporter> reporter,
      ReceiveOpenEventsCallback callback) override {
    ZX_PANIC("Events are not supported on HLCPP Sync Client Bindings");
  }
};

int main(int argc, const char** argv) {
  std::cout << "HLCPP sync client: main" << std::endl;
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  RunnerServer server;
  fidl::Binding<fidl::clientsuite::Runner> binding(&server);
  fidl::InterfaceRequestHandler<fidl::clientsuite::Runner> handler =
      [&](fidl::InterfaceRequest<fidl::clientsuite::Runner> server_end) {
        binding.Bind(std::move(server_end));
      };
  auto context = sys::ComponentContext::CreateAndServeOutgoingDirectory();
  context->outgoing()->AddPublicService(std::move(handler));

  std::cout << "HLCPP sync client: ready!" << std::endl;
  return loop.Run();
}
