// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.wlan.fullmac/cpp/driver/fidl.h>
#include <fidl/test.wlan.testcontroller/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/logging/cpp/structured_logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/stdcompat/source_location.h>
#include <lib/sync/cpp/completion.h>

#include <map>
#include <sstream>
#include <vector>

#include <wlan/drivers/fidl_bridge.h>

namespace wlan_testcontroller {

using ::wlan::drivers::fidl_bridge::FidlErrorToStatus;
using ::wlan::drivers::fidl_bridge::ForwardResult;

// Server that forwards WlanFullmacImplIfc calls from the test suite to wlanif.
class WlanFullmacImplIfcBridgeServer
    : public fidl::Server<fuchsia_wlan_fullmac::WlanFullmacImplIfc> {
  using WlanFullmacImplIfc = fuchsia_wlan_fullmac::WlanFullmacImplIfc;

 public:
  explicit WlanFullmacImplIfcBridgeServer(
      async_dispatcher_t* async_dispatcher,
      fidl::ServerEnd<fuchsia_wlan_fullmac::WlanFullmacImplIfc> server_end,
      fidl::ClientEnd<fuchsia_wlan_fullmac::WlanFullmacImplIfc> bridge_client_end)
      : binding_(async_dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure),
        bridge_client_(std::move(bridge_client_end), async_dispatcher) {
    WLAN_TRACE_DURATION();
  }

  void OnScanResult(OnScanResultRequest& request, OnScanResultCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->OnScanResult(request).Then(
        ForwardResult<WlanFullmacImplIfc::OnScanResult>(completer.ToAsync()));
  }
  void OnScanEnd(OnScanEndRequest& request, OnScanEndCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->OnScanEnd(request).Then(
        ForwardResult<WlanFullmacImplIfc::OnScanEnd>(completer.ToAsync()));
  }
  void OnScheduledScanMatchesAvailable(
      OnScheduledScanMatchesAvailableRequest& request,
      OnScheduledScanMatchesAvailableCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    auto result = bridge_client_->OnScheduledScanMatchesAvailable(request);
    if (result.is_error()) {
      FDF_SLOG(ERROR, "Failed to forward OnScheduledScanMatchesAvailable",
               KV("status", result.error_value().FormatDescription()));
    }
  }
  void OnScheduledScanStoppedByFirmware(
      OnScheduledScanStoppedByFirmwareRequest& request,
      OnScheduledScanStoppedByFirmwareCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    auto result = bridge_client_->OnScheduledScanStoppedByFirmware(request);
    if (result.is_error()) {
      FDF_SLOG(ERROR, "Failed to forward OnScheduledScanStoppedByFirmware",
               KV("status", result.error_value().FormatDescription()));
    }
  }
  void ConnectConf(ConnectConfRequest& request, ConnectConfCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->ConnectConf(request).Then(
        ForwardResult<WlanFullmacImplIfc::ConnectConf>(completer.ToAsync()));
  }
  void RoamConf(RoamConfRequest& request, RoamConfCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->RoamConf(request).Then(
        ForwardResult<WlanFullmacImplIfc::RoamConf>(completer.ToAsync()));
  }
  void RoamStartInd(RoamStartIndRequest& request, RoamStartIndCompleter::Sync& completer) override {
  }
  void RoamResultInd(RoamResultIndRequest& request,
                     RoamResultIndCompleter::Sync& completer) override {}
  void AuthInd(AuthIndRequest& request, AuthIndCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->AuthInd(request).Then(
        ForwardResult<WlanFullmacImplIfc::AuthInd>(completer.ToAsync()));
  }
  void DeauthConf(DeauthConfRequest& request, DeauthConfCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->DeauthConf(request).Then(
        ForwardResult<WlanFullmacImplIfc::DeauthConf>(completer.ToAsync()));
  }
  void DeauthInd(DeauthIndRequest& request, DeauthIndCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->DeauthInd(request).Then(
        ForwardResult<WlanFullmacImplIfc::DeauthInd>(completer.ToAsync()));
  }
  void AssocInd(AssocIndRequest& request, AssocIndCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->AssocInd(request).Then(
        ForwardResult<WlanFullmacImplIfc::AssocInd>(completer.ToAsync()));
  }
  void DisassocConf(DisassocConfRequest& request, DisassocConfCompleter::Sync& completer) override {
  }
  void DisassocInd(DisassocIndRequest& request, DisassocIndCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->DisassocInd(request).Then(
        ForwardResult<WlanFullmacImplIfc::DisassocInd>(completer.ToAsync()));
  }
  void StartConf(StartConfRequest& request, StartConfCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->StartConf(request).Then(
        ForwardResult<WlanFullmacImplIfc::StartConf>(completer.ToAsync()));
  }
  void StopConf(StopConfRequest& request, StopConfCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->StopConf(request).Then(
        ForwardResult<WlanFullmacImplIfc::StopConf>(completer.ToAsync()));
  }
  void EapolConf(EapolConfRequest& request, EapolConfCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->EapolConf(request).Then(
        ForwardResult<WlanFullmacImplIfc::EapolConf>(completer.ToAsync()));
  }
  void OnChannelSwitch(OnChannelSwitchRequest& request,
                       OnChannelSwitchCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->OnChannelSwitch(request).Then(
        ForwardResult<WlanFullmacImplIfc::OnChannelSwitch>(completer.ToAsync()));
  }
  void SignalReport(SignalReportRequest& request, SignalReportCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->SignalReport(request).Then(
        ForwardResult<WlanFullmacImplIfc::SignalReport>(completer.ToAsync()));
  }
  void EapolInd(EapolIndRequest& request, EapolIndCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->EapolInd(request).Then(
        ForwardResult<WlanFullmacImplIfc::EapolInd>(completer.ToAsync()));
  }
  void OnPmkAvailable(OnPmkAvailableRequest& request,
                      OnPmkAvailableCompleter::Sync& completer) override {}
  void SaeHandshakeInd(SaeHandshakeIndRequest& request,
                       SaeHandshakeIndCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->SaeHandshakeInd(request).Then(
        ForwardResult<WlanFullmacImplIfc::SaeHandshakeInd>(completer.ToAsync()));
  }
  void SaeFrameRx(SaeFrameRxRequest& request, SaeFrameRxCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->SaeFrameRx(request).Then(
        ForwardResult<WlanFullmacImplIfc::SaeFrameRx>(completer.ToAsync()));
  }
  void OnWmmStatusResp(OnWmmStatusRespRequest& request,
                       OnWmmStatusRespCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->OnWmmStatusResp(request).Then(
        ForwardResult<WlanFullmacImplIfc::OnWmmStatusResp>(completer.ToAsync()));
  }

 private:
  fidl::ServerBinding<fuchsia_wlan_fullmac::WlanFullmacImplIfc> binding_;
  fidl::Client<fuchsia_wlan_fullmac::WlanFullmacImplIfc> bridge_client_;
};

// Server that forwards WlanFullmacImpl calls from wlanif to the test suite.
class WlanFullmacImplBridgeServer : public fidl::Server<fuchsia_wlan_fullmac::WlanFullmacImpl> {
 private:
  using WlanFullmacImpl = fuchsia_wlan_fullmac::WlanFullmacImpl;

 public:
  explicit WlanFullmacImplBridgeServer(
      async_dispatcher_t* async_dispatcher,
      fidl::ServerEnd<fuchsia_wlan_fullmac::WlanFullmacImpl> server_end,
      fidl::ClientEnd<fuchsia_wlan_fullmac::WlanFullmacImpl> bridge_client_end,
      fidl::ClientEnd<fuchsia_driver_framework::NodeController> controller_client_end)
      : binding_(async_dispatcher, std::move(server_end), this,
                 std::mem_fn(&WlanFullmacImplBridgeServer::OnUnbind)),
        bridge_client_(std::move(bridge_client_end), async_dispatcher),
        async_dispatcher_(async_dispatcher),
        controller_(std::move(controller_client_end), async_dispatcher_) {
    WLAN_TRACE_DURATION();
  }

  void OnUnbind(fidl::UnbindInfo info) {
    WLAN_TRACE_DURATION();
    if (!info.is_peer_closed()) {
      FDF_SLOG(WARNING, "WlanFullmacImplBridge unbinding due to unexpected reason",
               KV("reason", info.FormatDescription()));
    }
    if (!unbind_callback_.has_value()) {
      FDF_SLOG(
          ERROR,
          "Unexpected server unbinding: WlanFullmacImplBridge does not have an unbind callback",
          KV("reason", info.FormatDescription()));
      return;
    }
    unbind_callback_.value()();
  }

  void Init(InitRequest& request, InitCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    // Init has to swap out the fdf::ClientEnd and instead pass along a fidl::ClientEnd to the
    // bridge server. This is necessary because the test case runs in a non-driver component,
    // which cannot use the fdf::ClientEnd.
    zx::result endpoints = fidl::CreateEndpoints<fuchsia_wlan_fullmac::WlanFullmacImplIfc>();
    if (endpoints.is_error()) {
      fdf::error("Could not create WlanFullmacImplIfc endpoints: {}", endpoints);
      completer.Reply(endpoints.take_error());
      return;
    }

    // Create and bind ifc bridge server.
    ifc_bridge_server_ = std::make_unique<WlanFullmacImplIfcBridgeServer>(
        async_dispatcher_, std::move(endpoints->server), std::move(*request.ifc()));

    InitRequest bridge_init;
    bridge_init.ifc() = std::move(endpoints->client);
    bridge_client_->Init(std::move(bridge_init))
        .Then(
            [completer = completer.ToAsync()](fidl::Result<WlanFullmacImpl::Init>& result) mutable {
              WLAN_LAMBDA_TRACE_DURATION("WlanFullmacImpl::Init callback");
              // Unlike all the other methods in WlanFullmacImpl,
              // WlanFullmacImpl::Init and WlanFullmacImpl::Init results are
              // considered different types by the compiler. So we need to convert between the two
              // types manually here.
              if (result.is_error()) {
                auto& error = result.error_value();
                FDF_SLOG(ERROR, "Init failed", KV("status", error.FormatDescription()));
                completer.Reply(zx::error(FidlErrorToStatus(error)));
              } else {
                // Forward the SME channel provided by test suite
                fuchsia_wlan_fullmac::WlanFullmacImplInitResponse response;
                response.sme_channel() = std::move(result->sme_channel());
                completer.Reply(zx::ok(std::move(response)));
              }
            });
  }

  void Query(QueryCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->Query().Then(ForwardResult<WlanFullmacImpl::Query>(completer.ToAsync()));
  }
  void QuerySecuritySupport(QuerySecuritySupportCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->QuerySecuritySupport().Then(
        ForwardResult<WlanFullmacImpl::QuerySecuritySupport>(completer.ToAsync()));
  }
  void QuerySpectrumManagementSupport(
      QuerySpectrumManagementSupportCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->QuerySpectrumManagementSupport().Then(
        ForwardResult<WlanFullmacImpl::QuerySpectrumManagementSupport>(completer.ToAsync()));
  }
  void QueryTelemetrySupport(QueryTelemetrySupportCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->QueryTelemetrySupport().Then(
        ForwardResult<WlanFullmacImpl::QueryTelemetrySupport>(completer.ToAsync()));
  }
  void StartScan(StartScanRequest& request, StartScanCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->StartScan(request).Then(
        ForwardResult<WlanFullmacImpl::StartScan>(completer.ToAsync()));
  }
  void StartScheduledScan(StartScheduledScanRequest& request,
                          StartScheduledScanCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->StartScheduledScan(request).Then(
        ForwardResult<WlanFullmacImpl::StartScheduledScan>(completer.ToAsync()));
  }
  void StopScheduledScan(StopScheduledScanRequest& request,
                         StopScheduledScanCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->StopScheduledScan(request).Then(
        ForwardResult<WlanFullmacImpl::StopScheduledScan>(completer.ToAsync()));
  }
  void Connect(ConnectRequest& request, ConnectCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->Connect(request).Then(
        ForwardResult<WlanFullmacImpl::Connect>(completer.ToAsync()));
  }
  void Reconnect(ReconnectRequest& request, ReconnectCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->Reconnect(request).Then(
        ForwardResult<WlanFullmacImpl::Reconnect>(completer.ToAsync()));
  }
  void Roam(RoamRequest& request, RoamCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->Roam(request).Then(ForwardResult<WlanFullmacImpl::Roam>(completer.ToAsync()));
  }
  void AuthResp(AuthRespRequest& request, AuthRespCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->AuthResp(request).Then(
        ForwardResult<WlanFullmacImpl::AuthResp>(completer.ToAsync()));
  }
  void Deauth(DeauthRequest& request, DeauthCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->Deauth(request).Then(
        ForwardResult<WlanFullmacImpl::Deauth>(completer.ToAsync()));
  }
  void AssocResp(AssocRespRequest& request, AssocRespCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->AssocResp(request).Then(
        ForwardResult<WlanFullmacImpl::AssocResp>(completer.ToAsync()));
  }
  void Disassoc(DisassocRequest& request, DisassocCompleter::Sync& completer) override {}
  void StartBss(StartBssRequest& request, StartBssCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->StartBss(request).Then(
        ForwardResult<WlanFullmacImpl::StartBss>(completer.ToAsync()));
  }
  void StopBss(StopBssRequest& request, StopBssCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->StopBss(request).Then(
        ForwardResult<WlanFullmacImpl::StopBss>(completer.ToAsync()));
  }
  void SetKeys(SetKeysRequest& request, SetKeysCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->SetKeys(request).Then(
        ForwardResult<WlanFullmacImpl::SetKeys>(completer.ToAsync()));
  }
  void EapolTx(EapolTxRequest& request, EapolTxCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->EapolTx(request).Then(
        ForwardResult<WlanFullmacImpl::EapolTx>(completer.ToAsync()));
  }
  void GetIfaceStats(GetIfaceStatsCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->GetIfaceStats().Then(
        ForwardResult<WlanFullmacImpl::GetIfaceStats>(completer.ToAsync()));
  }
  void GetIfaceHistogramStats(GetIfaceHistogramStatsCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->GetIfaceHistogramStats().Then(
        ForwardResult<WlanFullmacImpl::GetIfaceHistogramStats>(completer.ToAsync()));
  }
  void GetSignalReport(GetSignalReportCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->GetSignalReport().Then(
        ForwardResult<WlanFullmacImpl::GetSignalReport>(completer.ToAsync()));
  }
  void SaeHandshakeResp(SaeHandshakeRespRequest& request,
                        SaeHandshakeRespCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->SaeHandshakeResp(request).Then(
        ForwardResult<WlanFullmacImpl::SaeHandshakeResp>(completer.ToAsync()));
  }
  void SaeFrameTx(SaeFrameTxRequest& request, SaeFrameTxCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->SaeFrameTx(request).Then(
        ForwardResult<WlanFullmacImpl::SaeFrameTx>(completer.ToAsync()));
  }
  void WmmStatusReq(WmmStatusReqCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->WmmStatusReq().Then(
        ForwardResult<WlanFullmacImpl::WmmStatusReq>(completer.ToAsync()));
  }
  void OnLinkStateChanged(OnLinkStateChangedRequest& request,
                          OnLinkStateChangedCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->OnLinkStateChanged(request).Then(
        ForwardResult<WlanFullmacImpl::OnLinkStateChanged>(completer.ToAsync()));
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_wlan_fullmac::WlanFullmacImpl> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    FDF_SLOG(ERROR, "Unexpected call to unsupported method",
             KV("ordinal", metadata.method_ordinal));
  }

  void SetMacAddress(SetMacAddressRequest& request,
                     SetMacAddressCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->SetMacAddress(request).Then(
        ForwardResult<WlanFullmacImpl::SetMacAddress>(completer.ToAsync()));
  }

  // APF (Android Packet Filter) extensions
  void QueryApfPacketFilterSupport(QueryApfPacketFilterSupportCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->QueryApfPacketFilterSupport().Then(
        ForwardResult<WlanFullmacImpl::QueryApfPacketFilterSupport>(completer.ToAsync()));
  }

  void InstallApfPacketFilter(InstallApfPacketFilterRequest& request,
                              InstallApfPacketFilterCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->InstallApfPacketFilter(request).Then(
        ForwardResult<WlanFullmacImpl::InstallApfPacketFilter>(completer.ToAsync()));
  }

  void ReadApfPacketFilterData(ReadApfPacketFilterDataCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->ReadApfPacketFilterData().Then(
        ForwardResult<WlanFullmacImpl::ReadApfPacketFilterData>(completer.ToAsync()));
  }

  void SetApfPacketFilterEnabled(SetApfPacketFilterEnabledRequest& request,
                                 SetApfPacketFilterEnabledCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->SetApfPacketFilterEnabled(request).Then(
        ForwardResult<WlanFullmacImpl::SetApfPacketFilterEnabled>(completer.ToAsync()));
  }

  void GetApfPacketFilterEnabled(GetApfPacketFilterEnabledCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->GetApfPacketFilterEnabled().Then(
        ForwardResult<WlanFullmacImpl::GetApfPacketFilterEnabled>(completer.ToAsync()));
  }

  void GetScheduledScanEnabled(GetScheduledScanEnabledCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    bridge_client_->GetScheduledScanEnabled().Then(
        ForwardResult<WlanFullmacImpl::GetScheduledScanEnabled>(completer.ToAsync()));
  }

  // Calling |RemoveChild| will cause this server to eventually unbind.
  // |unbind_callback| will run in |OnUnbind|.
  zx::result<> RemoveChild(fit::closure unbind_callback) {
    WLAN_TRACE_DURATION();
    auto result = controller_->Remove();
    if (result.is_error()) {
      FDF_SLOG(ERROR, "Failed to remove child",
               KV("reason", result.error_value().FormatDescription()));
      return zx::error(result.error_value().status());
    }

    ZX_ASSERT(!unbind_callback_.has_value());
    unbind_callback_.emplace(std::move(unbind_callback));
    return zx::ok();
  }

 private:
  fidl::ServerBinding<fuchsia_wlan_fullmac::WlanFullmacImpl> binding_;
  fidl::Client<fuchsia_wlan_fullmac::WlanFullmacImpl> bridge_client_;
  async_dispatcher_t* async_dispatcher_{};
  std::unique_ptr<WlanFullmacImplIfcBridgeServer> ifc_bridge_server_;
  fidl::Client<fuchsia_driver_framework::NodeController> controller_;

  // Callback that runs when the bridge server is unbound.
  std::optional<fit::closure> unbind_callback_;
};

class TestController : public fdf::DriverBase2,
                       public fidl::Server<test_wlan_testcontroller::TestController> {
  static constexpr std::string_view kDriverName = "wlan_testcontroller";

 public:
  explicit TestController()
      : fdf::DriverBase2(kDriverName),
        devfs_connector_(fit::bind_member<&TestController::Serve>(this)) {}

  zx::result<> Start(fdf::DriverContext context) override {
    WLAN_TRACE_DURATION();

    node_.Bind(take_node());
    fidl::Arena arena;

    zx::result connector = devfs_connector_.Bind(dispatcher());
    if (connector.is_error()) {
      return connector.take_error();
    }

    // By calling AddChild with devfs_args, the child driver will be discoverable through devfs.
    auto args =
        fuchsia_driver_framework::NodeAddArgs({.name = std::string(kDriverName),
                                               .devfs_args = fuchsia_driver_framework::DevfsAddArgs(
                                                   {.connector = std::move(connector.value())})});

    auto controller_endpoints = fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

    auto result = node_->AddChild({std::move(args), std::move(controller_endpoints.server), {}});
    if (result.is_error()) {
      auto& error = result.error_value();
      FDF_SLOG(ERROR, "Failed to add child", KV("status", error.FormatDescription()));

      // AddChild's domain error is a NodeError, not a zx_status_t
      return zx::error(error.is_domain_error() ? ZX_ERR_INTERNAL
                                               : error.framework_error().status());
    }

    return zx::ok();
  }

  // Creates a new fullmac driver.
  // On success, this is guaranteed to complete only after wlanif has binded.
  // The user can expect that the bridge channels are open and wlanif is up and running once this
  // completes.
  void CreateFullmac(CreateFullmacRequest& request,
                     CreateFullmacCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();

    // Generate a unique child name for the new fullmac child.
    uint32_t id = next_fullmac_id_++;
    std::string child_name = FullmacChildName(id);

    zx::result controller_endpoints =
        fidl::CreateEndpoints<fuchsia_driver_framework::NodeController>();
    ZX_ASSERT(controller_endpoints.is_ok());

    // The async completer is stored in a shared pointer so that we can pass it to the protocol
    // handler callback while also retaining a reference to it in CreateFullmac.
    // This lets us complete the call in the protocol handler on success and complete the call in
    // CreateFullmac on error.
    auto async_completer = std::make_shared<CreateFullmacCompleter::Async>(completer.ToAsync());

    auto protocol_handler =
        [this, async_completer, id, bridge_client = std::move(request.bridge_client()),
         controller_client = std::move(controller_endpoints->client)](
            fidl::ServerEnd<fuchsia_wlan_fullmac::WlanFullmacImpl> server_end) mutable {
          // It's assumed that this protocol handler runs exactly once, meaning that each call to
          // CreateFullmac should result in exactly one call to this protocol handler.
          // It's also assumed that this protocol handler runs on the same dispatcher as
          // CreateFullmac and cannot run concurrently with CreateFullmac.

          // Ensure no duplicate bridges and all ids are unique.
          ZX_ASSERT(fullmac_bridges_.find(id) == fullmac_bridges_.end());

          // Check that async completer exists and that no other references to async_completer
          // exist. This ensures that once this callback runs we can drop async_completer.
          ZX_ASSERT(async_completer && async_completer.use_count() == 1);

          fullmac_bridges_.try_emplace(
              id, std::make_unique<WlanFullmacImplBridgeServer>(dispatcher(), std::move(server_end),
                                                                std::move(bridge_client),
                                                                std::move(controller_client)));
          async_completer->Reply(zx::ok(id));
          async_completer.reset();
        };

    fuchsia_wlan_fullmac::Service::InstanceHandler handler(
        {.wlan_fullmac_impl = std::move(protocol_handler)});

    zx::result result =
        outgoing()->AddService<fuchsia_wlan_fullmac::Service>(std::move(handler), child_name);
    if (result.is_error()) {
      fdf::error("Failed to add fullmac service: {}", result);
      async_completer->Reply(result.take_error());
      async_completer.reset();
      return;
    }

    auto offers = std::vector{fdf::MakeOffer2<fuchsia_wlan_fullmac::Service>(child_name)};

    auto args = fuchsia_driver_framework::NodeAddArgs({
        .name = child_name,
        .offers2 = std::move(offers),
    });

    auto add_child_result =
        node_->AddChild({std::move(args), std::move(controller_endpoints->server), {}});
    if (add_child_result.is_error()) {
      auto& error = add_child_result.error_value();
      FDF_SLOG(ERROR, "Failed to add child", KV("status", error.FormatDescription()));

      // AddChild's domain error is a NodeError, not a zx_status_t
      async_completer->Reply(zx::error(
          (error.is_domain_error() ? ZX_ERR_INTERNAL : error.framework_error().status())));
      async_completer.reset();
      return;
    }
  }

  // Removes the WlanFullmacImpl service which initiates teardown of wlanif.
  // On success, this is guaranteed to complete only after wlanif has been fully torn down.
  // The user can expect that the bridge channels are closed once this call completes.
  void DeleteFullmac(DeleteFullmacRequest& request,
                     DeleteFullmacCompleter::Sync& completer) override {
    WLAN_TRACE_DURATION();
    auto bridge_iter = fullmac_bridges_.find(request.id());
    if (bridge_iter == fullmac_bridges_.end()) {
      FDF_SLOG(ERROR, "Fullmac driver does not exist", KV("id", request.id()));
      completer.Reply(zx::error(ZX_ERR_NOT_FOUND));
      return;
    }

    WlanFullmacImplBridgeServer& bridge = *bridge_iter->second;
    auto node_remove_result = bridge.RemoveChild(
        // After the bridge server unbinds, we know that wlanif is fully torn down.
        // It is then safe to delete the bridge server and reply to the FIDL call.
        [this, id = request.id(), completer = completer.ToAsync()]() mutable {
          // This callback posts a task on |dispatcher()| because the |completer| has to run on this
          // thread. The bridge server's unbind callback does not run on this thread and will
          // fail an assert if we reply to the |completer| directly in the unbind callback.
          async::PostTask(dispatcher(), [this, id, completer = std::move(completer)]() mutable {
            WLAN_LAMBDA_TRACE_DURATION("WlanFullmacImplBridge unbind callback");
            ZX_ASSERT(fullmac_bridges_.find(id) != fullmac_bridges_.end());
            fullmac_bridges_.erase(id);
            completer.Reply(zx::ok());
          });
        });

    if (node_remove_result.is_error()) {
      completer.Reply(zx::error(node_remove_result.error_value()));
      return;
    }

    std::string child_name = FullmacChildName(request.id());
    auto service_remove_result =
        outgoing()->RemoveService<fuchsia_wlan_fullmac::Service>(child_name);
    if (service_remove_result.is_error()) {
      FDF_SLOG(ERROR, "Could not remove fullmac service", KV("id", request.id()),
               KV("status", service_remove_result.status_string()));
      completer.Reply(zx::error(service_remove_result.error_value()));
      return;
    }
  }

 private:
  static std::string FullmacChildName(test_wlan_testcontroller::FullmacId id) {
    std::stringstream ss;
    ss << "fullmac-child-" << id;
    return ss.str();
  }

  fidl::SyncClient<fuchsia_driver_framework::Node> node_;

  // Holds bindings to TestController, which all bind to this class
  fidl::ServerBindingGroup<test_wlan_testcontroller::TestController> bindings_;

  void Serve(fidl::ServerEnd<test_wlan_testcontroller::TestController> server) {
    bindings_.AddBinding(dispatcher(), std::move(server), this, fidl::kIgnoreBindingClosure);
  }

  // devfs_connector_ lets the class serve the TestController protocol over devfs.
  driver_devfs::Connector<test_wlan_testcontroller::TestController> devfs_connector_;

  // Used to generate a unique ID for each created fullmac driver.
  test_wlan_testcontroller::FullmacId next_fullmac_id_ = 0;

  // Maps from id -> bridge server.
  // This should only be accessed from the default dispatcher.
  std::map<test_wlan_testcontroller::FullmacId, std::unique_ptr<WlanFullmacImplBridgeServer>>
      fullmac_bridges_;
};

}  // namespace wlan_testcontroller

FUCHSIA_DRIVER_EXPORT2(wlan_testcontroller::TestController);
