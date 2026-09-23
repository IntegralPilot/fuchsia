// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "vim3_clk.h"

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/logging/cpp/structured_logger.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/mmio/cpp/mmio-view.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include <limits>
#include <memory>

#include <bind/fuchsia/test/cpp/bind.h>
#include <soc/aml-meson/aml-clk-common.h>
#include <soc/aml-meson/g12b-clk.h>

namespace vim3_clock {

zx::result<> Vim3Clock::Start(fdf::DriverContext context) {
  fdf::info("Vim3Clock::Start()");

  zx::result pdev_client_end =
      context.incoming().Connect<fuchsia_hardware_platform_device::Service::Device>();
  if (pdev_client_end.is_error()) {
    fdf::error("Failed to connect to platform device: {}", pdev_client_end);
    return pdev_client_end.take_error();
  }

  fdf::PDev pdev{std::move(pdev_client_end.value())};

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  if (zx::result result =
          clock_ids_metadata_server_.ForwardAndServe(*outgoing(), dispatcher(), pdev);
      result.is_error()) {
    fdf::error("Failed to forward clock IDs: {}", result);
    return result.take_error();
  }
#endif

  if (zx::result result =
          clock_init_metadata_server_.ForwardAndServe(*outgoing(), dispatcher(), pdev);
      result.is_error()) {
    fdf::error("Failed to forward clock init metadata: {}", result);
    return result.take_error();
  }

  zx::result hiu_mmio = pdev.MapMmio(kHiuMmioIndex);
  if (hiu_mmio.is_error()) {
    fdf::error("Failed to map HIU mmio, st = {}", zx_status_get_string(hiu_mmio.error_value()));
    return hiu_mmio.take_error();
  }
  hiu_mmio_ = std::move(hiu_mmio.value());

  zx::result dos_mmio = pdev.MapMmio(kDosMmioIndex);
  if (dos_mmio.is_error()) {
    fdf::error("Failed to map DOS mmio, st = {}", zx_status_get_string(dos_mmio.error_value()));
    return dos_mmio.take_error();
  }
  dos_mmio_ = std::move(dos_mmio.value());

  auto child_name = "clocks";

  auto add_service_result = outgoing()->AddService<fuchsia_hardware_clockimpl::Service>(
      fuchsia_hardware_clockimpl::Service::InstanceHandler({
          .device = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->get(),
                                            fidl::kIgnoreBindingClosure),
      }));
  if (add_service_result.is_error()) {
    fdf::error("Failed to add Device service {}", add_service_result);
    return add_service_result.take_error();
  }

  // Add a child node.
  std::vector<fuchsia_driver_framework::Offer> offers = {
      fdf::MakeOffer2<fuchsia_hardware_clockimpl::Service>(),
  };
  std::optional clock_ids_offer = clock_ids_metadata_server_.CreateOffer();
  if (clock_ids_offer.has_value()) {
    offers.push_back(std::move(clock_ids_offer.value()));
  }
  std::optional clock_init_offer = clock_init_metadata_server_.CreateOffer();
  if (clock_init_offer.has_value()) {
    offers.push_back(std::move(clock_init_offer.value()));
  }

  std::vector<fuchsia_driver_framework::NodeProperty2> properties = {};
  auto add_child_result = AddChild(child_name, properties, offers);
  if (add_child_result.is_error()) {
    fdf::error("Failed to add child: {}", add_child_result);
    return add_child_result.take_error();
  }

  child_controller_.Bind(std::move(add_child_result.value()));

  InitGates();

  InitHiu();

  InitCpuClks();

  return zx::ok();
}

void Vim3Clock::Enable(fuchsia_hardware_clockimpl::wire::ClockImplEnableRequest* request,
                       fdf::Arena& arena, EnableCompleter::Sync& completer) {
  fdf::trace("Enable - clkid = {}", request->id);

  const uint32_t id = request->id;

  const aml_clk_common::aml_clk_type type = aml_clk_common::AmlClkType(id);
  const uint16_t clkid = aml_clk_common::AmlClkIndex(id);

  zx_status_t result;
  switch (type) {
    case aml_clk_common::aml_clk_type::kMesonGate:
      result = ClkToggle(clkid, true);
      break;
    case aml_clk_common::aml_clk_type::kMesonPll:
      result = ClkTogglePll(clkid, true);
      break;
    default:
      result = ZX_ERR_NOT_SUPPORTED;
  }

  completer.buffer(arena).Reply(zx::make_result(result));
}

void Vim3Clock::Disable(fuchsia_hardware_clockimpl::wire::ClockImplDisableRequest* request,
                        fdf::Arena& arena, DisableCompleter::Sync& completer) {
  fdf::trace("Disable - clkid = {}", request->id);

  const uint32_t id = request->id;

  const aml_clk_common::aml_clk_type type = aml_clk_common::AmlClkType(id);
  const uint16_t clkid = aml_clk_common::AmlClkIndex(id);

  zx_status_t result;
  switch (type) {
    case aml_clk_common::aml_clk_type::kMesonGate:
      result = ClkToggle(clkid, false);
      break;
    case aml_clk_common::aml_clk_type::kMesonPll:
      result = ClkTogglePll(clkid, false);
      break;
    default:
      result = ZX_ERR_NOT_SUPPORTED;
  }

  completer.buffer(arena).Reply(zx::make_result(result));
}

void Vim3Clock::IsEnabled(fuchsia_hardware_clockimpl::wire::ClockImplIsEnabledRequest* request,
                          fdf::Arena& arena, IsEnabledCompleter::Sync& completer) {
  fdf::trace("IsEnabled - clkid = {}", request->id);

  completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void Vim3Clock::SetRate(fuchsia_hardware_clockimpl::wire::ClockImplSetRateRequest* request,
                        fdf::Arena& arena, SetRateCompleter::Sync& completer) {
  fdf::trace("SetRate clkid = {}, hz = {}", request->id, request->hz);

  MesonRateClock* target;
  zx_status_t result = GetMesonRateClock(request->id, &target);
  if (result != ZX_OK) {
    completer.buffer(arena).ReplyError(result);
    fdf::error("Failed to get Rate clock, clkid = {}", request->id);
    return;
  }

  if (request->hz > std::numeric_limits<uint32_t>::max()) {
    completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  result = target->SetRate(static_cast<uint32_t>(request->hz));

  completer.buffer(arena).Reply(zx::make_result(result));
}

void Vim3Clock::QuerySupportedRate(
    fuchsia_hardware_clockimpl::wire::ClockImplQuerySupportedRateRequest* request,
    fdf::Arena& arena, QuerySupportedRateCompleter::Sync& completer) {
  fdf::trace("QuerySupportedRate clkid = {}, hz = {}", request->id, request->hz);

  MesonRateClock* target;
  zx_status_t st = GetMesonRateClock(request->id, &target);
  if (st != ZX_OK) {
    completer.buffer(arena).ReplyError(st);
    fdf::error("Failed to get Rate clock, clkid = {}", request->id);
    return;
  }

  uint64_t supported_rate;
  st = target->QuerySupportedRate(request->hz, &supported_rate);

  if (st != ZX_OK) {
    completer.buffer(arena).ReplyError(st);
  } else {
    completer.buffer(arena).ReplySuccess(supported_rate);
  }
}

void Vim3Clock::GetRate(fuchsia_hardware_clockimpl::wire::ClockImplGetRateRequest* request,
                        fdf::Arena& arena, GetRateCompleter::Sync& completer) {
  fdf::trace("GetRate clkid = {}", request->id);

  MesonRateClock* target;
  zx_status_t st = GetMesonRateClock(request->id, &target);
  if (st != ZX_OK) {
    completer.buffer(arena).ReplyError(st);
    fdf::error("Failed to get Rate clock, clkid = {}", request->id);
    return;
  }

  uint64_t rate;
  st = target->GetRate(&rate);

  if (st != ZX_OK) {
    completer.buffer(arena).ReplyError(st);
  } else {
    completer.buffer(arena).ReplySuccess(rate);
  }
}

void Vim3Clock::SetInput(fuchsia_hardware_clockimpl::wire::ClockImplSetInputRequest* request,
                         fdf::Arena& arena, SetInputCompleter::Sync& completer) {
  fdf::trace("SetInput clkid = {}", request->id);

  completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void Vim3Clock::GetNumInputs(
    fuchsia_hardware_clockimpl::wire::ClockImplGetNumInputsRequest* request, fdf::Arena& arena,
    GetNumInputsCompleter::Sync& completer) {
  fdf::trace("GetNumInputs clkid = {}", request->id);

  completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void Vim3Clock::GetInput(fuchsia_hardware_clockimpl::wire::ClockImplGetInputRequest* request,
                         fdf::Arena& arena, GetInputCompleter::Sync& completer) {
  fdf::trace("GetInput clkid = {}", request->id);

  completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void Vim3Clock::GetClockProperties(fdf::Arena& arena,
                                   GetClockPropertiesCompleter::Sync& completer) {
  fdf::trace("GetClockProperties");
  completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
}

zx_status_t Vim3Clock::ClkToggle(uint32_t clk, bool enable) {
  if (clk >= gates_.size()) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  if (enable) {
    gates_.at(clk).Enable();
  } else {
    gates_.at(clk).Disable();
  }

  return ZX_OK;
}

zx_status_t Vim3Clock::ClkTogglePll(uint32_t clk, bool enable) {
  if (clk >= plls_.size()) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  return plls_.at(clk).Toggle(enable);
}

zx_status_t Vim3Clock::GetMesonRateClock(uint32_t clk, MesonRateClock** out) {
  aml_clk_common::aml_clk_type type = aml_clk_common::AmlClkType(clk);
  const uint16_t clkid = aml_clk_common::AmlClkIndex(clk);

  switch (type) {
    case aml_clk_common::aml_clk_type::kMesonPll:
      if (clkid >= plls_.size()) {
        fdf::error("HIU PLL out of range, clkid = {}.", clkid);
        return ZX_ERR_INVALID_ARGS;
      }

      *out = &plls_[clkid];
      return ZX_OK;
    case aml_clk_common::aml_clk_type::kMesonCpuClk:
      if (clkid >= cpu_clks_.size()) {
        fdf::error("cpu clk out of range, clkid = {}.", clkid);
        return ZX_ERR_INVALID_ARGS;
      }

      *out = &cpu_clks_[clkid];
      return ZX_OK;
    default:
      fdf::error("Unsupported clock type, type = 0x{:x}\n", static_cast<unsigned short>(type));
      return ZX_ERR_NOT_SUPPORTED;
  }

  __UNREACHABLE;
}

void Vim3Clock::InitGates() {
  ZX_ASSERT_MSG(gates_.empty(), "Gates has already been initialized");

  for (const meson_gate_descriptor_t& desc : kGateDescriptors) {
    switch (desc.bank) {
      case RegisterBank::Hiu:
        gates_.emplace_back(desc.id, desc.offset, desc.mask, hiu_mmio_->View(0));
        break;
      case vim3_clock::RegisterBank::Dos:
        gates_.emplace_back(desc.id, desc.offset, desc.mask, dos_mmio_->View(0));
        break;
    }
  }

  fdf::info("vim3 clock gates initialized with {} entries", gates_.size());
}

void Vim3Clock::InitHiu() {
  plls_.reserve(HIU_PLL_COUNT);
  s905d2_hiu_init_etc(&*hiudev_, hiu_mmio_->View(0));
  for (unsigned int pllnum = 0; pllnum < HIU_PLL_COUNT; pllnum++) {
    const hhi_plls_t pll = static_cast<hhi_plls_t>(pllnum);
    auto& newpll = plls_.emplace_back(pll, &*hiudev_);
    newpll.Init();
  }

  fdf::info("vim3 hiu plls initialized with {} entries", plls_.size());
}

void Vim3Clock::InitCpuClks() {
  constexpr size_t kNumCpuClks = std::size(kG12bCpuClks);
  cpu_clks_.reserve(kNumCpuClks);

  for (size_t i = 0; i < kNumCpuClks; i++) {
    cpu_clks_.emplace_back(&*hiu_mmio_, kG12bCpuClks[i].reg, &plls_[kG12bCpuClks[i].pll],
                           kG12bCpuClks[i].initial_hz);
  }

  fdf::info("vim3 cpu plls initialized with {} entries", cpu_clks_.size());
}

}  // namespace vim3_clock

FUCHSIA_DRIVER_EXPORT2(vim3_clock::Vim3Clock);
