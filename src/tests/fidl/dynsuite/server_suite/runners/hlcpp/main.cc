// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/serversuite/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async-loop/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/binding.h>
#include <lib/fidl/cpp/event_sender.h>
#include <lib/fidl/cpp/synchronous_interface_ptr.h>
#include <lib/fit/function.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include <iostream>
#include <set>
#include <variant>

namespace {

const std::set<fidl::serversuite::Test> kDisabledTests = {
    // This is for testing the test disabling functionality itself.
    fidl::serversuite::Test::IGNORE_DISABLED,
    // TODO(https://fxbug.dev/42083300): Should close the channel when replying fails.
    fidl::serversuite::Test::SERVER_SENDS_WRONG_HANDLE_TYPE,
    fidl::serversuite::Test::SERVER_SENDS_TOO_FEW_RIGHTS,
    fidl::serversuite::Test::RESPONSE_EXCEEDS_BYTE_LIMIT,
    fidl::serversuite::Test::RESPONSE_EXCEEDS_HANDLE_LIMIT,
    fidl::serversuite::Test::SERVER_SENDS_WRONG_HANDLE_TYPE,
    // TODO(https://fxbug.dev/42082198): Should validate the header even when there's no payload.
    fidl::serversuite::Test::V1_TWO_WAY_NO_PAYLOAD,
    // TODO(fxubg.dev/133377): Server should not be allowed to receive epitaphs.
    fidl::serversuite::Test::SERVER_RECEIVES_EPITAPH_INVALID,
};

fidl::serversuite::TeardownReason GetTeardownReason(zx_status_t error) {
  switch (error) {
    case ZX_ERR_PEER_CLOSED:
      return fidl::serversuite::TeardownReason::PEER_CLOSED;
    case ZX_ERR_INVALID_ARGS:
      // ZX_ERR_INVALID_ARGS could mean any of these.
      return fidl::serversuite::TeardownReason::ENCODE_FAILURE |
             fidl::serversuite::TeardownReason::DECODE_FAILURE |
             fidl::serversuite::TeardownReason::INCOMPATIBLE_FORMAT |
             fidl::serversuite::TeardownReason::UNEXPECTED_MESSAGE;
    case ZX_ERR_NOT_SUPPORTED:
      return fidl::serversuite::TeardownReason::UNEXPECTED_MESSAGE;
    case ZX_ERR_PROTOCOL_NOT_SUPPORTED:
      return fidl::serversuite::TeardownReason::INCOMPATIBLE_FORMAT;
    default:
      ZX_PANIC("GetTeardownReason: missing a case for this error: %d (%s)", error,
               zx_status_get_string(error));
  }
}

template <typename Protocol>
class TargetServer : public Protocol {
 public:
  explicit TargetServer(async_dispatcher_t* dispatcher, fidl::InterfaceRequest<Protocol> server_end,
                        fidl::serversuite::Runner::EventSender_& runner_event_sender,
                        const char* tag)
      : binding_(this, std::move(server_end), dispatcher),
        runner_event_sender_(runner_event_sender),
        tag_(tag) {
    binding_.set_error_handler(fit::bind_member(this, &TargetServer::OnUnbound));
  }

  void OnUnbound(zx_status_t error_expected_under_test) {
    FX_LOGST(INFO, tag_) << "Target completed: " << zx_status_get_string(error_expected_under_test);
    auto reason = GetTeardownReason(error_expected_under_test);
    FX_LOGST(INFO, tag_) << "Sending OnTeardown event: 0x" << std::hex
                         << static_cast<uint32_t>(reason) << std::dec;
    runner_event_sender_.OnTeardown(reason);
  }

  fidl::Binding<Protocol> binding_;
  fidl::serversuite::Runner::EventSender_& runner_event_sender_;
  const char* tag_;
};

class ClosedTargetServer : public TargetServer<fidl::serversuite::ClosedTarget> {
 public:
  using TargetServer::TargetServer;

  void OneWayNoPayload() override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: OneWayNoPayload";
    runner_event_sender_.OnReceivedClosedTargetOneWayNoPayload();
  }

  void TwoWayNoPayload(TwoWayNoPayloadCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: TwoWayNoPayload";
    callback();
  }

  void TwoWayStructPayload(int8_t v, TwoWayStructPayloadCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: TwoWayStructPayload";
    callback(v);
  }

  void TwoWayTablePayload(::fidl::serversuite::ClosedTargetTwoWayTablePayloadRequest request,
                          TwoWayTablePayloadCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: TwoWayTablePayload";
    fidl::serversuite::ClosedTargetTwoWayTablePayloadResponse response;
    response.set_v(request.v());
    callback(std::move(response));
  }

  void TwoWayUnionPayload(::fidl::serversuite::ClosedTargetTwoWayUnionPayloadRequest request,
                          TwoWayUnionPayloadCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: TwoWayUnionPayload";
    fidl::serversuite::ClosedTargetTwoWayUnionPayloadResponse response;
    response.set_v(request.v());
    callback(std::move(response));
  }

  void TwoWayResult(::fidl::serversuite::ClosedTargetTwoWayResultRequest request,
                    TwoWayResultCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: TwoWayResult";
    switch (request.Which()) {
      case fidl::serversuite::ClosedTargetTwoWayResultRequest::kPayload:
        callback(fpromise::ok(request.payload()));
        break;
      case fidl::serversuite::ClosedTargetTwoWayResultRequest::kError:
        callback(fpromise::error(request.error()));
        break;
      case fidl::serversuite::ClosedTargetTwoWayResultRequest::Invalid:
        ZX_PANIC("unexpected invalid case");
        break;
    }
  }

  void GetHandleRights(zx::handle handle, GetHandleRightsCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: GetHandleRights";
    zx_info_handle_basic_t info;
    ZX_ASSERT(ZX_OK ==
              handle.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
    callback(info.rights);
  }

  void GetSignalableEventRights(zx::event event,
                                GetSignalableEventRightsCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: GetSignalableEventRights";
    zx_info_handle_basic_t info;
    ZX_ASSERT(ZX_OK == event.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
    callback(info.rights);
  }

  void EchoAsTransferableSignalableEvent(
      zx::handle handle, EchoAsTransferableSignalableEventCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: EchoAsTransferableSignalableEvent";
    callback(zx::event(handle.release()));
  }

  void ByteVectorSize(std::vector<uint8_t> vec, ByteVectorSizeCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: ByteVectorSize";
    callback(static_cast<uint32_t>(vec.size()));
  }

  void HandleVectorSize(std::vector<zx::event> vec, HandleVectorSizeCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: HandleVectorSize";
    callback(static_cast<uint32_t>(vec.size()));
  }

  void CreateNByteVector(uint32_t n, CreateNByteVectorCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: CreateNByteVector";
    std::vector<uint8_t> bytes(n);
    callback(std::move(bytes));
  }

  void CreateNHandleVector(uint32_t n, CreateNHandleVectorCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: CreateNHandleVector";
    std::vector<zx::event> handles(n);
    for (auto& handle : handles) {
      ZX_ASSERT(ZX_OK == zx::event::create(0, &handle));
    }
    callback(std::move(handles));
  }
};

class AjarTargetServer : public TargetServer<fidl::serversuite::AjarTarget> {
 public:
  using TargetServer::TargetServer;

 protected:
  void handle_unknown_method(uint64_t ordinal) override {
    FX_LOGST(INFO, tag_) << "Handling AjarTarget request: (unknown method)";
    auto unknown_method_type = fidl::serversuite::UnknownMethodType::ONE_WAY;
    runner_event_sender_.OnReceivedUnknownMethod(ordinal, unknown_method_type);
  }
};

class OpenTargetServer : public TargetServer<fidl::serversuite::OpenTarget> {
 public:
  using TargetServer::TargetServer;

  void StrictOneWay() override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: StrictOneWay";
    runner_event_sender_.OnReceivedOpenTargetStrictOneWay();
  }

  void FlexibleOneWay() override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: FlexibleOneWay";
    runner_event_sender_.OnReceivedOpenTargetFlexibleOneWay();
  }

  void StrictTwoWay(StrictTwoWayCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: StrictTwoWay";
    callback();
  }

  void StrictTwoWayFields(int32_t reply_with, StrictTwoWayFieldsCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: StrictTwoWayFields";
    callback(reply_with);
  }

  void StrictTwoWayErr(::fidl::serversuite::OpenTargetStrictTwoWayErrRequest request,
                       StrictTwoWayErrCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: StrictTwoWayErr";
    if (request.is_reply_success()) {
      callback(fpromise::ok());
    } else if (request.is_reply_error()) {
      callback(fpromise::error(request.reply_error()));
    } else {
      ZX_PANIC("Unrecognized StrictTwoWayErr Request Variant");
    }
  }

  void StrictTwoWayFieldsErr(::fidl::serversuite::OpenTargetStrictTwoWayFieldsErrRequest request,
                             StrictTwoWayFieldsErrCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: StrictTwoWayFieldsErr";
    if (request.is_reply_success()) {
      callback(fpromise::ok(request.reply_success()));
    } else if (request.is_reply_error()) {
      callback(fpromise::error(request.reply_error()));
    } else {
      ZX_PANIC("Unrecognized StrictTwoWayFieldsErr Request Variant");
    }
  }

  void FlexibleTwoWay(FlexibleTwoWayCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: FlexibleTwoWay";
    callback(fpromise::ok());
  }

  void FlexibleTwoWayFields(int32_t reply_with, FlexibleTwoWayFieldsCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: FlexibleTwoWayFields";
    callback(fpromise::ok(reply_with));
  }

  void FlexibleTwoWayErr(::fidl::serversuite::OpenTargetFlexibleTwoWayErrRequest request,
                         FlexibleTwoWayErrCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: FlexibleTwoWayErr";
    if (request.is_reply_success()) {
      callback(fpromise::ok());
    } else if (request.is_reply_error()) {
      callback(fpromise::error(request.reply_error()));
    } else {
      ZX_PANIC("Unrecognized FlexibleTwoWayErr Request Variant");
    }
  }

  void FlexibleTwoWayFieldsErr(
      ::fidl::serversuite::OpenTargetFlexibleTwoWayFieldsErrRequest request,
      FlexibleTwoWayFieldsErrCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: FlexibleTwoWayFieldsErr";
    if (request.is_reply_success()) {
      callback(fpromise::ok(request.reply_success()));
    } else if (request.is_reply_error()) {
      callback(fpromise::error(request.reply_error()));
    } else {
      ZX_PANIC("Unrecognized FlexibleTwoWayFieldsErr Request Variant");
    }
  }

 protected:
  void handle_unknown_method(uint64_t ordinal, bool method_has_response) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: (unknown method)";
    auto unknown_method_type = method_has_response ? fidl::serversuite::UnknownMethodType::TWO_WAY
                                                   : fidl::serversuite::UnknownMethodType::ONE_WAY;
    runner_event_sender_.OnReceivedUnknownMethod(ordinal, unknown_method_type);
  }
};

class RunnerServer : public fidl::serversuite::Runner {
 public:
  explicit RunnerServer(async_dispatcher_t* dispatcher,
                        fidl::InterfaceRequest<fidl::serversuite::Runner> server_end)
      : binding_(this, std::move(server_end), dispatcher) {
    binding_.set_error_handler([this](zx_status_t status) {
      if (status != ZX_ERR_PEER_CLOSED) {
        ZX_PANIC("Runner failed: %s", zx_status_get_string(status));
      }
      delete this;
    });
  }

  void GetVersion(GetVersionCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling Runner request: GetVersion";
    callback(fidl::serversuite::SERVER_SUITE_VERSION);
  }

  void CheckAlive(CheckAliveCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling Runner request: CheckAlive";
    callback();
  }

  void Start(fidl::serversuite::Test test, fidl::serversuite::AnyTarget target,
             StartCallback callback) override {
    // Include the GoogleTest test name from the server suite harness.
    // This helps to clarify which logs belong to which test.
    snprintf(tag_, sizeof tag_, "ServerTest_%u", static_cast<uint32_t>(test));
    FX_LOGST(INFO, tag_) << "Handling Runner request: Start";
    ZX_ASSERT_MSG(std::holds_alternative<std::monostate>(target_), "must only call Start() once");
    if (kDisabledTests.count(test) != 0) {
      return callback(fpromise::error(fidl::serversuite::StartError::TEST_DISABLED));
    }
    switch (target.Which()) {
      case fidl::serversuite::AnyTarget::kClosed:
        FX_LOGST(INFO, tag_) << "Serving ClosedTarget...";
        target_.emplace<ClosedTargetServer>(binding_.dispatcher(), std::move(target.closed()),
                                            binding_.events(), tag_);
        break;
      case fidl::serversuite::AnyTarget::kAjar:
        FX_LOGST(INFO, tag_) << "Serving AjarTarget...";
        target_.emplace<AjarTargetServer>(binding_.dispatcher(), std::move(target.ajar()),
                                          binding_.events(), tag_);
        break;
      case fidl::serversuite::AnyTarget::kOpen:
        FX_LOGST(INFO, tag_) << "Serving OpenTarget...";
        target_.emplace<OpenTargetServer>(binding_.dispatcher(), std::move(target.open()),
                                          binding_.events(), tag_);
        break;
      case fidl::serversuite::AnyTarget::Invalid:
        ZX_PANIC("invalid target");
    }
    callback(fpromise::ok());
  }

  void ShutdownWithEpitaph(int32_t epitaph_status, ShutdownWithEpitaphCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling Runner request: ShutdownWithEpitaph";
    if (auto* target = std::get_if<ClosedTargetServer>(&target_)) {
      target->binding_.Close(epitaph_status);
    } else if (auto* target = std::get_if<AjarTargetServer>(&target_)) {
      target->binding_.Close(epitaph_status);
    } else if (auto* target = std::get_if<OpenTargetServer>(&target_)) {
      target->binding_.Close(epitaph_status);
    } else {
      ZX_PANIC("the target is not set");
    }
    // HLCPP does not call the error handler when close server manually,
    // so we need to send the OnTeardown event for it here.
    FX_LOGST(INFO, tag_) << "Sending OnTeardown event: VOLUNTARY_SHUTDOWN";
    binding_.events().OnTeardown(fidl::serversuite::TeardownReason::VOLUNTARY_SHUTDOWN);
    callback();
  }

  void SendOpenTargetStrictEvent(SendOpenTargetStrictEventCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling Runner request: SendOpenTargetStrictEvent";
    std::get<OpenTargetServer>(target_).binding_.events().StrictEvent();
    callback();
  }

  void SendOpenTargetFlexibleEvent(SendOpenTargetFlexibleEventCallback callback) override {
    FX_LOGST(INFO, tag_) << "Handling Runner request: SendOpenTargetFlexibleEvent";
    std::get<OpenTargetServer>(target_).binding_.events().FlexibleEvent();
    callback();
  }

 private:
  fidl::Binding<fidl::serversuite::Runner> binding_;
  std::variant<std::monostate, ClosedTargetServer, AjarTargetServer, OpenTargetServer> target_;
  char tag_[32] = "";
};

}  // namespace

int main(int argc, const char** argv) {
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"hlcpp"}).BuildAndInitialize();
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  fidl::InterfaceRequestHandler<fidl::serversuite::Runner> handler =
      [&](fidl::InterfaceRequest<fidl::serversuite::Runner> server_end) {
        FX_LOGS(INFO) << "Serving Runner...";
        // RunnerServer deletes itself when it unbinds.
        new RunnerServer(loop.dispatcher(), std::move(server_end));
      };
  auto context = sys::ComponentContext::Create();
  context->outgoing()->AddPublicService(std::move(handler));
  context->outgoing()->ServeFromStartupInfo(loop.dispatcher());
  FX_LOGS(INFO) << "HLCPP serversuite server: ready!";
  return loop.Run();
}
