// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-gpu.h"

#include <fidl/fuchsia.hardware.gpu.amlogic/cpp/fidl.h>
#include <fidl/fuchsia.hardware.gpu.mali/cpp/wire.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/compat/cpp/metadata.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/trace/event.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <zircon/errors.h>
#include <zircon/process.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/smc.h>

#include <bind/fuchsia/cpp/bind.h>
#include <soc/aml-common/aml-registers.h>

#include "s905d2-gpu.h"
#include "s912-gpu.h"
#include "src/devices/tee/drivers/optee/tee-smc.h"
#include "t931-gpu.h"

namespace aml_gpu {

AmlGpu::AmlGpu() : fdf::DriverBase2("aml-gpu") {}

AmlGpu::~AmlGpu() {}

void AmlGpu::SetClkFreqSource(int32_t clk_source) {
  if (current_clk_source_ == clk_source) {
    return;
  }

  fdf::info("Setting clock source to {}: {}", clk_source, gpu_block_->gpu_clk_freq[clk_source]);
  uint32_t current_clk_cntl = hiu_buffer_->Read32(4 * gpu_block_->hhi_clock_cntl_offset);
  uint32_t enabled_mux = current_clk_cntl & (1 << kFinalMuxBitShift);
  uint32_t new_mux = enabled_mux == 0;
  uint32_t mux_shift = new_mux ? 16 : 0;

  // clear existing values
  current_clk_cntl &= ~(kClockMuxMask << mux_shift);
  // set the divisor, enable & source for the unused mux
  current_clk_cntl |= CalculateClockMux(true, gpu_block_->gpu_clk_freq[clk_source], 1) << mux_shift;

  // Write the new values to the unused mux
  hiu_buffer_->Write32(current_clk_cntl, 4 * gpu_block_->hhi_clock_cntl_offset);
  zx_nanosleep(zx_deadline_after(ZX_USEC(10)));

  // Toggle current mux selection
  current_clk_cntl ^= (1 << kFinalMuxBitShift);

  // Select the unused input mux
  hiu_buffer_->Write32(current_clk_cntl, 4 * gpu_block_->hhi_clock_cntl_offset);

  current_clk_source_ = clk_source;
  UpdateClockProperties();
}

void AmlGpu::SetInitialClkFreqSource(int32_t clk_source) {
  uint32_t current_clk_cntl = hiu_buffer_->Read32(4 * gpu_block_->hhi_clock_cntl_offset);
  uint32_t enabled_mux = (current_clk_cntl & (1 << kFinalMuxBitShift)) != 0;
  uint32_t mux_shift = enabled_mux ? 16 : 0;

  if (current_clk_cntl & (1 << (mux_shift + kClkEnabledBitShift))) {
    SetClkFreqSource(clk_source);
  } else {
    fdf::info("Setting initial clock source to {}: {}", clk_source,
              gpu_block_->gpu_clk_freq[clk_source]);
    // Switching the final dynamic mux from a disabled source to an enabled
    // source doesn't work. If the current clock source is disabled, then
    // enable it instead of switching.
    current_clk_cntl &= ~(kClockMuxMask << mux_shift);
    current_clk_cntl |= CalculateClockMux(true, gpu_block_->gpu_clk_freq[clk_source], 1)
                        << mux_shift;

    // Write the new values to the existing mux.
    hiu_buffer_->Write32(current_clk_cntl, 4 * gpu_block_->hhi_clock_cntl_offset);
    zx_nanosleep(zx_deadline_after(ZX_USEC(10)));
    current_clk_source_ = clk_source;
    UpdateClockProperties();
  }
}

void AmlGpu::UpdateClockProperties() {
  current_clk_source_property_.Set(current_clk_source_);
  uint32_t clk_mux_source = gpu_block_->gpu_clk_freq[current_clk_source_];
  current_clk_mux_source_property_.Set(clk_mux_source);
  ZX_DEBUG_ASSERT(clk_mux_source < kClockInputs);
  uint32_t current_clk_freq_hz = gpu_block_->input_freq_map[clk_mux_source];
  current_clk_freq_hz_property_.Set(current_clk_freq_hz);
  TRACE_INSTANT("magma", "AmlGpu::UpdateClockProperties", TRACE_SCOPE_PROCESS, "current_clk_source",
                current_clk_source_, "clk_mux_source", clk_mux_source, "current_clk_freq_hz",
                current_clk_freq_hz);
}

zx_status_t AmlGpu::Gp0Init(const fdf::Namespace& incoming) {
  auto clock_client = incoming.Connect<fuchsia_hardware_clock::Service::Clock>("clock-gp0-pll");
  if (clock_client.is_error() || !clock_client.value().is_valid()) {
    fdf::error("could not get clock fragment");
    return ZX_ERR_NO_RESOURCES;
  }

  gp0_clock_ = fidl::WireSyncClient(std::move(clock_client.value()));

  // Errors setting the clock should be logged but ignored; initialization can continue either way.
  auto set_result = gp0_clock_->SetRate(846000000);
  if (set_result.status() != ZX_OK) {
    fdf::error("Setting clock frequency failed, {}", set_result.FormatDescription());
    return ZX_OK;
  }

  if (set_result->is_error()) {
    fdf::error("Setting clock frequency failed, {}",
               zx_status_get_string(set_result->error_value()));
    return ZX_OK;
  }

  auto enable_result = gp0_clock_->Enable();
  if (enable_result.status() != ZX_OK) {
    fdf::error("Enabling clock failed, {}", enable_result.FormatDescription());
    return ZX_OK;
  }

  if (enable_result->is_error()) {
    fdf::error("Enabling clock failed, {}", zx_status_get_string(enable_result->error_value()));
    return ZX_OK;
  }

  gp0_init_succeeded_ = true;
  root_.RecordBool("gp0_init_succeeded", true);
  return ZX_OK;
}

void AmlGpu::InitClock() {
  {
    auto result = reset_register_->WriteRegister32(gpu_block_->reset0_mask_offset,
                                                   aml_registers::MALI_RESET0_MASK, 0);
    if ((result.status() != ZX_OK) || result->is_error()) {
      fdf::error("Reset0 Mask Clear failed");
    }
  }

  {
    auto result = reset_register_->WriteRegister32(gpu_block_->reset0_level_offset,
                                                   aml_registers::MALI_RESET0_MASK, 0);
    if ((result.status() != ZX_OK) || result->is_error()) {
      fdf::error("Reset0 Level Clear failed");
    }
  }

  {
    auto result = reset_register_->WriteRegister32(gpu_block_->reset2_mask_offset,
                                                   aml_registers::MALI_RESET2_MASK, 0);
    if ((result.status() != ZX_OK) || result->is_error()) {
      fdf::error("Reset2 Mask Clear failed");
    }
  }

  {
    auto result = reset_register_->WriteRegister32(gpu_block_->reset2_level_offset,
                                                   aml_registers::MALI_RESET2_MASK, 0);
    if ((result.status() != ZX_OK) || result->is_error()) {
      fdf::error("Reset2 Level Clear failed");
    }
  }

  uint32_t initial_clock_index = gpu_block_->initial_clock_index;
  if (gpu_block_->enable_gp0 && !gp0_init_succeeded_) {
    initial_clock_index = gpu_block_->non_gp0_index;
  }

  SetInitialClkFreqSource(static_cast<int32_t>(initial_clock_index));

  {
    auto result = reset_register_->WriteRegister32(gpu_block_->reset0_level_offset,
                                                   aml_registers::MALI_RESET0_MASK,
                                                   aml_registers::MALI_RESET0_MASK);
    if ((result.status() != ZX_OK) || result->is_error()) {
      fdf::error("Reset0 Level Set failed");
    }
  }

  {
    auto result = reset_register_->WriteRegister32(gpu_block_->reset2_level_offset,
                                                   aml_registers::MALI_RESET2_MASK,
                                                   aml_registers::MALI_RESET2_MASK);
    if ((result.status() != ZX_OK) || result->is_error()) {
      fdf::error("Reset2 Level Set failed");
    }
  }

  gpu_buffer_->Write32(0x2968A819, 4 * kPwrKey);
  gpu_buffer_->Write32(0xfff | (0x20 << 16), 4 * kPwrOverride1);
}

void AmlGpu::GetProperties(fdf::Arena& arena, GetPropertiesCompleter::Sync& completer) {
  completer.buffer(arena).Reply(properties_);
}

// Match the definitions in the Amlogic OPTEE implementation.
#define DMC_DEV_ID_GPU 1

#define DMC_DEV_TYPE_NON_SECURE 0
#define DMC_DEV_TYPE_SECURE 1
#define DMC_DEV_TYPE_INACCESSIBLE 2

zx_status_t AmlGpu::SetProtected(uint32_t protection_mode) {
  if (!secure_monitor_)
    return ZX_ERR_NOT_SUPPORTED;

  // Call into the TEE to mark a particular hardware unit as able to access
  // protected memory or not.
  zx_smc_parameters_t params = {};
  zx_smc_result_t result = {};
  constexpr uint32_t kFuncIdConfigDeviceSecure = 14;
  params.func_id = tee_smc::CreateFunctionId(tee_smc::kFastCall, tee_smc::kSmc32CallConv,
                                             tee_smc::kTrustedOsService, kFuncIdConfigDeviceSecure);
  params.arg1 = DMC_DEV_ID_GPU;
  params.arg2 = protection_mode;
  zx_status_t status = zx_smc_call(secure_monitor_.get(), &params, &result);
  if (status != ZX_OK) {
    fdf::error("Failed to set unit {} protected status {} code: {}", params.arg1, params.arg2,
               status);
    return status;
  }
  if (result.arg0 != 0) {
    fdf::error("Failed to set unit {} protected status {}: {:x}", params.arg1, params.arg2,
               result.arg0);
    return ZX_ERR_INTERNAL;
  }
  current_protected_mode_property_.Set(protection_mode);
  return ZX_OK;
}

void AmlGpu::EnterProtectedMode(fdf::Arena& arena, EnterProtectedModeCompleter::Sync& completer) {
  if (!secure_monitor_) {
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
    return;
  }

  zx_status_t status = SetProtected(DMC_DEV_TYPE_SECURE);
  if (status == ZX_OK) {
    completer.buffer(arena).ReplySuccess();
  } else {
    completer.buffer(arena).ReplyError(status);
  }
}

void AmlGpu::StartExitProtectedMode(fdf::Arena& arena,
                                    StartExitProtectedModeCompleter::Sync& completer) {
  if (!secure_monitor_) {
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
    return;
  }
  // Switch device to inaccessible mode. This will prevent writes to all memory
  // and start resetting the GPU.
  zx_status_t status = SetProtected(DMC_DEV_TYPE_INACCESSIBLE);
  if (status == ZX_OK) {
    completer.buffer(arena).ReplySuccess();
  } else {
    completer.buffer(arena).ReplyError(status);
  }
}

void AmlGpu::FinishExitProtectedMode(fdf::Arena& arena,
                                     FinishExitProtectedModeCompleter::Sync& completer) {
  if (!secure_monitor_) {
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
    return;
  }
  // Switch to non-secure mode. This will check that the device has been reset
  // and will re-enable access to non-protected memory.
  zx_status_t status = SetProtected(DMC_DEV_TYPE_NON_SECURE);
  if (status == ZX_OK) {
    completer.buffer(arena).ReplySuccess();
  } else {
    completer.buffer(arena).ReplyError(status);
  }
}

zx::result<> AmlGpu::Start(fdf::DriverContext context) {
  zx::result loop_dispatcher = fdf::SynchronizedDispatcher::Create(
      fdf::SynchronizedDispatcher::Options{}, "aml-gpu-thread", [](fdf_dispatcher_t* dispatcher) {},
      "fuchsia.graphics.drivers.aml-gpu");

  if (!loop_dispatcher.is_ok()) {
    fdf::error("Creating dispatcher failed, status={}", loop_dispatcher);
    return loop_dispatcher.take_error();
  }
  loop_dispatcher_ = *std::move(loop_dispatcher);
  root_ = inspector_.GetRoot().CreateChild("aml-gpu");
  current_clk_source_property_ = root_.CreateUint("current_clk_source", current_clk_source_);
  current_clk_mux_source_property_ = root_.CreateUint("current_clk_mux_source", 0);
  current_clk_freq_hz_property_ = root_.CreateUint("current_clk_freq_hz", 0);
  // GPU is in unknown mode on Bind.
  current_protected_mode_property_ = root_.CreateInt("current_protected_mode", -1);
  auto builder = fuchsia_hardware_gpu_mali::wire::MaliProperties::Builder(arena_);

  zx::result pdev_client_end =
      context.incoming().Connect<fuchsia_hardware_platform_device::Service::Device>("pdev");
  if (pdev_client_end.is_error()) {
    fdf::error("Failed to connect to platform device: {}", pdev_client_end);
    return pdev_client_end.take_error();
  }
  fdf::PDev pdev(std::move(pdev_client_end.value()));

  zx::result metadata = pdev.GetFidlMetadata<fuchsia_hardware_gpu_amlogic::Metadata>();
  if (metadata.is_error()) {
    if (metadata.status_value() != ZX_ERR_NOT_FOUND) {
      fdf::error("Failed to get metadata: {}", metadata);
      return metadata.take_error();
    }
  } else {
    builder.supports_protected_mode(metadata->supports_protected_mode().has_value() &&
                                    metadata->supports_protected_mode().value());
  }

  zx::result gpu_buffer = pdev.MapMmio(kMmioGpuIndex);
  if (gpu_buffer.is_error()) {
    fdf::error("Failed to map gpu buffer: {}", gpu_buffer);
    return gpu_buffer.take_error();
  }
  gpu_buffer_ = std::move(gpu_buffer.value());

  zx::result hiu_buffer = pdev.MapMmio(kMmioHiuIndex);
  if (hiu_buffer.is_error()) {
    fdf::error("Failed to map hiu buffer: {}", hiu_buffer);
    return hiu_buffer.take_error();
  }
  hiu_buffer_ = std::move(hiu_buffer.value());

  zx::result info_result = pdev.GetDeviceInfo();
  if (info_result.is_error()) {
    fdf::error("Failed to get device info: {}", info_result);
    return info_result.take_error();
  }
  fdf::PDev::DeviceInfo info = std::move(info_result.value());

  if (info.vid == PDEV_VID_GENERIC && info.pid == PDEV_PID_GENERIC &&
      info.did == PDEV_DID_DEVICETREE_NODE) {
    // TODO(https://fxbug.dev/318736574) : Remove and rely only on GetDeviceInfo.
    zx::result board_info_result = pdev.GetBoardInfo();
    if (board_info_result.is_error()) {
      fdf::error("Failed to get board info: {}", board_info_result);
      return board_info_result.take_error();
    }
    fdf::PDev::BoardInfo board_info = std::move(board_info_result.value());
    if (board_info.vid == PDEV_VID_KHADAS) {
      switch (board_info.pid) {
        case PDEV_PID_VIM3:
          info.pid = PDEV_PID_AMLOGIC_A311D;
          break;
        default:
          fdf::error("Unsupported PID 0x{:x} for VID 0x{:x}", board_info.pid, board_info.vid);
          return zx::error(ZX_ERR_INVALID_ARGS);
      }
    } else if (board_info.vid == PDEV_VID_GOOGLE) {
      switch (board_info.pid) {
        case PDEV_PID_ASTRO:
          info.pid = PDEV_PID_AMLOGIC_S905D2;
          break;
        case PDEV_PID_SHERLOCK:
          info.pid = PDEV_PID_AMLOGIC_T931;
          break;
        case PDEV_PID_NELSON:
          info.pid = PDEV_PID_AMLOGIC_S905D3;
          break;
        default:
          fdf::error("Unsupported PID 0x{:x} for VID 0x{:x}", board_info.pid, board_info.vid);
          return zx::error(ZX_ERR_INVALID_ARGS);
      }
    } else {
      fdf::error("Unsupported VID 0x{:x}", board_info.vid);
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
  }

  switch (info.pid) {
    case PDEV_PID_AMLOGIC_S912:
      gpu_block_ = &s912_gpu_blocks;
      break;
    case PDEV_PID_AMLOGIC_S905D2:
    case PDEV_PID_AMLOGIC_S905D3:
      gpu_block_ = &s905d2_gpu_blocks;
      break;
    // A311D and T931 have the same GPU registers.
    case PDEV_PID_AMLOGIC_T931:
    case PDEV_PID_AMLOGIC_A311D:
      gpu_block_ = &t931_gpu_blocks;
      break;
    default:
      fdf::error("unsupported SOC PID {}", info.pid);
      return zx::error(ZX_ERR_INVALID_ARGS);
  }

  auto reset_register_client =
      context.incoming().Connect<fuchsia_hardware_registers::Service::Device>("register-reset");
  if (reset_register_client.is_error() || !reset_register_client.value().is_valid()) {
    fdf::error("could not get register-reset fragment");
    return zx::error(ZX_ERR_NO_RESOURCES);
  }

  reset_register_ = fidl::WireSyncClient(std::move(reset_register_client.value()));

  if (info.pid == PDEV_PID_AMLOGIC_S905D3 && builder.supports_protected_mode()) {
    // S905D3 needs to use an SMC into the TEE to do protected mode switching.
    static constexpr uint32_t kTrustedOsSmcIndex = 0;
    zx::result secure_monitor = pdev.GetSmc(kTrustedOsSmcIndex);
    if (secure_monitor.is_error()) {
      fdf::error("Failed to retrieve secure monitor SMC: {}", secure_monitor);
      return secure_monitor.take_error();
    }
    secure_monitor_ = std::move(secure_monitor.value());
    builder.use_protected_mode_callbacks(true);
  }

  if (gpu_block_->enable_gp0) {
    zx_status_t status = Gp0Init(context.incoming());
    if (status != ZX_OK) {
      fdf::error("aml_gp0_init failed: {}. Falling back to lower clock.", status);
      return zx::error(status);
    }
  }

  properties_ = builder.Build();

  InitClock();

  auto protocol = [this](fdf::ServerEnd<fuchsia_hardware_gpu_mali::ArmMali> server_end) mutable {
    fdf::BindServer(loop_dispatcher_.get(), std::move(server_end), this);
  };

  fuchsia_hardware_gpu_mali::Service::InstanceHandler handler({.arm_mali = std::move(protocol)});
  {
    auto status = outgoing()->AddService<fuchsia_hardware_gpu_mali::Service>(std::move(handler));
    if (status.is_error()) {
      fdf::error("{}: Failed to add service to outgoing directory: {}", __func__, status);
      return status.take_error();
    }
  }

  std::vector offers = {fdf::MakeOffer2<fuchsia_hardware_gpu_mali::Service>("default")};

  std::vector properties = {
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpu.mali.Service"),
  };

  zx::result child = AddChild("aml-gpu", properties, offers);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child);
    return child.take_error();
  }

  return zx::ok();
}

}  // namespace aml_gpu

// clang-format off
FUCHSIA_DRIVER_EXPORT2(aml_gpu::AmlGpu);
