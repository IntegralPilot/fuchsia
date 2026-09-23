// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.serversuite/cpp/common_types.h>
#include <fidl/fidl.serversuite/cpp/fidl.h>
#include <fidl/fidl.serversuite/cpp/natural_types.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/dispatcher.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/status.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

#include <iostream>
#include <set>
#include <variant>

#include "src/tests/fidl/dynsuite/server_suite/runners/cpp_util/teardown_reason.h"

namespace {

const std::set<fidl_serversuite::Test> kDisabledTests = {
    // This is for testing the test disabling functionality itself.
    fidl_serversuite::Test::kIgnoreDisabled,
    // TODO(https://fxbug.dev/42080225): Should validate txid.
    fidl_serversuite::Test::kOneWayWithNonZeroTxid,
    fidl_serversuite::Test::kTwoWayNoPayloadWithZeroTxid,
};

template <typename Protocol>
class TargetServer : public fidl::Server<Protocol> {
 public:
  explicit TargetServer(async_dispatcher_t* dispatcher, fidl::ServerEnd<Protocol> server_end,
                        const fidl::ServerBindingRef<fidl_serversuite::Runner>& runner_binding,
                        const char* tag)
      : binding_(fidl::BindServer(dispatcher, std::move(server_end), this,
                                  std::mem_fn(&TargetServer::OnUnbound))),
        runner_binding_(runner_binding),
        tag_(tag) {}

  void OnUnbound(fidl::UnbindInfo info, fidl::ServerEnd<Protocol> server_end) {
    FX_LOGST(INFO, tag_) << "Target completed: " << info;
    auto reason = cpp_util::GetTeardownReason(info);
    FX_LOGST(INFO, tag_) << "Sending OnTeardown event: 0x" << std::hex
                         << static_cast<uint32_t>(reason) << std::dec;
    ZX_ASSERT(fidl::SendEvent(runner_binding_)->OnTeardown({reason}).is_ok());
  }

  fidl::ServerBindingRef<Protocol> binding_;
  const fidl::ServerBindingRef<fidl_serversuite::Runner>& runner_binding_;
  const char* tag_;
};

class ClosedTargetServer : public TargetServer<fidl_serversuite::ClosedTarget> {
 public:
  using TargetServer::TargetServer;

  void OneWayNoPayload(OneWayNoPayloadCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: OneWayNoPayload";
    ZX_ASSERT(fidl::SendEvent(runner_binding_)->OnReceivedClosedTargetOneWayNoPayload().is_ok());
  }

  void TwoWayNoPayload(TwoWayNoPayloadCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: TwoWayNoPayload";
    completer.Reply();
  }

  void TwoWayStructPayload(TwoWayStructPayloadRequest& request,
                           TwoWayStructPayloadCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: TwoWayStructPayload";
    completer.Reply(request.v());
  }

  void TwoWayTablePayload(TwoWayTablePayloadRequest& request,
                          TwoWayTablePayloadCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: TwoWayTablePayload";
    fidl_serversuite::ClosedTargetTwoWayTablePayloadResponse response({.v = request.v()});
    completer.Reply(response);
  }

  void TwoWayUnionPayload(TwoWayUnionPayloadRequest& request,
                          TwoWayUnionPayloadCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: TwoWayUnionPayload";
    ZX_ASSERT(request.v().has_value());
    completer.Reply(
        fidl_serversuite::ClosedTargetTwoWayUnionPayloadResponse::WithV(request.v().value()));
  }

  void TwoWayResult(TwoWayResultRequest& request, TwoWayResultCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: TwoWayResult";
    switch (request.Which()) {
      case TwoWayResultRequest::Tag::kPayload:
        completer.Reply(fit::ok(request.payload().value()));
        return;
      case TwoWayResultRequest::Tag::kError:
        completer.Reply(fit::error(request.error().value()));
        return;
    }
    ZX_PANIC("unhandled case");
  }

  void GetHandleRights(GetHandleRightsRequest& request,
                       GetHandleRightsCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: GetHandleRights";
    zx_info_handle_basic_t info;
    ZX_ASSERT(ZX_OK == request.handle().get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr,
                                                 nullptr));
    completer.Reply(info.rights);
  }

  void GetSignalableEventRights(GetSignalableEventRightsRequest& request,
                                GetSignalableEventRightsCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: GetSignalableEventRights";
    zx_info_handle_basic_t info;
    ZX_ASSERT(ZX_OK == request.handle().get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr,
                                                 nullptr));
    completer.Reply(info.rights);
  }

  void EchoAsTransferableSignalableEvent(
      EchoAsTransferableSignalableEventRequest& request,
      EchoAsTransferableSignalableEventCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: EchoAsTransferableSignalableEvent";
    completer.Reply(zx::event(request.handle().release()));
  }

  void ByteVectorSize(ByteVectorSizeRequest& request,
                      ByteVectorSizeCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: ByteVectorSize";
    completer.Reply(static_cast<uint32_t>(request.vec().size()));
  }

  void HandleVectorSize(HandleVectorSizeRequest& request,
                        HandleVectorSizeCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: HandleVectorSize";
    completer.Reply(static_cast<uint32_t>(request.vec().size()));
  }

  void CreateNByteVector(CreateNByteVectorRequest& request,
                         CreateNByteVectorCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: CreateNByteVector";
    std::vector<uint8_t> bytes(request.n());
    completer.Reply(std::move(bytes));
  }

  void CreateNHandleVector(CreateNHandleVectorRequest& request,
                           CreateNHandleVectorCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: CreateNHandleVector";
    std::vector<zx::event> handles(request.n());
    for (auto& handle : handles) {
      ZX_ASSERT(ZX_OK == zx::event::create(0, &handle));
    }
    completer.Reply(std::move(handles));
  }
};

class AjarTargetServer : public TargetServer<fidl_serversuite::AjarTarget> {
 public:
  using TargetServer::TargetServer;

  void handle_unknown_method(fidl::UnknownMethodMetadata<fidl_serversuite::AjarTarget> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling AjarTarget request: (unknown method)";
    auto result = fidl::SendEvent(runner_binding_)
                      ->OnReceivedUnknownMethod(fidl_serversuite::UnknownMethodInfo(
                          metadata.method_ordinal, fidl_serversuite::UnknownMethodType::kOneWay));
    ZX_ASSERT(result.is_ok());
  }
};

class OpenTargetServer : public TargetServer<fidl_serversuite::OpenTarget> {
 public:
  using TargetServer::TargetServer;

  void StrictOneWay(StrictOneWayCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: StrictOneWay";
    ZX_ASSERT(fidl::SendEvent(runner_binding_)->OnReceivedOpenTargetStrictOneWay().is_ok());
  }

  void FlexibleOneWay(FlexibleOneWayCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: FlexibleOneWay";
    ZX_ASSERT(fidl::SendEvent(runner_binding_)->OnReceivedOpenTargetFlexibleOneWay().is_ok());
  }

  void StrictTwoWay(StrictTwoWayCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: StrictTwoWay";
    completer.Reply();
  }

  void StrictTwoWayFields(StrictTwoWayFieldsRequest& request,
                          StrictTwoWayFieldsCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: StrictTwoWayFields";
    completer.Reply(request.reply_with());
  }

  void StrictTwoWayErr(StrictTwoWayErrRequest& request,
                       StrictTwoWayErrCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: StrictTwoWayErr";
    switch (request.Which()) {
      case fidl_serversuite::OpenTargetStrictTwoWayErrRequest::Tag::kReplySuccess:
        completer.Reply(fit::ok());
        break;
      case fidl_serversuite::OpenTargetStrictTwoWayErrRequest::Tag::kReplyError:
        completer.Reply(fit::error(request.reply_error().value()));
        break;
    }
  }

  void StrictTwoWayFieldsErr(StrictTwoWayFieldsErrRequest& request,
                             StrictTwoWayFieldsErrCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: StrictTwoWayFieldsErr";
    switch (request.Which()) {
      case fidl_serversuite::OpenTargetStrictTwoWayFieldsErrRequest::Tag::kReplySuccess:
        completer.Reply(fit::ok(request.reply_success().value()));
        break;
      case fidl_serversuite::OpenTargetStrictTwoWayFieldsErrRequest::Tag::kReplyError:
        completer.Reply(fit::error(request.reply_error().value()));
        break;
    }
  }

  void FlexibleTwoWay(FlexibleTwoWayCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: FlexibleTwoWay";
    completer.Reply();
  }

  void FlexibleTwoWayFields(FlexibleTwoWayFieldsRequest& request,
                            FlexibleTwoWayFieldsCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: FlexibleTwoWayFields";
    completer.Reply(request.reply_with());
  }

  void FlexibleTwoWayErr(FlexibleTwoWayErrRequest& request,
                         FlexibleTwoWayErrCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: FlexibleTwoWayErr";
    switch (request.Which()) {
      case fidl_serversuite::OpenTargetFlexibleTwoWayErrRequest::Tag::kReplySuccess:
        completer.Reply(fit::ok());
        break;
      case fidl_serversuite::OpenTargetFlexibleTwoWayErrRequest::Tag::kReplyError:
        completer.Reply(fit::error(request.reply_error().value()));
        break;
    }
  }

  void FlexibleTwoWayFieldsErr(FlexibleTwoWayFieldsErrRequest& request,
                               FlexibleTwoWayFieldsErrCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: FlexibleTwoWayFieldsErr";
    switch (request.Which()) {
      case fidl_serversuite::OpenTargetFlexibleTwoWayFieldsErrRequest::Tag::kReplySuccess:
        completer.Reply(fit::ok(request.reply_success().value()));
        break;
      case fidl_serversuite::OpenTargetFlexibleTwoWayFieldsErrRequest::Tag::kReplyError:
        completer.Reply(fit::error(request.reply_error().value()));
        break;
    }
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fidl_serversuite::OpenTarget> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: (unknown method)";
    fidl_serversuite::UnknownMethodType method_type;
    switch (metadata.unknown_method_type) {
      case fidl::UnknownMethodType::kOneWay:
        method_type = fidl_serversuite::UnknownMethodType::kOneWay;
        break;
      case fidl::UnknownMethodType::kTwoWay:
        method_type = fidl_serversuite::UnknownMethodType::kTwoWay;
        break;
    }
    auto result = fidl::SendEvent(runner_binding_)
                      ->OnReceivedUnknownMethod(fidl_serversuite::UnknownMethodInfo(
                          metadata.method_ordinal, method_type));
    ZX_ASSERT(result.is_ok());
  }
};

class RunnerServer : public fidl::Server<fidl_serversuite::Runner> {
 public:
  explicit RunnerServer(async_dispatcher_t* dispatcher,
                        fidl::ServerEnd<fidl_serversuite::Runner> server_end)
      : dispatcher_(dispatcher),
        binding_(fidl::BindServer(dispatcher, std::move(server_end), this,
                                  std::mem_fn(&RunnerServer::OnUnbound))) {}

  void GetVersion(GetVersionCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling Runner request: GetVersion";
    completer.Reply(fidl_serversuite::kServerSuiteVersion);
  }

  void CheckAlive(CheckAliveCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling Runner request: CheckAlive";
    completer.Reply();
  }

  void Start(StartRequest& request, StartCompleter::Sync& completer) override {
    // Include the GoogleTest test name from the server suite harness.
    // This helps to clarify which logs belong to which test.
    snprintf(tag_, sizeof tag_, "ServerTest_%u", static_cast<uint32_t>(request.test()));
    FX_LOGST(INFO, tag_) << "Handling Runner request: Start";
    ZX_ASSERT_MSG(std::holds_alternative<std::monostate>(target_), "must only call Start() once");
    if (kDisabledTests.count(request.test()) != 0) {
      return completer.Reply(fit::error(fidl_serversuite::StartError::kTestDisabled));
    }
    switch (request.any_target().Which()) {
      case fidl_serversuite::AnyTarget::Tag::kClosed:
        FX_LOGST(INFO, tag_) << "Serving ClosedTarget...";
        target_.emplace<ClosedTargetServer>(
            dispatcher_, std::move(request.any_target().closed().value()), binding_, tag_);
        break;
      case fidl_serversuite::AnyTarget::Tag::kAjar:
        FX_LOGST(INFO, tag_) << "Serving AjarTarget...";
        target_.emplace<AjarTargetServer>(
            dispatcher_, std::move(request.any_target().ajar().value()), binding_, tag_);
        break;
      case fidl_serversuite::AnyTarget::Tag::kOpen: {
        FX_LOGST(INFO, tag_) << "Serving OpenTarget...";
        target_.emplace<OpenTargetServer>(
            dispatcher_, std::move(request.any_target().open().value()), binding_, tag_);
        break;
      }
    }
    completer.Reply(fit::ok());
  }

  void ShutdownWithEpitaph(ShutdownWithEpitaphRequest& request,
                           ShutdownWithEpitaphCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling Runner request: ShutdownWithEpitaph";
    if (auto* target = std::get_if<ClosedTargetServer>(&target_)) {
      target->binding_.Close(request.epitaph_status());
    } else if (auto* target = std::get_if<AjarTargetServer>(&target_)) {
      target->binding_.Close(request.epitaph_status());
    } else if (auto* target = std::get_if<OpenTargetServer>(&target_)) {
      target->binding_.Close(request.epitaph_status());
    } else {
      ZX_PANIC("the target is not set");
    }
    completer.Reply();
  }

  void SendOpenTargetStrictEvent(SendOpenTargetStrictEventCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling Runner request: SendOpenTargetStrictEvent";
    ZX_ASSERT(fidl::SendEvent(std::get<OpenTargetServer>(target_).binding_)->StrictEvent().is_ok());
    completer.Reply();
  }

  void SendOpenTargetFlexibleEvent(SendOpenTargetFlexibleEventCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling Runner request: SendOpenTargetFlexibleEvent";
    ZX_ASSERT(
        fidl::SendEvent(std::get<OpenTargetServer>(target_).binding_)->FlexibleEvent().is_ok());
    completer.Reply();
  }

 private:
  void OnUnbound(fidl::UnbindInfo info, fidl::ServerEnd<fidl_serversuite::Runner> server_end) {
    if (!info.is_peer_closed()) {
      ZX_PANIC("Runner failed: %s", info.FormatDescription().c_str());
    }
    delete this;
  }

  async_dispatcher_t* dispatcher_;
  fidl::ServerBindingRef<fidl_serversuite::Runner> binding_;
  std::variant<std::monostate, ClosedTargetServer, AjarTargetServer, OpenTargetServer> target_;
  char tag_[32] = "";
};

}  // namespace

int main(int argc, const char** argv) {
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"cpp"}).BuildAndInitialize();
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  auto outgoing = component::OutgoingDirectory(loop.dispatcher());
  ZX_ASSERT(outgoing.ServeFromStartupInfo().is_ok());
  auto result = outgoing.AddUnmanagedProtocol<fidl_serversuite::Runner>(
      [&](fidl::ServerEnd<fidl_serversuite::Runner> server_end) {
        FX_LOGS(INFO) << "Serving Runner...";
        // RunnerServer deletes itself when it unbinds.
        new RunnerServer(loop.dispatcher(), std::move(server_end));
      });
  ZX_ASSERT(result.is_ok());
  FX_LOGS(INFO) << "CPP serversuite server: ready!";
  return loop.Run();
}
