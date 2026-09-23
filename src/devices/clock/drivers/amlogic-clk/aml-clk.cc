// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-clk.h"

#include <fidl/fuchsia.hardware.clock/cpp/wire.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>
#include <string.h>

#include <bind/fuchsia/clock/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <fbl/auto_lock.h>
#include <soc/aml-meson/aml-clk-common.h>

#include "aml-a1-blocks.h"
#include "aml-a5-blocks.h"
#include "aml-axg-blocks.h"
#include "aml-g12a-blocks.h"
#include "aml-g12b-blocks.h"
#include "aml-gxl-blocks.h"
#include "aml-sm1-blocks.h"

namespace amlogic_clock {

#define MSR_WAIT_BUSY_RETRIES 5
#define MSR_WAIT_BUSY_TIMEOUT_US 10000

zx_status_t AmlClock::PopulateRegisterBlocks(uint32_t device_id, fdf::PDev& pdev) {
  // Populate the correct register blocks.
  switch (device_id) {
    case PDEV_DID_AMLOGIC_AXG_CLK: {
      // Gauss
      gates_ = axg_clk_gates;
      gate_count_ = std::size(axg_clk_gates);
      meson_gate_enable_count_.resize(gate_count_);
      break;
    }
    case PDEV_DID_AMLOGIC_GXL_CLK: {
      gates_ = gxl_clk_gates;
      gate_count_ = std::size(gxl_clk_gates);
      meson_gate_enable_count_.resize(gate_count_);
      break;
    }
    case PDEV_DID_AMLOGIC_G12A_CLK: {
      // Astro
      clk_msr_offsets_ = g12a_clk_msr;

      clk_table_ = static_cast<const char* const*>(g12a_clk_table);
      clk_table_count_ = std::size(g12a_clk_table);

      gates_ = g12a_clk_gates;
      gate_count_ = std::size(g12a_clk_gates);
      meson_gate_enable_count_.resize(gate_count_);

      InitHiu();

      constexpr size_t cpu_clk_count = std::size(g12a_cpu_clks);
      cpu_clks_.reserve(cpu_clk_count);
      for (const auto& g12a_cpu_clk : g12a_cpu_clks) {
        cpu_clks_.emplace_back(&hiu_mmio_.value(), g12a_cpu_clk.reg, &pllclk_[g12a_cpu_clk.pll],
                               g12a_cpu_clk.initial_hz);
      }

      break;
    }
    case PDEV_DID_AMLOGIC_G12B_CLK: {
      // Sherlock
      clk_msr_offsets_ = g12b_clk_msr;

      clk_table_ = static_cast<const char* const*>(g12b_clk_table);
      clk_table_count_ = std::size(g12b_clk_table);

      gates_ = g12b_clk_gates;
      gate_count_ = std::size(g12b_clk_gates);
      meson_gate_enable_count_.resize(gate_count_);

      InitHiu();

      constexpr size_t cpu_clk_count = std::size(g12b_cpu_clks);
      cpu_clks_.reserve(cpu_clk_count);
      for (const auto& g12b_cpu_clk : g12b_cpu_clks) {
        cpu_clks_.emplace_back(&hiu_mmio_.value(), g12b_cpu_clk.reg, &pllclk_[g12b_cpu_clk.pll],
                               g12b_cpu_clk.initial_hz);
      }

      break;
    }
    case PDEV_DID_AMLOGIC_SM1_CLK: {
      // Nelson
      clk_msr_offsets_ = sm1_clk_msr;

      clk_table_ = static_cast<const char* const*>(sm1_clk_table);
      clk_table_count_ = std::size(sm1_clk_table);

      gates_ = sm1_clk_gates;
      gate_count_ = std::size(sm1_clk_gates);
      meson_gate_enable_count_.resize(gate_count_);

      muxes_ = sm1_muxes;
      mux_count_ = std::size(sm1_muxes);

      InitHiu();

      break;
    }
    case PDEV_DID_AMLOGIC_A5_CLK: {
      // AV400
      uint32_t chip_id = PDEV_PID_AMLOGIC_A5;

      zx::result smc_resource = pdev.GetSmc(0);
      if (smc_resource.is_error()) {
        fdf::error("Failed to get SMC: {}", smc_resource);
        return smc_resource.status_value();
      }

      clk_msr_offsets_ = a5_clk_msr;

      clk_table_ = static_cast<const char* const*>(a5_clk_table);
      clk_table_count_ = std::size(a5_clk_table);

      gates_ = a5_clk_gates;
      gate_count_ = std::size(a5_clk_gates);
      meson_gate_enable_count_.resize(gate_count_);

      muxes_ = a5_muxes;
      mux_count_ = std::size(a5_muxes);

      pll_count_ = a5::PLL_COUNT;
      InitHiuA5();

      constexpr size_t cpu_clk_count = std::size(a5_cpu_clks);
      cpu_clks_.reserve(cpu_clk_count);
      // For A5, there is only 1 CPU clock
      cpu_clks_.emplace_back(&hiu_mmio_.value(), a5_cpu_clks[0].reg, &pllclk_[a5_cpu_clks[0].pll],
                             a5_cpu_clks[0].initial_hz, chip_id, std::move(smc_resource.value()));

      break;
    }
    case PDEV_DID_AMLOGIC_A1_CLK: {
      // clover
      uint32_t chip_id = PDEV_PID_AMLOGIC_A1;
      clk_msr_offsets_ = a1_clk_msr;

      clk_table_ = static_cast<const char* const*>(a1_clk_table);
      clk_table_count_ = std::size(a1_clk_table);

      gates_ = a1_clk_gates;
      gate_count_ = std::size(a1_clk_gates);
      meson_gate_enable_count_.resize(gate_count_);

      muxes_ = a1_muxes;
      mux_count_ = std::size(a1_muxes);

      pll_count_ = a1::PLL_COUNT;
      InitHiuA1();

      constexpr size_t cpu_clk_count = std::size(a1_cpu_clks);
      cpu_clks_.reserve(cpu_clk_count);
      // For A1, there is only 1 CPU clock
      cpu_clks_.emplace_back(&cpuctrl_mmio_.value(), a1_cpu_clks[0].reg,
                             &pllclk_[a1_cpu_clks[0].pll], a1_cpu_clks[0].initial_hz, chip_id);

      break;
    }
    default:
      fdf::error("Unsupported SOC DID: {}", device_id);
      return ZX_ERR_INVALID_ARGS;
  }
  return ZX_OK;
}

zx::result<> AmlClock::Start(fdf::DriverContext context) {
  // Get the platform device protocol and try to map all the MMIO regions.
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

  // All AML clocks have HIU and dosbus regs but only some support MSR regs.
  // Figure out which of the varieties we're dealing with.
  {
    zx::result hiu_mmio = pdev.MapMmio(kHiuMmio);
    if (hiu_mmio.is_error()) {
      fdf::error("Failed to map HIU mmio: {}", hiu_mmio);
      return hiu_mmio.take_error();
    }
    hiu_mmio_.emplace(std::move(hiu_mmio.value()));
  }

  {
    zx::result dosbus_mmio = pdev.MapMmio(kDosbusMmio);
    if (dosbus_mmio.is_error()) {
      fdf::error("Failed to map DOS mmio: {}", dosbus_mmio);
      return dosbus_mmio.take_error();
    }
    dosbus_mmio_.emplace(std::move(dosbus_mmio.value()));
  }

  // Use the Pdev Device Info to determine if we've been provided with two
  // MMIO regions.
  zx::result device_info = pdev.GetDeviceInfo();
  if (device_info.is_error()) {
    fdf::error("Failed to get device info: {}", device_info);
    return device_info.take_error();
  }

  if (device_info->vid == PDEV_VID_GENERIC && device_info->pid == PDEV_PID_GENERIC &&
      device_info->did == PDEV_DID_DEVICETREE_NODE) {
    // TODO(https://fxbug.dev/318736574) : Remove and rely only on GetDeviceInfo.
    zx::result board_info = pdev.GetBoardInfo();
    if (board_info.is_error()) {
      fdf::error("Failed to get board info: {}", board_info);
      return board_info.take_error();
    }

    if (board_info->vid == PDEV_VID_KHADAS) {
      switch (board_info->pid) {
        case PDEV_PID_VIM3:
          device_info->pid = PDEV_PID_AMLOGIC_A311D;
          device_info->did = PDEV_DID_AMLOGIC_G12B_CLK;
          break;
        default:
          fdf::error("Unsupported PID 0x{:x} for VID 0x{:x}", board_info->pid, board_info->vid);
          return zx::error(ZX_ERR_INVALID_ARGS);
      }
    } else if (board_info->vid == PDEV_VID_GOOGLE) {
      switch (board_info->pid) {
        case PDEV_PID_ASTRO:
          device_info->pid = PDEV_PID_AMLOGIC_S905D2;
          device_info->did = PDEV_DID_AMLOGIC_G12A_CLK;
          break;
        case PDEV_PID_SHERLOCK:
          device_info->pid = PDEV_PID_AMLOGIC_T931;
          device_info->did = PDEV_DID_AMLOGIC_G12B_CLK;
          break;
        default:
          fdf::error("Unsupported PID 0x{:x} for VID 0x{:x}", board_info->pid, board_info->vid);
          return zx::error(ZX_ERR_INVALID_ARGS);
      }
    } else {
      fdf::error("Unsupported VID 0x{:x}", board_info->vid);
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
  }

  if (device_info->mmio_count > kMsrMmio) {
    zx::result msr_mmio = pdev.MapMmio(kMsrMmio);
    if (msr_mmio.is_error()) {
      fdf::error("Failed to map MSR mmio: {}", msr_mmio);
      return msr_mmio.take_error();
    }
    msr_mmio_ = std::move(msr_mmio.value());
  }

  // For A1, this register is within cpuctrl mmio
  if (device_info->pid == PDEV_PID_AMLOGIC_A1 && device_info->mmio_count > kCpuCtrlMmio) {
    zx::result cpuctrl_mmio = pdev.MapMmio(kCpuCtrlMmio);
    if (cpuctrl_mmio.is_error()) {
      fdf::error("Failed to map cpuctrl mmio: {}", cpuctrl_mmio);
      return cpuctrl_mmio.take_error();
    }
    cpuctrl_mmio_ = std::move(cpuctrl_mmio.value());
  }

  zx_status_t status = PopulateRegisterBlocks(device_info->did, pdev);
  if (status != ZX_OK) {
    fdf::error("Failed to populate register blocks: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  auto add_service_result = outgoing()->AddService<fuchsia_hardware_clockimpl::Service>(
      fuchsia_hardware_clockimpl::Service::InstanceHandler({
          .device = clock_impl_binding_group_.CreateHandler(
              this, fdf::Dispatcher::GetCurrent()->get(), fidl::kIgnoreBindingClosure),
      }));
  if (add_service_result.is_error()) {
    fdf::error("Failed to add clock-impl service {}", add_service_result);
    return add_service_result.take_error();
  }

  status = InitChildNode();
  if (status != ZX_OK) {
    fdf::error("Failed to initialize child node: {}", zx_status_get_string(status));
    return zx::error(status);
  }
  return zx::ok();
}

zx_status_t AmlClock::InitChildNode() {
  zx::result connector = devfs_connector_.Bind(dispatcher());
  if (connector.is_error()) {
    fdf::error("Failed to bind devfs connecter to dispatcher: {}", connector);
    return connector.status_value();
  }

  fuchsia_driver_framework::DevfsAddArgs devfs_add_args{
      {.connector = std::move(connector.value())}};

  auto properties = std::vector{
      fdf::MakeProperty2(bind_fuchsia::PROTOCOL, bind_fuchsia_clock::BIND_PROTOCOL_IMPL),
  };

  std::vector<fuchsia_driver_framework::Offer> offers = {
      fdf::MakeOffer2<fuchsia_hardware_clockimpl::Service>()};
  std::optional clock_ids_offer = clock_ids_metadata_server_.CreateOffer();
  if (clock_ids_offer.has_value()) {
    offers.push_back(std::move(clock_ids_offer.value()));
  }
  std::optional clock_init_offer = clock_init_metadata_server_.CreateOffer();
  if (clock_init_offer.has_value()) {
    offers.push_back(std::move(clock_init_offer.value()));
  }

  zx::result result = AddChild(kChildNodeName, devfs_add_args, properties, offers);
  if (result.is_error()) {
    fdf::error("Failed to add device: {}", result);
    return result.status_value();
  }
  child_node_controller_.Bind(std::move(result.value()));

  return ZX_OK;
}

void AmlClock::DevfsConnect(fidl::ServerEnd<fuchsia_hardware_clock_measure::Measurer> request) {
  measurer_binding_group_.AddBinding(dispatcher(), std::move(request), this,
                                     fidl::kIgnoreBindingClosure);
}

zx_status_t AmlClock::ClkTogglePll(uint32_t id, const bool enable) {
  if (id >= pll_count_) {
    fdf::error("Invalid clkid: {}, pll count {}", id, pll_count_);
    return ZX_ERR_INVALID_ARGS;
  }

  return pllclk_[id].Toggle(enable);
}

zx_status_t AmlClock::ClkToggle(uint32_t id, bool enable) {
  if (id >= gate_count_) {
    return ZX_ERR_INVALID_ARGS;
  }

  const meson_clk_gate_t* gate = &(gates_[id]);

  fbl::AutoLock al(&lock_);

  uint32_t enable_count = meson_gate_enable_count_[id];

  // For the sake of catching bugs, disabling a clock that has never
  // been enabled is a bug.
  ZX_ASSERT_MSG((enable == true || enable_count > 0),
                "Cannot disable already disabled clock. clkid = %u", id);

  // Update the refcounts.
  if (enable) {
    meson_gate_enable_count_[id]++;
  } else {
    ZX_ASSERT(enable_count > 0);
    meson_gate_enable_count_[id]--;
  }

  if (enable && meson_gate_enable_count_[id] == 1) {
    // Transition from 0 refs to 1.
    ClkToggleHw(gate, true);
  }

  if (!enable && meson_gate_enable_count_[id] == 0) {
    // Transition from 1 ref to 0.
    ClkToggleHw(gate, false);
  }

  return ZX_OK;
}

void AmlClock::ClkToggleHw(const meson_clk_gate_t* gate, bool enable) {
  uint32_t mask = gate->mask ? gate->mask : (1 << gate->bit);
  fdf::MmioBuffer* mmio;
  switch (gate->register_set) {
    case kMesonRegisterSetHiu:
      mmio = &hiu_mmio_.value();
      break;
    case kMesonRegisterSetDos:
      mmio = &dosbus_mmio_.value();
      break;
    default:
      ZX_PANIC("Unsupported register set: %d", gate->register_set);
  }

  if (enable) {
    mmio->SetBits32(mask, gate->reg);
  } else {
    mmio->ClearBits32(mask, gate->reg);
  }
}

void AmlClock::Enable(EnableRequestView request, fdf::Arena& arena,
                      EnableCompleter::Sync& completer) {
  // Determine which clock type we're trying to control.
  aml_clk_common::aml_clk_type type = aml_clk_common::AmlClkType(request->id);
  const uint16_t clkid = aml_clk_common::AmlClkIndex(request->id);

  zx_status_t status;
  switch (type) {
    case aml_clk_common::aml_clk_type::kMesonGate:
      status = ClkToggle(clkid, true);
      if (status != ZX_OK) {
        completer.buffer(arena).ReplyError(status);
        return;
      }
      break;
    case aml_clk_common::aml_clk_type::kMesonPll:
      status = ClkTogglePll(clkid, true);
      if (status != ZX_OK) {
        completer.buffer(arena).ReplyError(status);
        return;
      }
      break;
    default:
      // Not a supported clock type?
      completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
      return;
  }

  completer.buffer(arena).ReplySuccess();
}

void AmlClock::Disable(DisableRequestView request, fdf::Arena& arena,
                       DisableCompleter::Sync& completer) {
  // Determine which clock type we're trying to control.
  aml_clk_common::aml_clk_type type = aml_clk_common::AmlClkType(request->id);
  const uint16_t clkid = aml_clk_common::AmlClkIndex(request->id);

  zx_status_t status;
  switch (type) {
    case aml_clk_common::aml_clk_type::kMesonGate:
      status = ClkToggle(clkid, false);
      if (status != ZX_OK) {
        completer.buffer(arena).ReplyError(status);
        return;
      }
      break;
    case aml_clk_common::aml_clk_type::kMesonPll:
      status = ClkTogglePll(clkid, false);
      if (status != ZX_OK) {
        completer.buffer(arena).ReplyError(status);
        return;
      }
      break;
    default:
      // Not a supported clock type?
      completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
      return;
  };

  completer.buffer(arena).ReplySuccess();
}

void AmlClock::IsEnabled(IsEnabledRequestView request, fdf::Arena& arena,
                         IsEnabledCompleter::Sync& completer) {
  completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void AmlClock::SetRate(SetRateRequestView request, fdf::Arena& arena,
                       SetRateCompleter::Sync& completer) {
  fdf::trace("{}: clk = {}, hz = {}", __func__, request->id, request->hz);

  if (request->hz >= UINT32_MAX) {
    fdf::error("{}: requested rate exceeds uint32_max, clkid = {}, rate = {}", __func__,
               request->id, request->hz);
    completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  MesonRateClock* target_clock;
  zx_status_t st = GetMesonRateClock(request->id, &target_clock);
  if (st != ZX_OK) {
    completer.buffer(arena).ReplyError(st);
    return;
  }

  st = target_clock->SetRate(static_cast<uint32_t>(request->hz));
  if (st != ZX_OK) {
    completer.buffer(arena).ReplyError(st);
    return;
  }

  completer.buffer(arena).ReplySuccess();
}

void AmlClock::QuerySupportedRate(QuerySupportedRateRequestView request, fdf::Arena& arena,
                                  QuerySupportedRateCompleter::Sync& completer) {
  fdf::trace("{}: clkid = {}, max_rate = {}", __func__, request->id, request->hz);

  MesonRateClock* target_clock;
  zx_status_t st = GetMesonRateClock(request->id, &target_clock);
  if (st != ZX_OK) {
    completer.buffer(arena).ReplyError(st);
    return;
  }

  zx::result supported_rate = target_clock->QuerySupportedRate(request->hz);
  if (supported_rate.is_error()) {
    completer.buffer(arena).ReplyError(supported_rate.status_value());
    return;
  }
  completer.buffer(arena).ReplySuccess(supported_rate.value());
}

void AmlClock::GetRate(GetRateRequestView request, fdf::Arena& arena,
                       GetRateCompleter::Sync& completer) {
  fdf::trace("{}: clkid = {}", __func__, request->id);

  MesonRateClock* target_clock;
  zx_status_t st = GetMesonRateClock(request->id, &target_clock);
  if (st != ZX_OK) {
    completer.buffer(arena).ReplyError(st);
    return;
  }

  zx::result rate = target_clock->GetRate();
  if (rate.is_error()) {
    completer.buffer(arena).ReplyError(rate.status_value());
    return;
  }
  completer.buffer(arena).ReplySuccess(rate.value());
}

zx_status_t AmlClock::IsSupportedMux(uint32_t id, uint16_t supported_mask) {
  const uint16_t index = aml_clk_common::AmlClkIndex(id);
  const uint16_t type = static_cast<uint16_t>(aml_clk_common::AmlClkType(id));

  if ((type & supported_mask) == 0) {
    fdf::error("{}: Unsupported mux type for operation, clkid = {}", __func__, id);
    return ZX_ERR_NOT_SUPPORTED;
  }

  if (!muxes_ || mux_count_ == 0) {
    fdf::error("{}: Platform does not have mux support.", __func__);
    return ZX_ERR_NOT_SUPPORTED;
  }

  if (index >= mux_count_) {
    fdf::error("{}: Mux index out of bounds, count = {}, idx = {}", __func__, mux_count_, index);
    return ZX_ERR_OUT_OF_RANGE;
  }

  return ZX_OK;
}

void AmlClock::SetInput(SetInputRequestView request, fdf::Arena& arena,
                        SetInputCompleter::Sync& completer) {
  constexpr uint16_t kSupported = static_cast<uint16_t>(aml_clk_common::aml_clk_type::kMesonMux);
  zx_status_t st = IsSupportedMux(request->id, kSupported);
  if (st != ZX_OK) {
    completer.buffer(arena).ReplyError(st);
    return;
  }

  const uint16_t index = aml_clk_common::AmlClkIndex(request->id);

  fbl::AutoLock al(&lock_);

  const meson_clk_mux_t& mux = muxes_[index];

  if (request->idx >= mux.n_inputs) {
    fdf::error("{}: mux input index out of bounds, max = {}, idx = {}.", __func__, mux.n_inputs,
               request->idx);
    completer.buffer(arena).ReplyError(ZX_ERR_OUT_OF_RANGE);
    return;
  }

  uint32_t clkidx;
  if (mux.inputs) {
    clkidx = mux.inputs[request->idx];
  } else {
    clkidx = request->idx;
  }

  uint32_t val = hiu_mmio_->Read32(mux.reg);
  val &= ~(mux.mask << mux.shift);
  val |= (clkidx & mux.mask) << mux.shift;
  hiu_mmio_->Write32(val, mux.reg);

  completer.buffer(arena).ReplySuccess();
}

void AmlClock::GetNumInputs(GetNumInputsRequestView request, fdf::Arena& arena,
                            GetNumInputsCompleter::Sync& completer) {
  constexpr uint16_t kSupported =
      (static_cast<uint16_t>(aml_clk_common::aml_clk_type::kMesonMux) |
       static_cast<uint16_t>(aml_clk_common::aml_clk_type::kMesonMuxRo));

  zx_status_t st = IsSupportedMux(request->id, kSupported);
  if (st != ZX_OK) {
    completer.buffer(arena).ReplyError(st);
    return;
  }

  const uint16_t index = aml_clk_common::AmlClkIndex(request->id);

  const meson_clk_mux_t& mux = muxes_[index];

  completer.buffer(arena).ReplySuccess(mux.n_inputs);
}

void AmlClock::GetInput(GetInputRequestView request, fdf::Arena& arena,
                        GetInputCompleter::Sync& completer) {
  // Bitmask representing clock types that support this operation.
  constexpr uint16_t kSupported =
      (static_cast<uint16_t>(aml_clk_common::aml_clk_type::kMesonMux) |
       static_cast<uint16_t>(aml_clk_common::aml_clk_type::kMesonMuxRo));

  zx_status_t st = IsSupportedMux(request->id, kSupported);
  if (st != ZX_OK) {
    completer.buffer(arena).ReplyError(st);
    return;
  }

  const uint16_t index = aml_clk_common::AmlClkIndex(request->id);

  const meson_clk_mux_t& mux = muxes_[index];

  const uint32_t result = (hiu_mmio_->Read32(mux.reg) >> mux.shift) & mux.mask;

  if (mux.inputs) {
    for (uint32_t i = 0; i < mux.n_inputs; i++) {
      if (result == mux.inputs[i]) {
        completer.buffer(arena).ReplySuccess(i);
        return;
      }
    }
  }

  completer.buffer(arena).ReplySuccess(result);
}

void AmlClock::GetClockProperties(fdf::Arena& arena, GetClockPropertiesCompleter::Sync& completer) {
  fidl::VectorView<fuchsia_hardware_clockimpl::wire::ClockProperties> result(arena,
                                                                             clk_table_count_);
  for (size_t i = 0; i < clk_table_count_; i++) {
    result[i] = fuchsia_hardware_clockimpl::wire::ClockProperties::Builder(arena)
                    .clock_id(static_cast<uint32_t>(i))
                    .clock_name(fidl::StringView::FromExternal(clk_table_[i]))
                    .Build();
  }

  completer.buffer(arena).ReplySuccess(result);
}

// Note: The clock index taken here are the index of clock
// from the clock table and not the clock_gates index.
// This API measures the clk frequency for clk.
// Following implementation is adopted from Amlogic SDK,
// there is absolutely no documentation.
zx_status_t AmlClock::ClkMeasureUtil(uint32_t id, uint64_t* clk_freq) {
  if (!msr_mmio_) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Set the measurement gate to 64uS.
  uint32_t value = 64 - 1;
  msr_mmio_->Write32(value, clk_msr_offsets_.reg0_offset);
  // Disable continuous measurement.
  // Disable interrupts.
  value = MSR_CONT | MSR_INTR;
  // Clear the clock source.
  value |= MSR_CLK_SRC_MASK << MSR_CLK_SRC_SHIFT;
  msr_mmio_->ClearBits32(value, clk_msr_offsets_.reg0_offset);

  value = ((id << MSR_CLK_SRC_SHIFT) |  // Select the MUX.
           MSR_RUN |                    // Enable the clock.
           MSR_ENABLE);                 // Enable measuring.
  msr_mmio_->SetBits32(value, clk_msr_offsets_.reg0_offset);

  // Wait for the measurement to be done.
  for (uint32_t i = 0; i < MSR_WAIT_BUSY_RETRIES; i++) {
    value = msr_mmio_->Read32(clk_msr_offsets_.reg0_offset);
    if (value & MSR_BUSY) {
      // Wait a little bit before trying again.
      zx_nanosleep(zx_deadline_after(ZX_USEC(MSR_WAIT_BUSY_TIMEOUT_US)));
      continue;
    }
    // Disable measuring.
    msr_mmio_->ClearBits32(MSR_ENABLE, clk_msr_offsets_.reg0_offset);
    // Get the clock value.
    value = msr_mmio_->Read32(clk_msr_offsets_.reg2_offset);
    // Magic numbers, since lack of documentation.
    *clk_freq = (((value + 31) & MSR_VAL_MASK) / 64);
    return ZX_OK;
  }
  return ZX_ERR_TIMED_OUT;
}

void AmlClock::Measure(MeasureRequestView request, MeasureCompleter::Sync& completer) {
  fuchsia_hardware_clock_measure::wire::FrequencyInfo info;
  if (request->clock >= clk_table_count_) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  std::string name = clk_table_[request->clock];
  if (name.length() >= fuchsia_hardware_clock_measure::wire::kMaxNameLen) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  info.name = fidl::StringView::FromExternal(name);
  zx_status_t status = ClkMeasureUtil(request->clock, &info.frequency);
  if (status != ZX_OK) {
    completer.ReplyError(status);
    return;
  }

  completer.ReplySuccess(info);
}

void AmlClock::GetCount(GetCountCompleter::Sync& completer) {
  completer.Reply(static_cast<uint32_t>(clk_table_count_));
}

AmlClock::~AmlClock() {
  hiu_mmio_.reset();

  if (msr_mmio_) {
    msr_mmio_->reset();
  }
}

zx_status_t AmlClock::GetMesonRateClock(const uint32_t id, MesonRateClock** out) {
  aml_clk_common::aml_clk_type type = aml_clk_common::AmlClkType(id);
  const uint16_t clkid = aml_clk_common::AmlClkIndex(id);

  switch (type) {
    case aml_clk_common::aml_clk_type::kMesonPll:
      if (clkid >= pll_count_) {
        fdf::error("{}: HIU PLL out of range, clkid = {}.", __func__, clkid);
        return ZX_ERR_INVALID_ARGS;
      }

      *out = &pllclk_[clkid];
      return ZX_OK;
    case aml_clk_common::aml_clk_type::kMesonCpuClk:
      if (clkid >= cpu_clks_.size()) {
        fdf::error("{}: cpu clk out of range, clkid = {}.", __func__, clkid);
        return ZX_ERR_INVALID_ARGS;
      }

      *out = &cpu_clks_[clkid];
      return ZX_OK;
    default:
      fdf::error("{}: Unsupported clock type, type = 0x{:x}\n", __func__,
                 static_cast<unsigned short>(type));
      return ZX_ERR_NOT_SUPPORTED;
  }

  __UNREACHABLE;
}

void AmlClock::InitHiu() {
  pllclk_.reserve(pll_count_);
  s905d2_hiu_init_etc(&*hiudev_, hiu_mmio_->View(0));
  for (unsigned int pllnum = 0; pllnum < pll_count_; pllnum++) {
    const hhi_plls_t pll = static_cast<hhi_plls_t>(pllnum);
    pllclk_.emplace_back(pll, &*hiudev_);
    pllclk_[pllnum].Init();
  }
}

void AmlClock::InitHiuA5() {
  pllclk_.reserve(pll_count_);
  for (unsigned int pllnum = 0; pllnum < pll_count_; pllnum++) {
    auto plldev = a5::CreatePllDevice(&dosbus_mmio_.value(), pllnum);
    pllclk_.emplace_back(std::move(plldev));
    pllclk_[pllnum].Init();
  }
}

void AmlClock::InitHiuA1() {
  pllclk_.reserve(pll_count_);
  for (unsigned int pllnum = 0; pllnum < pll_count_; pllnum++) {
    auto plldev = a1::CreatePllDevice(&dosbus_mmio_.value(), pllnum);
    pllclk_.emplace_back(std::move(plldev));
    pllclk_[pllnum].Init();
  }
}

}  // namespace amlogic_clock

FUCHSIA_DRIVER_EXPORT2(amlogic_clock::AmlClock);
