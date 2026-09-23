// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "wlantap-phy-impl.h"

#include <fidl/fuchsia.wlan.internal/cpp/fidl.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fidl/cpp/wire/status.h>
#include <lib/fit/defer.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <array>
#include <cstddef>
#include <string>
#include <type_traits>
#include <utility>

#include <wlan/drivers/log.h>

#include "utils.h"

namespace wlan {

std::string CountryCodeToStr(
    const std::array<uint8_t, fuchsia_wlan_internal::kCountryCodeLen>& country_code) {
  auto data = country_code.data();
  if (std::isprint(data[0]) && std::isprint(data[1])) {
    return std::string(reinterpret_cast<const char*>(data), fuchsia_wlan_internal::kCountryCodeLen);
  }
  return "(" + std::to_string(data[0]) + ")(" + std::to_string(data[1]) + ")";
}

template <class... Ts>
struct overloaded : Ts... {
  using Ts::operator()...;
};
template <class... Ts>
overloaded(Ts...) -> overloaded<Ts...>;

std::shared_ptr<WlanPhyDevice> WlanPhyDevice::New(
    WlantapDriverContext context, zx::channel user_channel,
    const fuchsia_wlan_tap::WlantapPhyConfig& phy_config, NodeControllerClient phy_controller) {
  WLAN_TRACE_DURATION();
  auto device = std::shared_ptr<WlanPhyDevice>(new WlanPhyDevice(context, phy_config));
  device->Init(std::move(user_channel), std::move(phy_controller));
  return device;
}

WlanPhyDevice::WlanPhyDevice(WlantapDriverContext context,
                             const fuchsia_wlan_tap::WlantapPhyConfig& phy_config)
    : driver_context_(context), phy_config_(phy_config) {
  WLAN_TRACE_DURATION();
}

void WlanPhyDevice::Init(zx::channel user_channel,
                         fidl::ClientEnd<fuchsia_driver_framework::NodeController> phy_controller) {
  WLAN_TRACE_DURATION();
  wlantap_phy_ = std::make_unique<WlantapPhy>(
      std::move(user_channel), phy_config_,
      [self = shared_from_this(),
       name = name_](WlantapPhy::ShutdownCompleter::Async wlantap_phy_shutdown_completer) mutable
          -> zx::result<> {
        // Return an error if |self| has already been reset(). This function
        // should only be called once, and |self| is reset() upon completion
        // to drop its reference.
        if (self == nullptr) {
          fdf::error("{}: shutdown callback called more than once", name);
          return zx::error(ZX_ERR_INTERNAL);
        }

        self->wlantap_phy_shutdown_completer_ = std::move(wlantap_phy_shutdown_completer);

        // Unbind the WlanPhyImpl child node. This effectively blocks iface
        // management from the outside.
        auto phy_removal_status = self->phy_controller_->Remove();
        zx::result<> result = zx::ok();
        if (phy_removal_status.is_error()) {
          fdf::error("{}: Could not remove phy: {}", name,
                     phy_removal_status.error_value().status_string());
          self->ShutdownComplete();

          result = zx::error(phy_removal_status.error_value().status());
        }
        self.reset();
        return result;
      });

  // The PhyControllerEventHandler class detects when NodeController server associated
  // with the phy is no longer available. This normally occurs during shutdown.
  class PhyControllerEventHandler
      : public fidl::AsyncEventHandler<fuchsia_driver_framework::NodeController> {
   public:
    explicit PhyControllerEventHandler(std::shared_ptr<WlanPhyDevice> device)
        : device_(std::move(device)) {}
    void on_fidl_error(::fidl::UnbindInfo error) override {
      WLAN_TRACE_DURATION();
      auto cleanup = fit::defer([this] { delete this; });

      auto device = std::move(device_);
      fdf::info("{}: phy node unbound: {}", device->name_, error.FormatDescription());
      device->phy_controller_ = {};

      std::optional<WlanPhyDevice::IfaceSlot> next_slot;
      std::visit(
          overloaded{[device, &next_slot](WlanPhyDevice::SlotActive& slot) {
                       auto controller = std::move(slot.controller);
                       auto mac = std::move(slot.mac);
                       next_slot = WlanPhyDevice::SlotDestroying{
                           .mac = std::move(mac), .controller = std::move(controller)};
                     },
                     [device](WlanPhyDevice::SlotDestroying& slot) {
                       auto status = slot.controller->Remove();
                       if (status.is_error()) {
                         fdf::error("{}: Could not remove iface: {}", device->name_,
                                    status.error_value().status_string());
                         device->ShutdownComplete();
                       }
                     },
                     [device](WlanPhyDevice::SlotEmpty& slot) { device->ShutdownComplete(); },
                     [device](WlanPhyDevice::SlotCreating& slot) { device->ShutdownComplete(); }},
          device->iface_slot_);

      if (next_slot.has_value()) {
        device->iface_slot_ = std::move(*next_slot);
        auto& destroying = std::get<WlanPhyDevice::SlotDestroying>(device->iface_slot_);
        auto status = destroying.controller->Remove();
        if (status.is_error()) {
          fdf::error("{}: Could not remove iface: {}", device->name_,
                     status.error_value().status_string());
          device->ShutdownComplete();
        }
      }
    }
    void handle_unknown_event(
        fidl::UnknownEventMetadata<fuchsia_driver_framework::NodeController> metadata) override {}

   private:
    std::shared_ptr<WlanPhyDevice> device_;
  };
  phy_controller_.Bind(std::move(phy_controller), fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                       new PhyControllerEventHandler(shared_from_this()));
}

void WlanPhyDevice::Init(InitRequest& request, InitCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  if (!request.notify_client().has_value()) {
    fdf::error("Failed to initialize WlanPhy server. notify_client client end not provided.");
    completer.Reply(fit::error(ZX_ERR_INVALID_ARGS));
  }
  notify_client_ = std::move(request.notify_client().value());
  completer.Reply(fit::ok(fuchsia_wlan_phy::WlanPhyInitResponse{}));
}

void WlanPhyDevice::GetSupportedMacRoles(GetSupportedMacRolesCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();  // wlantap-phy only supports a single mac role determined by the config
  std::vector<fuchsia_wlan_common::WlanMacRole> reply_vec = {phy_config_.mac_role()};

  fdf::info("{}: received a 'GetSupportedMacRoles' DDK request. Responding with roles = {{{}}}",
            name_, static_cast<uint32_t>(phy_config_.mac_role()));

  auto response =
      fuchsia_wlan_phy::WlanPhyGetSupportedMacRolesResponse{{.supported_mac_roles = reply_vec}};
  completer.Reply(fit::ok(response));
}

void WlanPhyDevice::CreateIface(CreateIfaceRequest& request,
                                CreateIfaceCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  fdf::info("{}: received a 'CreateIface' request", name_);

  std::string role_str = RoleToString(request.role().value());
  fdf::info("{}: received a 'CreateIface' for role: {}", name_, role_str);
  if (phy_config_.mac_role() != request.role().value()) {
    fdf::error("{}: CreateIface({}): role not supported", name_, role_str);
    completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
    return;
  }

  if (!std::holds_alternative<SlotEmpty>(iface_slot_)) {
    fdf::error(
        "{}: CreateIface({}): Failed to create iface. wlantap only supports at most one iface.",
        name_, role_str);
    completer.Reply(fit::error(ZX_ERR_ALREADY_EXISTS));
    return;
  }

  if (!request.mlme_channel().has_value() || !request.mlme_channel().value().is_valid()) {
    fdf::error("{}: CreateIface({}): MLME channel in request is missing or invalid", name_,
               role_str);
    completer.Reply(fit::error(ZX_ERR_IO_INVALID));
    return;
  }

  iface_slot_ = SlotCreating{};

  zx_status_t status =
      CreateWlanSoftmac(request.role().value(), std::move(request.mlme_channel().value()));
  if (status != ZX_OK) {
    fdf::error("{}: CreateIface({}): Could not create softmac: {}", name_, role_str,
               zx_status_get_string(status));
    iface_slot_ = SlotEmpty{};
    completer.Reply(fit::error(status));
    return;
  }

  fidl::Arena fidl_arena;
  auto resp = fuchsia_wlan_phy::WlanPhyCreateIfaceResponse{{.iface_id = 0}};
  completer.Reply(fit::ok(resp));
}

// Calls the stored ShutdownCompleter received through WlantapPhy.Shutdown().
void WlanPhyDevice::ShutdownComplete() {
  WLAN_TRACE_DURATION();
  if (this->wlantap_phy_shutdown_completer_.has_value()) {
    this->wlantap_phy_shutdown_completer_->Reply();
  }
}

void WlanPhyDevice::DestroyIface(DestroyIfaceRequest& request,
                                 DestroyIfaceCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  fdf::info("{}: received a 'DestroyIface' DDK request", name_);

  if (!std::holds_alternative<SlotActive>(iface_slot_)) {
    fdf::error("{}: Iface doesn't exist or is not active", name_);
    completer.Reply(fit::error(ZX_ERR_NOT_FOUND));
    return;
  }

  auto active = std::move(std::get<SlotActive>(iface_slot_));
  auto result = active.controller->Remove();
  if (result.is_error()) {
    fdf::error("{}: Failed to destroy iface: {}", name_, result.error_value().FormatDescription());
    completer.Reply(fit::error(result.error_value().status()));
    return;
  }

  iface_slot_ =
      SlotDestroying{.mac = std::move(active.mac), .controller = std::move(active.controller)};

  completer.Reply(fit::ok());
}

void WlanPhyDevice::SetCountry(SetCountryRequest& request, SetCountryCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  fdf::info("{}: SetCountry() to [{}] received", name_, CountryCodeToStr(request.country()));

  fuchsia_wlan_tap::SetCountryArgs args{{.alpha2 = request.country()}};
  zx_status_t status = wlantap_phy_->SetCountry(args);
  if (status != ZX_OK) {
    fdf::error("{}: SetCountry() failed: {}", name_, zx_status_get_string(status));
    completer.Reply(fit::error(status));
    return;
  }
  completer.Reply(fit::ok());
}

void WlanPhyDevice::ClearCountry(ClearCountryCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  fdf::warn("{}: ClearCountry() not supported", name_);
  completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
}

void WlanPhyDevice::GetCountry(GetCountryCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  fdf::warn("{}: GetCountry() not supported", name_);
  completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
}

void WlanPhyDevice::SetPowerSaveMode(SetPowerSaveModeRequest& request,
                                     SetPowerSaveModeCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  fdf::warn("{}: SetPowerSaveMode() not supported", name_);
  completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
}

void WlanPhyDevice::GetPowerSaveMode(GetPowerSaveModeCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  fdf::warn("{}: GetPowerSaveMode() not supported", name_);
  completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
}

void WlanPhyDevice::PowerDown(PowerDownCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  fdf::warn("{}: PowerDown() not supported", name_);
  completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
}
void WlanPhyDevice::PowerUp(PowerUpCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  fdf::warn("{}: PowerUp() not supported", name_);
  completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
}

void WlanPhyDevice::Reset(ResetCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  fdf::warn("{}: Reset() not supported", name_);
  completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
}
void WlanPhyDevice::GetPowerState(GetPowerStateCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  fdf::warn("{}: GetPowerState() not supported", name_);
  completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
}

void WlanPhyDevice::SetBtCoexistenceMode(SetBtCoexistenceModeRequest& request,
                                         SetBtCoexistenceModeCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  fdf::warn("{}: SetBtCoexistenceMode() not supported", name_);
  completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
}

void WlanPhyDevice::SetTxPowerScenario(SetTxPowerScenarioRequest& request,
                                       SetTxPowerScenarioCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  fdf::warn("{}: SetTxPowerScenario() not supported", name_);
  completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
}
void WlanPhyDevice::ResetTxPowerScenario(ResetTxPowerScenarioCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  fdf::warn("{}: ResetTxPowerScenario() not supported", name_);
  completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
}
void WlanPhyDevice::GetTxPowerScenario(GetTxPowerScenarioCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  fdf::warn("{}: GetTxPowerScenario() not supported", name_);
  completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
}

zx_status_t WlanPhyDevice::CreateWlanSoftmac(fuchsia_wlan_common::WlanMacRole role,
                                             zx::channel mlme_channel) {
  WLAN_TRACE_DURATION();
  static size_t n = 0;
  char name[ZX_MAX_NAME_LEN + 1];
  snprintf(name, sizeof(name), "wlansoftmac-%lu", n++);

  auto endpoints = fidl::CreateEndpoints<fuchsia_driver_framework::NodeController>();
  if (endpoints.is_error()) {
    fdf::error("{}: Failed to create endpoints: {}", name_, endpoints);
    return endpoints.status_value();
  }

  auto serve_result = ServeWlanSoftmac(name, role, std::move(mlme_channel));
  if (serve_result.is_error()) {
    fdf::error("{}: ServeWlanSoftmac failed: {}", name_, serve_result.status_string());
    return serve_result.error_value();
  }
  std::unique_ptr<WlantapMac> mac = std::move(serve_result.value());

  zx_status_t status = AddWlanSoftmacChild(name, std::move(endpoints->server));
  if (status != ZX_OK) {
    fdf::error("{}: AddWlanSoftmacChild failed: {}", name_, zx_status_get_string(status));
    zx::result remove_res =
        driver_context_.outgoing()->RemoveService<fuchsia_wlan_softmac::Service>(name);
    if (remove_res.is_error()) {
      fdf::error("{}: Failed to remove service instance during rollback: {}", name_, remove_res);
    }
    return status;
  }

  // The IfaceControllerEventHandler class detects when NodeController server associated
  // with an iface is no longer available. This normally occurs during shutdown.
  class IfaceControllerEventHandler
      : public fidl::AsyncEventHandler<fuchsia_driver_framework::NodeController> {
   public:
    explicit IfaceControllerEventHandler(std::shared_ptr<WlanPhyDevice> device)
        : device_(std::move(device)) {}
    void on_fidl_error(::fidl::UnbindInfo error) override {
      auto cleanup = fit::defer([this] { delete this; });
      auto device = std::move(device_);
      fdf::info("{}: Iface node unbound: {}", device->name_, error.FormatDescription());

      std::visit(overloaded{[device](WlanPhyDevice::SlotActive& slot) {
                              fdf::warn("{}: Iface node unexpectedly unbound!", device->name_);
                            },
                            [device](WlanPhyDevice::SlotDestroying& slot) {
                              if (device->wlantap_phy_shutdown_completer_.has_value()) {
                                device->ShutdownComplete();
                              }
                            },
                            [](WlanPhyDevice::SlotEmpty& slot) {},
                            [](WlanPhyDevice::SlotCreating& slot) {}},
                 device->iface_slot_);

      device->iface_slot_ = WlanPhyDevice::SlotEmpty{};
    }
    void handle_unknown_event(
        fidl::UnknownEventMetadata<fuchsia_driver_framework::NodeController> metadata) override {}

   private:
    std::shared_ptr<WlanPhyDevice> device_;
  };

  fidl::Client<fuchsia_driver_framework::NodeController> controller;
  controller.Bind(std::move(endpoints->client), fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                  new IfaceControllerEventHandler(shared_from_this()));

  iface_slot_ = SlotActive{.mac = std::move(mac), .controller = std::move(controller)};
  return ZX_OK;
}

zx_status_t WlanPhyDevice::AddWlanSoftmacChild(
    std::string_view name, fidl::ServerEnd<fuchsia_driver_framework::NodeController> server) {
  WLAN_TRACE_DURATION();
  auto offers = std::vector{fdf::MakeOffer2<fuchsia_wlan_softmac::Service>(std::string(name))};
  fdf::info("{}: Creating Child node", name_);
  fuchsia_driver_framework::NodeAddArgs args;
  args.name(std::string(name)).offers2(std::move(offers));

  auto res = driver_context_.node_client()->AddChild(
      {{.args = std::move(args), .controller = std::move(server)}});
  if (res.is_error()) {
    fdf::error("{}: Failed to add child: {}", name_, res.error_value().FormatDescription());
    return ZX_ERR_INTERNAL;
  }

  return ZX_OK;
}

zx::result<std::unique_ptr<WlantapMac>> WlanPhyDevice::ServeWlanSoftmac(
    std::string_view name, fuchsia_wlan_common::WlanMacRole role, zx::channel mlme_channel) {
  WLAN_TRACE_DURATION();
  auto out_mac =
      std::make_unique<WlantapMac>(wlantap_phy_.get(), role, phy_config_, std::move(mlme_channel));

  fdf::info("{}: Adding softmac outgoing service", name_);
  fuchsia_wlan_softmac::Service::InstanceHandler handler(
      {.wlan_softmac = out_mac->ProtocolHandler()});

  zx::result result = driver_context_.outgoing()->AddService<fuchsia_wlan_softmac::Service>(
      std::move(handler), name);

  if (result.is_error()) {
    fdf::error("{}: Failed To add WlanSoftmac service: {}", name_, result);
    return result.take_error();
  }

  return zx::ok(std::move(out_mac));
}

}  // namespace wlan
