// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.serversuite/cpp/wire_messaging.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

#include <iostream>
#include <set>

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
class TargetServer : public fidl::WireServer<Protocol> {
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
    ZX_ASSERT(fidl::WireSendEvent(runner_binding_)->OnTeardown({reason}).ok());
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
    ZX_ASSERT(fidl::WireSendEvent(runner_binding_)->OnReceivedClosedTargetOneWayNoPayload().ok());
  }

  void TwoWayNoPayload(TwoWayNoPayloadCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: ";
    completer.Reply();
  }

  void TwoWayStructPayload(TwoWayStructPayloadRequestView request,
                           TwoWayStructPayloadCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: TwoWayStructPayload";
    completer.Reply(request->v);
  }

  void TwoWayTablePayload(TwoWayTablePayloadRequestView request,
                          TwoWayTablePayloadCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: TwoWayTablePayload";
    fidl::Arena arena;
    completer.Reply(fidl_serversuite::wire::ClosedTargetTwoWayTablePayloadResponse::Builder(arena)
                        .v(request->v())
                        .Build());
  }

  void TwoWayUnionPayload(TwoWayUnionPayloadRequestView request,
                          TwoWayUnionPayloadCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: TwoWayUnionPayload";
    completer.Reply(
        fidl_serversuite::wire::ClosedTargetTwoWayUnionPayloadResponse::WithV(request->v()));
  }

  void TwoWayResult(TwoWayResultRequestView request,
                    TwoWayResultCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: TwoWayResult";
    switch (request->Which()) {
      case fidl_serversuite::wire::ClosedTargetTwoWayResultRequest::Tag::kPayload: {
        completer.ReplySuccess(request->payload());
        return;
      }
      case fidl_serversuite::wire::ClosedTargetTwoWayResultRequest::Tag::kError:
        completer.ReplyError(request->error());
        return;
    }
  }

  void GetHandleRights(GetHandleRightsRequestView request,
                       GetHandleRightsCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: GetHandleRights";
    zx_info_handle_basic_t info;
    ZX_ASSERT(ZX_OK == request->handle.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr,
                                                nullptr));
    completer.Reply(info.rights);
  }

  void GetSignalableEventRights(GetSignalableEventRightsRequestView request,
                                GetSignalableEventRightsCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: GetSignalableEventRights";
    zx_info_handle_basic_t info;
    ZX_ASSERT(ZX_OK == request->handle.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr,
                                                nullptr));
    completer.Reply(info.rights);
  }

  void EchoAsTransferableSignalableEvent(
      EchoAsTransferableSignalableEventRequestView request,
      EchoAsTransferableSignalableEventCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: EchoAsTransferableSignalableEvent";
    completer.Reply(zx::event(request->handle.release()));
  }

  void ByteVectorSize(ByteVectorSizeRequestView request,
                      ByteVectorSizeCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: ByteVectorSize";
    completer.Reply(static_cast<uint32_t>(request->vec.size()));
  }

  void HandleVectorSize(HandleVectorSizeRequestView request,
                        HandleVectorSizeCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: HandleVectorSize";
    completer.Reply(static_cast<uint32_t>(request->vec.size()));
  }

  void CreateNByteVector(CreateNByteVectorRequestView request,
                         CreateNByteVectorCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: CreateNByteVector";
    std::vector<uint8_t> bytes(request->n);
    completer.Reply(fidl::VectorView<uint8_t>::FromExternal(bytes));
  }

  void CreateNHandleVector(CreateNHandleVectorRequestView request,
                           CreateNHandleVectorCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling ClosedTarget request: CreateNHandleVector";
    std::vector<zx::event> handles(request->n);
    for (auto& handle : handles) {
      ZX_ASSERT(ZX_OK == zx::event::create(0, &handle));
    }
    completer.Reply(fidl::VectorView<zx::event>::FromExternal(handles));
  }
};

class AjarTargetServer : public TargetServer<fidl_serversuite::AjarTarget> {
 public:
  using TargetServer::TargetServer;

 protected:
  void handle_unknown_method(fidl::UnknownMethodMetadata<fidl_serversuite::AjarTarget> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling AjarTarget request: (unknown method)";
    auto result = fidl::WireSendEvent(runner_binding_)
                      ->OnReceivedUnknownMethod(metadata.method_ordinal,
                                                fidl_serversuite::wire::UnknownMethodType::kOneWay);
    ZX_ASSERT(result.ok());
  }
};

class OpenTargetServer : public TargetServer<fidl_serversuite::OpenTarget> {
 public:
  using TargetServer::TargetServer;

  void StrictOneWay(StrictOneWayCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: StrictOneWay";
    ZX_ASSERT(fidl::WireSendEvent(runner_binding_)->OnReceivedOpenTargetStrictOneWay().ok());
  }

  void FlexibleOneWay(FlexibleOneWayCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: FlexibleOneWay";
    ZX_ASSERT(fidl::WireSendEvent(runner_binding_)->OnReceivedOpenTargetFlexibleOneWay().ok());
  }

  void StrictTwoWay(StrictTwoWayCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: ";
    completer.Reply();
  }

  void StrictTwoWayFields(StrictTwoWayFieldsRequestView request,
                          StrictTwoWayFieldsCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: StrictTwoWayFields";
    completer.Reply(request->reply_with);
  }

  void StrictTwoWayErr(StrictTwoWayErrRequestView request,
                       StrictTwoWayErrCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: StrictTwoWayErr";
    switch (request->Which()) {
      case fidl_serversuite::wire::OpenTargetStrictTwoWayErrRequest::Tag::kReplySuccess:
        completer.ReplySuccess();
        break;
      case fidl_serversuite::wire::OpenTargetStrictTwoWayErrRequest::Tag::kReplyError:
        completer.ReplyError(request->reply_error());
        break;
    }
  }

  void StrictTwoWayFieldsErr(StrictTwoWayFieldsErrRequestView request,
                             StrictTwoWayFieldsErrCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: StrictTwoWayFieldsErr";
    switch (request->Which()) {
      case fidl_serversuite::wire::OpenTargetStrictTwoWayFieldsErrRequest::Tag::kReplySuccess:
        completer.ReplySuccess(request->reply_success());
        break;
      case fidl_serversuite::wire::OpenTargetStrictTwoWayFieldsErrRequest::Tag::kReplyError:
        completer.ReplyError(request->reply_error());
        break;
    }
  }

  void FlexibleTwoWay(FlexibleTwoWayCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: ";
    completer.Reply();
  }

  void FlexibleTwoWayFields(FlexibleTwoWayFieldsRequestView request,
                            FlexibleTwoWayFieldsCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: FlexibleTwoWayFields";
    completer.Reply(request->reply_with);
  }

  void FlexibleTwoWayErr(FlexibleTwoWayErrRequestView request,
                         FlexibleTwoWayErrCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: FlexibleTwoWayErr";
    switch (request->Which()) {
      case fidl_serversuite::wire::OpenTargetFlexibleTwoWayErrRequest::Tag::kReplySuccess:
        completer.ReplySuccess();
        break;
      case fidl_serversuite::wire::OpenTargetFlexibleTwoWayErrRequest::Tag::kReplyError:
        completer.ReplyError(request->reply_error());
        break;
    }
  }

  void FlexibleTwoWayFieldsErr(FlexibleTwoWayFieldsErrRequestView request,
                               FlexibleTwoWayFieldsErrCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling OpenTarget request: FlexibleTwoWayFieldsErr";
    switch (request->Which()) {
      case fidl_serversuite::wire::OpenTargetFlexibleTwoWayFieldsErrRequest::Tag::kReplySuccess:
        completer.ReplySuccess(request->reply_success());
        break;
      case fidl_serversuite::wire::OpenTargetFlexibleTwoWayFieldsErrRequest::Tag::kReplyError:
        completer.ReplyError(request->reply_error());
        break;
    }
  }

 protected:
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
    auto result = fidl::WireSendEvent(runner_binding_)
                      ->OnReceivedUnknownMethod(metadata.method_ordinal, method_type);
    ZX_ASSERT(result.ok());
  }
};

class RunnerServer : public fidl::WireServer<fidl_serversuite::Runner> {
 public:
  explicit RunnerServer(async_dispatcher_t* dispatcher,
                        fidl::ServerEnd<fidl_serversuite::Runner> server_end)
      : dispatcher_(dispatcher),
        binding_(fidl::BindServer(dispatcher, std::move(server_end), this,
                                  std::mem_fn(&RunnerServer::OnUnbound))) {}

  void GetVersion(GetVersionCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling Runner request: GetVersion";
    completer.Reply(fidl_serversuite::wire::kServerSuiteVersion);
  }

  void CheckAlive(CheckAliveCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling Runner request: CheckAlive";
    completer.Reply();
  }

  void Start(StartRequestView request, StartCompleter::Sync& completer) override {
    // Include the GoogleTest test name from the server suite harness.
    // This helps to clarify which logs belong to which test.
    snprintf(tag_, sizeof tag_, "ServerTest_%u", static_cast<uint32_t>(request->test));
    FX_LOGST(INFO, tag_) << "Handling Runner request: Start";
    ZX_ASSERT_MSG(std::holds_alternative<std::monostate>(target_), "must only call Start() once");
    if (kDisabledTests.count(request->test) != 0) {
      return completer.Reply(fit::error(fidl_serversuite::StartError::kTestDisabled));
    }
    switch (request->any_target.Which()) {
      case fidl_serversuite::wire::AnyTarget::Tag::kClosed:
        FX_LOGST(INFO, tag_) << "Serving ClosedTarget...";
        target_.emplace<ClosedTargetServer>(dispatcher_, std::move(request->any_target.closed()),
                                            binding_, tag_);
        break;
      case fidl_serversuite::wire::AnyTarget::Tag::kAjar:
        FX_LOGST(INFO, tag_) << "Serving AjarTarget...";
        target_.emplace<AjarTargetServer>(dispatcher_, std::move(request->any_target.ajar()),
                                          binding_, tag_);
        break;
      case fidl_serversuite::wire::AnyTarget::Tag::kOpen: {
        FX_LOGST(INFO, tag_) << "Serving OpenTarget...";
        target_.emplace<OpenTargetServer>(dispatcher_, std::move(request->any_target.open()),
                                          binding_, tag_);
        break;
      }
    }
    completer.Reply(fit::ok());
  }

  void ShutdownWithEpitaph(ShutdownWithEpitaphRequestView request,
                           ShutdownWithEpitaphCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling Runner request: ShutdownWithEpitaph";
    if (auto* target = std::get_if<ClosedTargetServer>(&target_)) {
      target->binding_.Close(request->epitaph_status);
    } else if (auto* target = std::get_if<AjarTargetServer>(&target_)) {
      target->binding_.Close(request->epitaph_status);
    } else if (auto* target = std::get_if<OpenTargetServer>(&target_)) {
      target->binding_.Close(request->epitaph_status);
    } else {
      ZX_PANIC("the target is not set");
    }
    completer.Reply();
  }

  void SendOpenTargetStrictEvent(SendOpenTargetStrictEventCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling Runner request: SendOpenTargetStrictEvent";
    ZX_ASSERT(
        fidl::WireSendEvent(std::get<OpenTargetServer>(target_).binding_)->StrictEvent().ok());
    completer.Reply();
  }

  void SendOpenTargetFlexibleEvent(SendOpenTargetFlexibleEventCompleter::Sync& completer) override {
    FX_LOGST(INFO, tag_) << "Handling Runner request: SendOpenTargetFlexibleEvent";
    ZX_ASSERT(
        fidl::WireSendEvent(std::get<OpenTargetServer>(target_).binding_)->FlexibleEvent().ok());
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
  builder.WithTags({"cpp_wire"}).BuildAndInitialize();
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
  FX_LOGS(INFO) << "CPP Wire serversuite server: ready!";
  return loop.Run();
}
