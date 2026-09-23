// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.clientsuite/cpp/wire_messaging.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>

#include <iostream>
#include <memory>

#include "fidl/fidl.clientsuite/cpp/wire_types.h"
#include "src/tests/fidl/dynsuite/client_suite/runners/cpp_util/error_util.h"

class RunnerServer : public fidl::WireServer<fidl_clientsuite::Runner> {
 public:
  RunnerServer() = default;

  void GetVersion(GetVersionCompleter::Sync& completer) override {
    completer.Reply(fidl_clientsuite::kClientSuiteVersion);
  }

  void IsTestEnabled(IsTestEnabledRequestView request,
                     IsTestEnabledCompleter::Sync& completer) override {
    switch (request->test) {
      // TODO(https://fxbug.dev/42067516): Should validate response ordinal matches.
      case fidl_clientsuite::Test::kReceiveResponseWrongOrdinalKnown:
      case fidl_clientsuite::Test::kReceiveResponseWrongOrdinalUnknown:
      // TODO(https://fxbug.dev/42080225): Should validate that event txid is 0.
      case fidl_clientsuite::Test::kReceiveEventUnexpectedTxid:
        completer.Reply(false);
        return;
      default:
        completer.Reply(true);
        return;
    }
  }

  void CheckAlive(CheckAliveCompleter::Sync& completer) override { completer.Reply(); }

  void GetBindingsProperties(GetBindingsPropertiesCompleter::Sync& completer) override {
    fidl::Arena arena;
    completer.Reply(fidl_clientsuite::wire::BindingsProperties::Builder(arena)
                        .io_style(fidl_clientsuite::IoStyle::kSync)
                        .Build());
  }

  void CallTwoWayNoPayload(CallTwoWayNoPayloadRequestView request,
                           CallTwoWayNoPayloadCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->TwoWayNoPayload();
    if (result.ok()) {
      completer.Reply(
          fidl::WireResponse<fidl_clientsuite::Runner::CallTwoWayNoPayload>::WithSuccess(
              ::fidl_clientsuite::wire::Empty()));
    } else {
      completer.Reply(
          fidl::WireResponse<fidl_clientsuite::Runner::CallTwoWayNoPayload>::WithFidlError(
              clienttest_util::ClassifyError(result)));
    }
  }

  void CallTwoWayStructPayload(CallTwoWayStructPayloadRequestView request,
                               CallTwoWayStructPayloadCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->TwoWayStructPayload();
    if (result.ok()) {
      completer.Reply(
          fidl::WireResponse<fidl_clientsuite::Runner::CallTwoWayStructPayload>::WithSuccess(
              result.value()));
    } else {
      completer.Reply(
          fidl::WireResponse<fidl_clientsuite::Runner::CallTwoWayStructPayload>::WithFidlError(
              clienttest_util::ClassifyError(result)));
    }
  }

  void CallTwoWayTablePayload(CallTwoWayTablePayloadRequestView request,
                              CallTwoWayTablePayloadCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->TwoWayTablePayload();
    if (result.ok()) {
      completer.Reply(
          fidl::WireResponse<fidl_clientsuite::Runner::CallTwoWayTablePayload>::WithSuccess(
              fidl::ObjectView<fidl_clientsuite::wire::TablePayload>::FromExternal(
                  &result.value())));
    } else {
      completer.Reply(
          fidl::WireResponse<fidl_clientsuite::Runner::CallTwoWayTablePayload>::WithFidlError(
              clienttest_util::ClassifyError(result)));
    }
  }

  void CallTwoWayUnionPayload(CallTwoWayUnionPayloadRequestView request,
                              CallTwoWayUnionPayloadCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->TwoWayUnionPayload();
    if (result.ok()) {
      completer.Reply(
          fidl::WireResponse<fidl_clientsuite::Runner::CallTwoWayUnionPayload>::WithSuccess(
              fidl::ObjectView<fidl_clientsuite::wire::UnionPayload>::FromExternal(
                  &result.value())));
    } else {
      completer.Reply(
          fidl::WireResponse<fidl_clientsuite::Runner::CallTwoWayUnionPayload>::WithFidlError(
              clienttest_util::ClassifyError(result)));
    }
  }

  void CallTwoWayStructPayloadErr(CallTwoWayStructPayloadErrRequestView request,
                                  CallTwoWayStructPayloadErrCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->TwoWayStructPayloadErr();
    if (result.ok()) {
      if (result.value().is_ok()) {
        completer.Reply(
            fidl::WireResponse<fidl_clientsuite::Runner::CallTwoWayStructPayloadErr>::WithSuccess(
                *result.value().value()));
      } else {
        completer.Reply(fidl::WireResponse<fidl_clientsuite::Runner::CallTwoWayStructPayloadErr>::
                            WithApplicationError(result.value().error_value()));
      }
    } else {
      completer.Reply(
          fidl::WireResponse<fidl_clientsuite::Runner::CallTwoWayStructPayloadErr>::WithFidlError(
              clienttest_util::ClassifyError(result)));
    }
  }

  void CallTwoWayStructRequest(CallTwoWayStructRequestRequestView request,
                               CallTwoWayStructRequestCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->TwoWayStructRequest(request->request.some_field);
    if (result.ok()) {
      completer.Reply(
          fidl::WireResponse<fidl_clientsuite::Runner::CallTwoWayStructRequest>::WithSuccess(
              ::fidl_clientsuite::wire::Empty()));
    } else {
      completer.Reply(
          fidl::WireResponse<fidl_clientsuite::Runner::CallTwoWayStructRequest>::WithFidlError(
              clienttest_util::ClassifyError(result)));
    }
  }

  void CallTwoWayTableRequest(CallTwoWayTableRequestRequestView request,
                              CallTwoWayTableRequestCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->TwoWayTableRequest(request->request);
    if (result.ok()) {
      completer.Reply(
          fidl::WireResponse<fidl_clientsuite::Runner::CallTwoWayTableRequest>::WithSuccess(
              ::fidl_clientsuite::wire::Empty()));
    } else {
      completer.Reply(
          fidl::WireResponse<fidl_clientsuite::Runner::CallTwoWayTableRequest>::WithFidlError(
              clienttest_util::ClassifyError(result)));
    }
  }

  void CallTwoWayUnionRequest(CallTwoWayUnionRequestRequestView request,
                              CallTwoWayUnionRequestCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->TwoWayUnionRequest(request->request);
    if (result.ok()) {
      completer.Reply(
          fidl::WireResponse<fidl_clientsuite::Runner::CallTwoWayUnionRequest>::WithSuccess(
              ::fidl_clientsuite::wire::Empty()));
    } else {
      completer.Reply(
          fidl::WireResponse<fidl_clientsuite::Runner::CallTwoWayUnionRequest>::WithFidlError(
              clienttest_util::ClassifyError(result)));
    }
  }

  void CallOneWayNoRequest(CallOneWayNoRequestRequestView request,
                           CallOneWayNoRequestCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->OneWayNoRequest();
    if (result.ok()) {
      completer.Reply(fidl_clientsuite::wire::EmptyResultClassification::WithSuccess(
          ::fidl_clientsuite::wire::Empty()));
    } else {
      completer.Reply(fidl_clientsuite::wire::EmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(result.error())));
    }
  }

  void CallOneWayStructRequest(CallOneWayStructRequestRequestView request,
                               CallOneWayStructRequestCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->OneWayStructRequest(request->request.some_field);
    if (result.ok()) {
      completer.Reply(fidl_clientsuite::wire::EmptyResultClassification::WithSuccess(
          ::fidl_clientsuite::wire::Empty()));
    } else {
      completer.Reply(fidl_clientsuite::wire::EmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(result.error())));
    }
  }

  void CallOneWayTableRequest(CallOneWayTableRequestRequestView request,
                              CallOneWayTableRequestCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->OneWayTableRequest(request->request);
    if (result.ok()) {
      completer.Reply(fidl_clientsuite::wire::EmptyResultClassification::WithSuccess(
          ::fidl_clientsuite::wire::Empty()));
    } else {
      completer.Reply(fidl_clientsuite::wire::EmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(result.error())));
    }
  }

  void CallOneWayUnionRequest(CallOneWayUnionRequestRequestView request,
                              CallOneWayUnionRequestCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->OneWayUnionRequest(request->request);
    if (result.ok()) {
      completer.Reply(fidl_clientsuite::wire::EmptyResultClassification::WithSuccess(
          ::fidl_clientsuite::wire::Empty()));
    } else {
      completer.Reply(fidl_clientsuite::wire::EmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(result.error())));
    }
  }

  void CallStrictOneWay(CallStrictOneWayRequestView request,
                        CallStrictOneWayCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->StrictOneWay();
    if (result.ok()) {
      completer.Reply(fidl_clientsuite::wire::EmptyResultClassification::WithSuccess(
          ::fidl_clientsuite::wire::Empty()));
    } else {
      completer.Reply(fidl_clientsuite::wire::EmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(result.error())));
    }
  }

  void CallFlexibleOneWay(CallFlexibleOneWayRequestView request,
                          CallFlexibleOneWayCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->FlexibleOneWay();
    if (result.ok()) {
      completer.Reply(fidl_clientsuite::wire::EmptyResultClassification::WithSuccess(
          ::fidl_clientsuite::wire::Empty()));
    } else {
      completer.Reply(fidl_clientsuite::wire::EmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(result.error())));
    }
  }

  void CallStrictTwoWay(CallStrictTwoWayRequestView request,
                        CallStrictTwoWayCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->StrictTwoWay();
    if (result.ok()) {
      completer.Reply(fidl_clientsuite::wire::EmptyResultClassification::WithSuccess(
          ::fidl_clientsuite::wire::Empty()));
    } else {
      completer.Reply(fidl_clientsuite::wire::EmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(result.error())));
    }
  }

  void CallStrictTwoWayFields(CallStrictTwoWayFieldsRequestView request,
                              CallStrictTwoWayFieldsCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->StrictTwoWayFields();
    if (result.ok()) {
      completer.Reply(
          fidl_clientsuite::wire::NonEmptyResultClassification::WithSuccess(result.value()));
    } else {
      completer.Reply(fidl_clientsuite::wire::NonEmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(result)));
    }
  }

  void CallStrictTwoWayErr(CallStrictTwoWayErrRequestView request,
                           CallStrictTwoWayErrCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->StrictTwoWayErr();
    if (result.ok()) {
      if (result.value().is_ok()) {
        completer.Reply(fidl_clientsuite::wire::EmptyResultWithErrorClassification::WithSuccess(
            ::fidl_clientsuite::wire::Empty()));
      } else {
        completer.Reply(
            fidl_clientsuite::wire::EmptyResultWithErrorClassification::WithApplicationError(
                result.value().error_value()));
      }
    } else {
      completer.Reply(fidl_clientsuite::wire::EmptyResultWithErrorClassification::WithFidlError(
          clienttest_util::ClassifyError(result)));
    }
  }

  void CallStrictTwoWayFieldsErr(CallStrictTwoWayFieldsErrRequestView request,
                                 CallStrictTwoWayFieldsErrCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->StrictTwoWayFieldsErr();
    if (result.ok()) {
      if (result.value().is_ok()) {
        completer.Reply(fidl_clientsuite::wire::NonEmptyResultWithErrorClassification::WithSuccess(
            *result.value().value()));
      } else {
        completer.Reply(
            fidl_clientsuite::wire::NonEmptyResultWithErrorClassification::WithApplicationError(
                result.value().error_value()));
      }
    } else {
      completer.Reply(fidl_clientsuite::wire::NonEmptyResultWithErrorClassification::WithFidlError(
          clienttest_util::ClassifyError(result)));
    }
  }

  void CallFlexibleTwoWay(CallFlexibleTwoWayRequestView request,
                          CallFlexibleTwoWayCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->FlexibleTwoWay();
    if (result.ok()) {
      completer.Reply(fidl_clientsuite::wire::EmptyResultClassification::WithSuccess(
          ::fidl_clientsuite::wire::Empty()));
    } else {
      completer.Reply(fidl_clientsuite::wire::EmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(result)));
    }
  }

  void CallFlexibleTwoWayFields(CallFlexibleTwoWayFieldsRequestView request,
                                CallFlexibleTwoWayFieldsCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->FlexibleTwoWayFields();
    if (result.ok()) {
      completer.Reply(
          fidl_clientsuite::wire::NonEmptyResultClassification::WithSuccess(result.value()));
    } else {
      completer.Reply(fidl_clientsuite::wire::NonEmptyResultClassification::WithFidlError(
          clienttest_util::ClassifyError(result)));
    }
  }

  void CallFlexibleTwoWayErr(CallFlexibleTwoWayErrRequestView request,
                             CallFlexibleTwoWayErrCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->FlexibleTwoWayErr();
    if (result.ok()) {
      if (result.value().is_ok()) {
        completer.Reply(fidl_clientsuite::wire::EmptyResultWithErrorClassification::WithSuccess(
            ::fidl_clientsuite::wire::Empty()));
      } else {
        completer.Reply(
            fidl_clientsuite::wire::EmptyResultWithErrorClassification::WithApplicationError(
                result.value().error_value()));
      }
    } else {
      completer.Reply(fidl_clientsuite::wire::EmptyResultWithErrorClassification::WithFidlError(
          clienttest_util::ClassifyError(result)));
    }
  }

  void CallFlexibleTwoWayFieldsErr(CallFlexibleTwoWayFieldsErrRequestView request,
                                   CallFlexibleTwoWayFieldsErrCompleter::Sync& completer) override {
    fidl::WireSyncClient client{std::move(request->target)};
    auto result = client->FlexibleTwoWayFieldsErr();
    if (result.ok()) {
      if (result.value().is_ok()) {
        completer.Reply(fidl_clientsuite::wire::NonEmptyResultWithErrorClassification::WithSuccess(
            *result.value().value()));
      } else {
        completer.Reply(
            fidl_clientsuite::wire::NonEmptyResultWithErrorClassification::WithApplicationError(
                result.value().error_value()));
      }
    } else {
      completer.Reply(fidl_clientsuite::wire::NonEmptyResultWithErrorClassification::WithFidlError(
          clienttest_util::ClassifyError(result)));
    }
  }

  void ReceiveClosedEvents(ReceiveClosedEventsRequestView request,
                           ReceiveClosedEventsCompleter::Sync& completer) override {
    class EventHandler : public fidl::WireSyncEventHandler<fidl_clientsuite::ClosedTarget> {
     public:
      void OnEventNoPayload() override {
        received_event = fidl_clientsuite::ClosedTargetEventReport::WithOnEventNoPayload({});
      }
      void OnEventStructPayload(
          ::fidl::WireEvent<::fidl_clientsuite::ClosedTarget::OnEventStructPayload>* event)
          override {
        received_event = fidl_clientsuite::ClosedTargetEventReport::WithOnEventStructPayload(
            fidl::ToNatural(*event));
      }
      void OnEventTablePayload(
          ::fidl::WireEvent<::fidl_clientsuite::ClosedTarget::OnEventTablePayload>* event)
          override {
        received_event = fidl_clientsuite::ClosedTargetEventReport::WithOnEventTablePayload(
            fidl::ToNatural(*event));
      }
      void OnEventUnionPayload(
          ::fidl::WireEvent<::fidl_clientsuite::ClosedTarget::OnEventUnionPayload>* event)
          override {
        received_event = fidl_clientsuite::ClosedTargetEventReport::WithOnEventUnionPayload(
            fidl::ToNatural(*event));
      }

      // Using the natural types for the reporter makes the ajar and open
      // targets simpler to test, because |fidl_clientsuite::UnknownEvent| is >8
      // bytes, so is out-of-line and requires an ObjectView in wire types. The
      // reporter isn't under test, so using natural types for it doesn't change
      // which system is under test.
      std::optional<fidl_clientsuite::ClosedTargetEventReport> received_event;
    };

    std::thread([client = fidl::WireSyncClient(std::move(request->target)),

                 reporter = fidl::SyncClient(std::move(request->reporter))]() mutable {
      while (true) {
        EventHandler event_handler;
        auto handle_result = client.HandleOneEvent(event_handler);
        if (!handle_result.ok()) {
          event_handler.received_event = fidl_clientsuite::ClosedTargetEventReport::WithFidlError(
              clienttest_util::ClassifyError(handle_result));
        }
        ZX_ASSERT(event_handler.received_event.has_value());
        auto report_result = reporter->ReportEvent(event_handler.received_event.value());
        if (report_result.is_error()) {
          ZX_PANIC("Could not report received event: %s",
                   report_result.error_value().lossy_description());
        }

        // If we got any error other than UnknownInteraction, we can stop
        // reporting events. However we want to keep the thread open until the
        // reporter closes so that the test case has the ability to
        // differentiate between the channel being closed due to the error vs
        // due to the channel going out of scope.
        if (!handle_result.ok() && handle_result.reason() != fidl::Reason::kUnknownMethod) {
          auto wait_status = reporter.client_end().channel().wait_one(
              ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), nullptr);
          ZX_ASSERT(wait_status == ZX_OK);
          break;
        }
      }
    }).detach();
    completer.Reply();
  }

  void ReceiveAjarEvents(ReceiveAjarEventsRequestView request,
                         ReceiveAjarEventsCompleter::Sync& completer) override {
    class EventHandler : public fidl::WireSyncEventHandler<fidl_clientsuite::AjarTarget> {
      void handle_unknown_event(
          fidl::UnknownEventMetadata<fidl_clientsuite::AjarTarget> metadata) override {
        ZX_ASSERT(!received_event.has_value());
        received_event = fidl_clientsuite::AjarTargetEventReport::WithUnknownEvent(
            {{.ordinal = metadata.event_ordinal}});
      }

     public:
      // Using the natural types for the reporter makes the ajar and open
      // targets simpler to test, because |fidl_clientsuite::UnknownEvent| is >8
      // bytes, so is out-of-line and requires an ObjectView in wire types. The
      // reporter isn't under test, so using natural types for it doesn't change
      // which system is under test.
      std::optional<fidl_clientsuite::AjarTargetEventReport> received_event;
    };

    std::thread([client = fidl::WireSyncClient(std::move(request->target)),
                 reporter = fidl::SyncClient(std::move(request->reporter))]() mutable {
      while (true) {
        EventHandler event_handler;
        auto handle_result = client.HandleOneEvent(event_handler);
        if (!handle_result.ok()) {
          event_handler.received_event = fidl_clientsuite::AjarTargetEventReport::WithFidlError(
              clienttest_util::ClassifyError(handle_result));
        }
        ZX_ASSERT(event_handler.received_event.has_value());
        auto report_result = reporter->ReportEvent(event_handler.received_event.value());
        if (report_result.is_error()) {
          ZX_PANIC("Could not report received event: %s",
                   report_result.error_value().lossy_description());
        }

        // If we got any error other than UnknownInteraction, we can stop
        // reporting events. However we want to keep the thread open until the
        // reporter closes so that the test case has the ability to
        // differentiate between the channel being closed due to the error vs
        // due to the channel going out of scope.
        if (!handle_result.ok() && handle_result.reason() != fidl::Reason::kUnknownMethod) {
          auto wait_status = reporter.client_end().channel().wait_one(
              ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), nullptr);
          ZX_ASSERT(wait_status == ZX_OK);
          break;
        }
      }
    }).detach();
    completer.Reply();
  }

  void ReceiveOpenEvents(ReceiveOpenEventsRequestView request,
                         ReceiveOpenEventsCompleter::Sync& completer) override {
    class EventHandler : public fidl::WireSyncEventHandler<fidl_clientsuite::OpenTarget> {
      void StrictEvent() override {
        ZX_ASSERT(!received_event.has_value());
        received_event = fidl_clientsuite::OpenTargetEventReport::WithStrictEvent({});
      }

      void FlexibleEvent() override {
        ZX_ASSERT(!received_event.has_value());
        received_event = fidl_clientsuite::OpenTargetEventReport::WithFlexibleEvent({});
      }

      void handle_unknown_event(
          fidl::UnknownEventMetadata<fidl_clientsuite::OpenTarget> metadata) override {
        ZX_ASSERT(!received_event.has_value());
        received_event = fidl_clientsuite::OpenTargetEventReport::WithUnknownEvent(
            {{.ordinal = metadata.event_ordinal}});
      }

     public:
      // Using the natural types for the reporter makes the ajar and open
      // targets simpler to test, because |fidl_clientsuite::UnknownEvent| is >8
      // bytes, so is out-of-line and requires an ObjectView in wire types. The
      // reporter isn't under test, so using natural types for it doesn't change
      // which system is under test.
      std::optional<fidl_clientsuite::OpenTargetEventReport> received_event;
    };

    std::thread([client = fidl::WireSyncClient(std::move(request->target)),
                 reporter = fidl::SyncClient(std::move(request->reporter))]() mutable {
      while (true) {
        EventHandler event_handler;
        auto handle_result = client.HandleOneEvent(event_handler);
        if (!handle_result.ok()) {
          event_handler.received_event = fidl_clientsuite::OpenTargetEventReport::WithFidlError(
              clienttest_util::ClassifyError(handle_result));
        }
        ZX_ASSERT(event_handler.received_event.has_value());
        auto report_result = reporter->ReportEvent(event_handler.received_event.value());
        if (report_result.is_error()) {
          ZX_PANIC("Could not report received event: %s",
                   report_result.error_value().lossy_description());
        }

        // If we got any error other than UnknownInteraction, we can stop
        // reporting events. However we want to keep the thread open until the
        // reporter closes so that the test case has the ability to
        // differentiate between the channel being closed due to the error vs
        // due to the channel going out of scope.
        if (!handle_result.ok() && handle_result.reason() != fidl::Reason::kUnknownMethod) {
          auto wait_status = reporter.client_end().channel().wait_one(
              ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), nullptr);
          ZX_ASSERT(wait_status == ZX_OK);
          break;
        }
      }
    }).detach();
    completer.Reply();
  }
};

int main(int argc, const char** argv) {
  std::cout << "CPP wire sync client: main" << std::endl;
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  auto outgoing = component::OutgoingDirectory(loop.dispatcher());
  ZX_ASSERT(outgoing.ServeFromStartupInfo().is_ok());
  auto result = outgoing.AddProtocol<fidl_clientsuite::Runner>(std::make_unique<RunnerServer>());
  ZX_ASSERT(result.is_ok());

  std::cout << "CPP wire sync client: ready!" << std::endl;
  return loop.Run();
}
