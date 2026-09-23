// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_WLANSOFTMAC_TESTS_FAKE_WLANSOFTMAC_SERVER_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_WLANSOFTMAC_TESTS_FAKE_WLANSOFTMAC_SERVER_H_

#include <fidl/fuchsia.wlan.sme/cpp/fidl.h>
#include <fidl/fuchsia.wlan.softmac/cpp/driver/fidl.h>
#include <lib/fidl/cpp/client.h>

namespace wlan::drivers::wlansoftmac {

class UnimplementedWlanSoftmacServer : public fdf::Server<fuchsia_wlan_softmac::WlanSoftmac> {
 public:
  explicit UnimplementedWlanSoftmacServer(
      fdf::ServerEnd<fuchsia_wlan_softmac::WlanSoftmac> server_end)
      : binding_(fdf_dispatcher_get_current_dispatcher(), std::move(server_end), this,
                 [](fidl::UnbindInfo info) {
                   ZX_ASSERT_MSG(info.is_peer_closed(), "Unexpectedly closed: %s",
                                 info.lossy_description());
                 }) {}

  void Start(StartRequest& request, StartCompleter::Sync& completer) override {
    completer.Reply(fit::error(ZX_ERR_UNAVAILABLE));
  }
  void Stop(StopCompleter::Sync& completer) override { ZX_PANIC("Not implemented."); }
  void QueueTx(QueueTxRequest& request, QueueTxCompleter::Sync& completer) override {
    completer.Reply(fit::error(ZX_ERR_UNAVAILABLE));
  }
  void Query(QueryCompleter::Sync& completer) override {
    completer.Reply(fit::error(ZX_ERR_UNAVAILABLE));
  }
  void QueryDiscoverySupport(QueryDiscoverySupportCompleter::Sync& completer) override {
    completer.Reply(fit::error(ZX_ERR_UNAVAILABLE));
  }
  void QueryMacSublayerSupport(QueryMacSublayerSupportCompleter::Sync& completer) override {
    completer.Reply(fit::error(ZX_ERR_UNAVAILABLE));
  }
  void QuerySecuritySupport(QuerySecuritySupportCompleter::Sync& completer) override {
    completer.Reply(fit::error(ZX_ERR_UNAVAILABLE));
  }
  void QuerySpectrumManagementSupport(
      QuerySpectrumManagementSupportCompleter::Sync& completer) override {
    completer.Reply(fit::error(ZX_ERR_UNAVAILABLE));
  }
  void SetChannel(SetChannelRequest& request, SetChannelCompleter::Sync& completer) override {
    completer.Reply(fit::error(ZX_ERR_UNAVAILABLE));
  }
  void JoinBss(JoinBssRequest& request, JoinBssCompleter::Sync& completer) override {
    completer.Reply(fit::error(ZX_ERR_UNAVAILABLE));
  }
  void EnableBeaconing(EnableBeaconingRequest& request,
                       EnableBeaconingCompleter::Sync& completer) override {
    completer.Reply(fit::error(ZX_ERR_UNAVAILABLE));
  }
  void DisableBeaconing(DisableBeaconingCompleter::Sync& completer) override {
    completer.Reply(fit::error(ZX_ERR_UNAVAILABLE));
  }
  void InstallKey(InstallKeyRequest& request, InstallKeyCompleter::Sync& completer) override {
    completer.Reply(fit::error(ZX_ERR_UNAVAILABLE));
  }
  void NotifyAssociationComplete(NotifyAssociationCompleteRequest& request,
                                 NotifyAssociationCompleteCompleter::Sync& completer) override {
    completer.Reply(fit::error(ZX_ERR_UNAVAILABLE));
  }
  void ClearAssociation(ClearAssociationRequest& request,
                        ClearAssociationCompleter::Sync& completer) override {
    completer.Reply(fit::error(ZX_ERR_UNAVAILABLE));
  }
  void StartPassiveScan(StartPassiveScanRequest& request,
                        StartPassiveScanCompleter::Sync& completer) override {
    completer.Reply(fit::error(ZX_ERR_UNAVAILABLE));
  }
  void StartActiveScan(StartActiveScanRequest& request,
                       StartActiveScanCompleter::Sync& completer) override {
    completer.Reply(fit::error(ZX_ERR_UNAVAILABLE));
  }
  void CancelScan(CancelScanRequest& request, CancelScanCompleter::Sync& completer) override {
    completer.Reply(fit::error(ZX_ERR_UNAVAILABLE));
  }
  void UpdateWmmParameters(UpdateWmmParametersRequest& request,
                           UpdateWmmParametersCompleter::Sync& completer) override {
    completer.Reply(fit::error(ZX_ERR_UNAVAILABLE));
  }
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_wlan_softmac::WlanSoftmac> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

 private:
  fdf::ServerBinding<fuchsia_wlan_softmac::WlanSoftmac> binding_;
};

class BasicWlanSoftmacServer : public UnimplementedWlanSoftmacServer {
 public:
  using UnimplementedWlanSoftmacServer::UnimplementedWlanSoftmacServer;

  void Start(StartRequest& request, StartCompleter::Sync& completer) override {
    // Acquire/Construct the WlanSoftmacIfc, UsmeBootstrap, and GenericSme endpoints.
    softmac_ifc_client_endpoint_ = std::move(request.ifc());
    auto usme_bootstrap_endpoints = fidl::CreateEndpoints<fuchsia_wlan_sme::UsmeBootstrap>();
    usme_bootstrap_client_ =
        fidl::Client(std::move(usme_bootstrap_endpoints.value().client),
                     fdf_dispatcher_get_async_dispatcher(fdf_dispatcher_get_current_dispatcher()));
    auto generic_sme_endpoints = fidl::CreateEndpoints<fuchsia_wlan_sme::GenericSme>();
    generic_sme_client_endpoint_ = std::move(generic_sme_endpoints.value().client);

    // Make the required UsmeBootstrap.Start during driver Start.
    usme_bootstrap_client_.value()
        ->Start(fuchsia_wlan_sme::UsmeBootstrapStartRequest(
            std::move(generic_sme_endpoints.value().server),
            fuchsia_wlan_sme::LegacyPrivacySupport(false, false)))
        .Then([&](fidl::Result<fuchsia_wlan_sme::UsmeBootstrap::Start>& result) mutable {
          ZX_ASSERT(result.is_ok());
          inspect_vmo_ = std::move(result->inspect_vmo());
        });

    completer.Reply(fit::ok(fuchsia_wlan_softmac::WlanSoftmacStartResponse(
        usme_bootstrap_endpoints.value().server.TakeChannel())));
  }

  void Query(QueryCompleter::Sync& completer) override {
    fuchsia_wlan_softmac::WlanSoftmacQueryResponse response;
    response.sta_addr(std::array<uint8_t, 6>{8, 8, 8, 8, 8, 8})
        .mac_role(fuchsia_wlan_common::WlanMacRole::kClient)
        .supported_phys(std::vector<fuchsia_wlan_ieee80211::WlanPhyType>{
            fuchsia_wlan_ieee80211::WlanPhyType::kDsss})
        .hardware_capability(0)
        .factory_addr({std::array<uint8_t, 6>{8, 8, 8, 8, 8, 8}});

    fuchsia_wlan_softmac::WlanSoftmacBandCapability band_capability;
    band_capability.band(fuchsia_wlan_ieee80211::WlanBand::kTwoGhz)
        .basic_rates(std::vector<uint8_t>{1})
        .primary_channels(std::vector<fuchsia_wlan_ieee80211::ChannelNumber>{
            fuchsia_wlan_ieee80211::ChannelNumber()
                .band(fuchsia_wlan_ieee80211::WlanBand::kTwoGhz)
                .number(1)});
    response.band_caps(
        std::vector<fuchsia_wlan_softmac::WlanSoftmacBandCapability>{band_capability});
    completer.Reply(fit::ok(std::move(response)));
  }

  void QueryDiscoverySupport(QueryDiscoverySupportCompleter::Sync& completer) override {
    fuchsia_wlan_softmac::DiscoverySupport response;
    completer.Reply(fit::ok(response));
  }

  void QueryMacSublayerSupport(QueryMacSublayerSupportCompleter::Sync& completer) override {
    fuchsia_wlan_common::MacSublayerSupport response;
    response.rate_selection_offload(
        fuchsia_wlan_common::RateSelectionOffloadExtension().supported(true));
    response.data_plane(fuchsia_wlan_common::DataPlaneExtension().data_plane_type(
        fuchsia_wlan_common::DataPlaneType::kEthernetDevice));
    response.device(
        fuchsia_wlan_common::DeviceExtension()
            .is_synthetic(true)
            .mac_implementation_type(fuchsia_wlan_common::MacImplementationType::kSoftmac)
            .tx_status_report_supported(false));
    completer.Reply(fit::ok(response));
  }

  void QuerySecuritySupport(QuerySecuritySupportCompleter::Sync& completer) override {
    fuchsia_wlan_common::SecuritySupport response;
    response.sae(fuchsia_wlan_common::SaeFeature()
                     .driver_handler_supported(false)
                     .sme_handler_supported(true)
                     .hash_to_element_supported(false));
    response.mfp(fuchsia_wlan_common::MfpFeature().supported(false));
    response.owe(fuchsia_wlan_common::OweFeature().supported(false));
    completer.Reply(fit::ok(response));
  }

  void QuerySpectrumManagementSupport(
      QuerySpectrumManagementSupportCompleter::Sync& completer) override {
    fuchsia_wlan_common::SpectrumManagementSupport response;
    completer.Reply(fit::ok(response));
  }

  void DropWlanSoftmacIfcClient() { softmac_ifc_client_endpoint_.reset(); }
  void DropGenericSmeClient() { generic_sme_client_endpoint_.reset(); }

 private:
  std::optional<fdf::ClientEnd<::fuchsia_wlan_softmac::WlanSoftmacIfc>>
      softmac_ifc_client_endpoint_;
  std::optional<fidl::Client<fuchsia_wlan_sme::UsmeBootstrap>> usme_bootstrap_client_;
  std::optional<fidl::ClientEnd<fuchsia_wlan_sme::GenericSme>> generic_sme_client_endpoint_;
  std::optional<zx::vmo> inspect_vmo_;
};

}  // namespace wlan::drivers::wlansoftmac

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_WLANSOFTMAC_TESTS_FAKE_WLANSOFTMAC_SERVER_H_
