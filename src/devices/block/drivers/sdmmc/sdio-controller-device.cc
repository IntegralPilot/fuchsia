// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdio-controller-device.h"

#include <fidl/fuchsia.hardware.sdio/cpp/wire.h>
#include <fuchsia/hardware/sdio/c/banjo.h>
#include <fuchsia/hardware/sdmmc/c/banjo.h>
#include <inttypes.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/dispatcher.h>
#include <lib/fit/defer.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/sdio/hw.h>
#include <lib/zx/clock.h>
#include <stdlib.h>
#include <string.h>
#include <zircon/process.h>
#include <zircon/threads.h>

#include <algorithm>

#include <fbl/algorithm.h>

#include "sdmmc-root-device.h"

namespace {

constexpr uint8_t kCccrVendorAddressMin = 0xf0;

uint32_t SdioReadTupleBody(const uint8_t* tuple_body, size_t start, size_t numbytes) {
  uint32_t res = 0;

  for (size_t i = start; i < (start + numbytes); i++) {
    res |= tuple_body[i] << ((i - start) * 8);
  }
  return res;
}

inline bool SdioFnIdxValid(uint8_t fn_idx) { return (fn_idx < SDIO_MAX_FUNCS); }

inline uint8_t GetBits(uint32_t x, uint32_t mask, uint32_t loc) {
  return static_cast<uint8_t>((x & mask) >> loc);
}

inline void UpdateBitsU8(uint8_t* x, uint8_t mask, uint8_t loc, uint8_t val) {
  *x = static_cast<uint8_t>(*x & ~mask);
  *x = static_cast<uint8_t>(*x | ((val << loc) & mask));
}

inline uint8_t GetBitsU8(uint8_t x, uint8_t mask, uint8_t loc) {
  return static_cast<uint8_t>((x & mask) >> loc);
}

}  // namespace

namespace sdmmc {

zx_status_t SdioControllerDevice::Create(SdmmcRootDevice* parent,
                                         std::unique_ptr<SdmmcDevice> sdmmc,
                                         std::unique_ptr<SdioControllerDevice>* out_dev) {
  fbl::AllocChecker ac;
  out_dev->reset(new (&ac) SdioControllerDevice(parent, std::move(sdmmc)));
  if (!ac.check()) {
    fdf::error("failed to allocate device memory");
    return ZX_ERR_NO_MEMORY;
  }

  return ZX_OK;
}

zx_status_t SdioControllerDevice::Probe(const fuchsia_hardware_sdmmc::SdmmcMetadata& metadata) {
  if (metadata.vccq_off_with_controller_off()) {
    vccq_off_with_controller_off_ = *metadata.vccq_off_with_controller_off();
  }

  std::lock_guard<std::mutex> lock(lock_);
  return ProbeLocked();
}

zx_status_t SdioControllerDevice::ProbeLocked() {
  zx_status_t st = SdioReset();

  if ((st = sdmmc_->SdmmcGoIdle()) != ZX_OK) {
    fdf::error("SDMMC_GO_IDLE_STATE failed, retcode = {}", st);
    return st;
  }

  sdmmc_->SdSendIfCond();

  uint32_t ocr;
  if ((st = sdmmc_->SdioSendOpCond(0, &ocr)) != ZX_OK) {
    fdf::debug("SDIO_SEND_OP_COND failed, retcode = {}", st);
    return st;
  }
  // Select voltage 3.3 V. Also request for 1.8V. Section 3.2 SDIO spec
  constexpr uint32_t kOcr33vS18r = SDIO_SEND_OP_COND_IO_OCR_33V | SDIO_SEND_OP_COND_CMD_S18R;
  if ((st = sdmmc_->SdioSendOpCond(kOcr33vS18r, &ocr)) != ZX_OK) {
    fdf::error("SDIO_SEND_OP_COND failed, retcode = {}", st);
    return st;
  }

  const zx::time deadline = zx::deadline_after(zx::sec(1));
  while (!(ocr & SDIO_SEND_OP_COND_RESP_IORDY)) {
    zx::nanosleep(zx::deadline_after(zx::msec(1)));
    if ((st = sdmmc_->SdioSendOpCond(kOcr33vS18r, &ocr)) != ZX_OK) {
      fdf::error("SDIO_SEND_OP_COND failed, retcode = {}", st);
      return st;
    }

    if (zx::clock::get_monotonic() >= deadline) {
      break;
    }
  }

  if (ocr & SDIO_SEND_OP_COND_RESP_MEM_PRESENT) {
    // Combo cards not supported
    fdf::error("Combo card not supported");
    return ZX_ERR_NOT_SUPPORTED;
  }
  if (!(ocr & SDIO_SEND_OP_COND_RESP_IORDY)) {
    fdf::warn("IO not ready after SDIO_SEND_OP_COND");
    return ZX_ERR_IO;
  }
  if (ocr & SDIO_SEND_OP_COND_RESP_S18A) {
    if ((st = sdmmc_->SdSwitchUhsVoltage(ocr)) != ZX_OK) {
      fdf::error("Failed to switch voltage to 1.8V");
      return st;
    }
  }
  hw_info_.num_funcs =
      GetBits(ocr, SDIO_SEND_OP_COND_RESP_NUM_FUNC_MASK, SDIO_SEND_OP_COND_RESP_NUM_FUNC_LOC);
  if ((st = sdmmc_->SdSendRelativeAddr(nullptr)) != ZX_OK) {
    fdf::error("SD_SEND_RELATIVE_ADDR failed, retcode = {}", st);
    return st;
  }

  // Increment to account for function 0, which is not represented in OCR.
  hw_info_.num_funcs++;

  if ((st = sdmmc_->MmcSelectCard()) != ZX_OK) {
    fdf::error("MMC_SELECT_CARD failed, retcode = {}", st);
    return st;
  }

  sdmmc_->SetRequestRetries(10);

  if ((st = ProcessCccr()) != ZX_OK) {
    fdf::error("Read CCCR failed, retcode = {}", st);
    return st;
  }

  // Read CIS to get max block size
  if ((st = ProcessCis(0)) != ZX_OK) {
    fdf::error("Read CIS failed, retcode = {}", st);
    return st;
  }

  if (ocr & SDIO_SEND_OP_COND_RESP_S18A) {
    // The voltage switch succeeded and we're now in SDR12 mode. Move to the highest available UHS-I
    // speed mode.
    if ((st = TrySwitchUhs()) != ZX_OK) {
      fdf::error("Switching to ultra high speed failed, retcode = {}", st);
      return st;
    }
  } else {
    // The device does not support UHS-I; move to High- or Full-Speed instead.
    if ((st = TrySwitchHs()) != ZX_OK) {
      fdf::error("Switching to high speed failed, retcode = {}", st);
      if ((st = SwitchFreq(SDIO_DEFAULT_FREQ)) != ZX_OK) {
        fdf::error("Switch freq retcode = {}", st);
        return st;
      }
    }
  }

  // This effectively excludes cards that don't report the mandatory FUNCE tuple, as the max block
  // size would still be set to zero.
  if ((st = SdioUpdateBlockSizeLocked(0, 0, true)) != ZX_OK) {
    fdf::error("Failed to update function 0 block size, retcode = {}", st);
    return st;
  }

  // 0 is the common function. Already initialized
  for (size_t i = 1; i < hw_info_.num_funcs; i++) {
    if ((st = InitFunc(static_cast<uint8_t>(i))) != ZX_OK) {
      fdf::error("Failed to initialize function {}, retcode = {}", i, st);
      return st;
    }
  }

  sdmmc_->SetRequestRetries(0);

  fdf::info("sdio device initialized successfully");
  fdf::info("          Manufacturer: 0x{:x}", funcs_[0].hw_info.manufacturer_id);
  fdf::info("          Product: 0x{:x}", funcs_[0].hw_info.product_id);
  fdf::info("          cccr vsn: 0x{:x}", hw_info_.cccr_vsn);
  fdf::info("          SDIO vsn: 0x{:x}", hw_info_.sdio_vsn);
  fdf::info("          num funcs: {}", hw_info_.num_funcs);
  return ZX_OK;
}

zx_status_t SdioControllerDevice::StartSdioIrqDispatcherIfNeeded() {
  std::lock_guard<std::mutex> lock(irq_dispatcher_lock_);

  if (shutdown_) {
    return ZX_ERR_CANCELED;
  }
  if (irq_dispatcher_.get()) {
    return ZX_OK;
  }

  for (uint32_t i = 1; i < hw_info_.num_funcs; i++) {
    zx_status_t status = zx::interrupt::create({}, 0, ZX_INTERRUPT_VIRTUAL, &sdio_irqs_[i]);
    if (status != ZX_OK) {
      fdf::error("Failed to create virtual interrupt for function {}: {}", i,
                 zx_status_get_string(status));
      return status;
    }
  }

  auto dispatcher = fdf::SynchronizedDispatcher::Create(
      fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "sdio-irq-thread",
      [&](fdf_dispatcher_t*) { irq_shutdown_completion_.Signal(); });
  if (dispatcher.is_error()) {
    fdf::error("Failed to create dispatcher: {}", zx_status_get_string(dispatcher.status_value()));
    return dispatcher.status_value();
  }
  irq_dispatcher_ = *std::move(dispatcher);

  auto [client, server] = fdf::Endpoints<fuchsia_hardware_sdmmc::InBandInterrupt>::Create();
  zx_status_t status = sdmmc_->RegisterInBandInterrupt(std::move(client));
  if (status != ZX_OK) {
    in_band_interrupt_supported_ = false;

    // Stop the dispatcher if in-band interrupts are not supported. We won't attempt to do any of
    // this again, so there is no need to reset any state.
    irq_dispatcher_.ShutdownAsync();
    irq_shutdown_completion_.Wait();
    return status;
  }

  async::PostTask(irq_dispatcher_.async_dispatcher(), [this, server = std::move(server)]() mutable {
    in_band_interrupt_binding_.emplace(irq_dispatcher_.get(), std::move(server), this,
                                       fidl::kIgnoreBindingClosure);
  });
  return ZX_OK;
}

zx_status_t SdioControllerDevice::AddDevice() {
  std::lock_guard<std::mutex> lock(lock_);

  auto [controller_client_end, controller_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();
  auto [node_client_end, node_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::Node>::Create();

  controller_.Bind(std::move(controller_client_end));
  sdio_controller_node_.Bind(std::move(node_client_end));

  fidl::Arena arena;

  const auto args =
      fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena).name(arena, kDeviceName).Build();

  auto result = parent_->root_node()->AddChild(args, std::move(controller_server_end),
                                               std::move(node_server_end));
  if (!result.ok()) {
    fdf::error("Failed to add child sdio controller device: {}", result.status_string());
    return result.status();
  }

  auto remove_device_on_error =
      fit::defer([&]() { [[maybe_unused]] auto result = controller_->Remove(); });

  zx_status_t st;
  for (uint32_t i = 0; i < hw_info_.num_funcs - 1; i++) {
    if ((st = SdioFunctionDevice::Create(this, i + 1, &child_sdio_function_devices_[i])) != ZX_OK) {
      return st;
    }
  }

  // Clear all bits except for function 0, then selectively set the rest depending on which
  // functions are actually present.
  function_power_on_.reset();
  function_power_on_.set(0);
  for (uint32_t i = 1; i < hw_info_.num_funcs; i++) {
    function_power_on_.set(i);
    if ((st = child_sdio_function_devices_[i - 1]->AddDevice(funcs_[i].hw_info)) != ZX_OK) {
      return st;
    }
  }

  root_ = parent_->driver_inspector().root().CreateChild("sdio_core");
  tx_errors_ = root_.CreateUint("tx_errors", 0);
  rx_errors_ = root_.CreateUint("rx_errors", 0);

  remove_device_on_error.cancel();
  return ZX_OK;
}

void SdioControllerDevice::StopSdioIrqDispatcher(std::optional<fdf::StopCompleter> completer) {
  shutdown_ = true;

  {
    std::lock_guard<std::mutex> lock(irq_dispatcher_lock_);
    if (irq_dispatcher_.get()) {
      irq_dispatcher_.ShutdownAsync();
      irq_shutdown_completion_.Wait();
    }
  }

  for (const zx::interrupt& irq : sdio_irqs_) {
    if (irq.is_valid()) {
      // Return an error to any waiters.
      irq.destroy();
    }
  }

  if (completer.has_value()) {
    completer.value()(zx::ok());
  }
}

zx_status_t SdioControllerDevice::SdioGetDevHwInfo(uint8_t fn_idx, sdio_hw_info_t* out_hw_info) {
  if (!SdioFnIdxValid(fn_idx)) {
    return ZX_ERR_INVALID_ARGS;
  }

  std::lock_guard<std::mutex> lock(lock_);

  memcpy(&out_hw_info->dev_hw_info, &hw_info_, sizeof(sdio_device_hw_info_t));
  memcpy(&out_hw_info->func_hw_info, &funcs_[fn_idx].hw_info, sizeof(sdio_func_hw_info_t));
  out_hw_info->host_max_transfer_size =
      static_cast<uint32_t>(sdmmc_->host_info().max_transfer_size);
  return ZX_OK;
}

zx_status_t SdioControllerDevice::SdioEnableFn(uint8_t fn_idx) {
  std::lock_guard<std::mutex> lock(lock_);
  return SdioEnableFnLocked(fn_idx);
}

zx_status_t SdioControllerDevice::SdioEnableFnLocked(uint8_t fn_idx) {
  uint8_t ioex_reg = 0;
  zx_status_t st = ZX_OK;

  if (!SdioFnIdxValid(fn_idx)) {
    return ZX_ERR_INVALID_ARGS;
  }

  SdioFunction& func = funcs_[fn_idx];
  if (func.enabled) {
    return ZX_OK;
  }
  if ((st = SdioDoRwByteLocked(false, 0, SDIO_CIA_CCCR_IOEx_EN_FUNC_ADDR, 0, &ioex_reg)) != ZX_OK) {
    fdf::error("Error enabling func:{} status:{}", fn_idx, zx_status_get_string(st));
    return st;
  }

  ioex_reg = static_cast<uint8_t>(ioex_reg | (1 << fn_idx));
  st = SdioDoRwByteLocked(true, 0, SDIO_CIA_CCCR_IOEx_EN_FUNC_ADDR, ioex_reg, nullptr);
  if (st != ZX_OK) {
    fdf::error("Error enabling func:{} status:{}", fn_idx, zx_status_get_string(st));
    return st;
  }
  // wait for the device to enable the func.
  zx::nanosleep(zx::deadline_after(zx::msec(10)));
  if ((st = SdioDoRwByteLocked(false, 0, SDIO_CIA_CCCR_IOEx_EN_FUNC_ADDR, 0, &ioex_reg)) != ZX_OK) {
    fdf::error("Error enabling func:{} status:{}", fn_idx, zx_status_get_string(st));
    return st;
  }

  if (!(ioex_reg & (1 << fn_idx))) {
    st = ZX_ERR_IO;
    fdf::error("Failed to enable func {}", fn_idx);
    return st;
  }

  func.enabled = true;
  fdf::debug("Func {} is enabled", fn_idx);
  return st;
}

zx_status_t SdioControllerDevice::SdioDisableFn(uint8_t fn_idx) {
  uint8_t ioex_reg = 0;
  zx_status_t st = ZX_OK;

  if (!SdioFnIdxValid(fn_idx)) {
    return ZX_ERR_INVALID_ARGS;
  }

  std::lock_guard<std::mutex> lock(lock_);

  SdioFunction* func = &funcs_[fn_idx];
  if (!func->enabled) {
    fdf::error("Func {} is not enabled", fn_idx);
    return ZX_ERR_IO;
  }

  if ((st = SdioDoRwByteLocked(false, 0, SDIO_CIA_CCCR_IOEx_EN_FUNC_ADDR, 0, &ioex_reg)) != ZX_OK) {
    fdf::error("Error reading IOEx reg. func: {} status: {}", fn_idx, zx_status_get_string(st));
    return st;
  }

  ioex_reg = static_cast<uint8_t>(ioex_reg & ~(1 << fn_idx));
  st = SdioDoRwByteLocked(true, 0, SDIO_CIA_CCCR_IOEx_EN_FUNC_ADDR, ioex_reg, nullptr);
  if (st != ZX_OK) {
    fdf::error("Error writing IOEx reg. func: {} status:{}", fn_idx, zx_status_get_string(st));
    return st;
  }

  func->enabled = false;
  fdf::debug("Function {} is disabled", fn_idx);
  return st;
}

zx_status_t SdioControllerDevice::SdioIoReady(uint8_t fn_idx, bool* out_ready) {
  if (!SdioFnIdxValid(fn_idx) || fn_idx == 0) {
    return ZX_ERR_INVALID_ARGS;
  }

  std::lock_guard<std::mutex> lock(lock_);

  uint8_t io_ready_reg = 0;

  zx_status_t st = SdioDoRwByteLocked(false, 0, SDIO_CIA_CCCR_IORx_FUNC_RDY_ADDR, 0, &io_ready_reg);
  if (st != ZX_OK) {
    fdf::error("Error reading I/O Ready: {} status: {}", fn_idx, zx_status_get_string(st));
    return st;
  }

  *out_ready = io_ready_reg & (1 << fn_idx);
  return ZX_OK;
}

zx_status_t SdioControllerDevice::SdioEnableFnIntr(uint8_t fn_idx) {
  zx_status_t st = ZX_OK;

  if (!SdioFnIdxValid(fn_idx)) {
    return ZX_ERR_INVALID_ARGS;
  }

  std::lock_guard<std::mutex> lock(lock_);

  SdioFunction* func = &funcs_[fn_idx];
  if (func->intr_enabled) {
    return ZX_OK;
  }

  uint8_t intr_byte;
  st = SdioDoRwByteLocked(false, 0, SDIO_CIA_CCCR_IEN_INTR_EN_ADDR, 0, &intr_byte);
  if (st != ZX_OK) {
    fdf::error("Failed to enable interrupt for fn: {} status: {}", fn_idx,
               zx_status_get_string(st));
    return st;
  }

  // Enable fn intr
  intr_byte = static_cast<uint8_t>(intr_byte | 1 << fn_idx);
  // Enable master intr
  intr_byte = static_cast<uint8_t>(intr_byte | 1);

  st = SdioDoRwByteLocked(true, 0, SDIO_CIA_CCCR_IEN_INTR_EN_ADDR, intr_byte, nullptr);
  if (st != ZX_OK) {
    fdf::error("Failed to enable interrupt for fn: {} status: {}", fn_idx,
               zx_status_get_string(st));
    return st;
  }

  func->intr_enabled = true;
  fdf::debug("Interrupt enabled for fn {}", fn_idx);
  return ZX_OK;
}

zx_status_t SdioControllerDevice::SdioDisableFnIntr(uint8_t fn_idx) {
  zx_status_t st = ZX_OK;

  if (!SdioFnIdxValid(fn_idx)) {
    return ZX_ERR_INVALID_ARGS;
  }

  std::lock_guard<std::mutex> lock(lock_);

  SdioFunction* func = &funcs_[fn_idx];
  if (!func->intr_enabled) {
    fdf::error("Interrupt is not enabled for {}", fn_idx);
    return ZX_ERR_BAD_STATE;
  }

  uint8_t intr_byte;
  if ((st = SdioDoRwByteLocked(false, 0, SDIO_CIA_CCCR_IEN_INTR_EN_ADDR, 0, &intr_byte)) != ZX_OK) {
    fdf::error("Failed reading intr enable reg. func: {} status: {}", fn_idx,
               zx_status_get_string(st));
    return st;
  }

  intr_byte = static_cast<uint8_t>(intr_byte & ~(1 << fn_idx));
  if (!(intr_byte & SDIO_ALL_INTR_ENABLED_MASK)) {
    // disable master as well
    intr_byte = 0;
  }

  st = SdioDoRwByteLocked(true, 0, SDIO_CIA_CCCR_IEN_INTR_EN_ADDR, intr_byte, nullptr);
  if (st != ZX_OK) {
    fdf::error("Error writing to intr enable reg. func: {} status: {}", fn_idx,
               zx_status_get_string(st));
    return st;
  }

  func->intr_enabled = false;
  fdf::debug("Interrupt disabled for fn {}", fn_idx);
  return ZX_OK;
}

zx_status_t SdioControllerDevice::SdioUpdateBlockSize(uint8_t fn_idx, uint16_t blk_sz, bool deflt) {
  std::lock_guard<std::mutex> lock(lock_);
  return SdioUpdateBlockSizeLocked(fn_idx, blk_sz, deflt);
}

zx_status_t SdioControllerDevice::SdioUpdateBlockSizeLocked(uint8_t fn_idx, uint16_t blk_sz,
                                                            bool deflt) {
  if (!SdioFnIdxValid(fn_idx)) {
    return ZX_ERR_INVALID_ARGS;
  }

  SdioFunction* func = &funcs_[fn_idx];
  if (deflt) {
    blk_sz = static_cast<uint16_t>(func->hw_info.max_blk_size);
  }

  // The minimum block size is 1 for all functions, as per the CCCR and FBR sections of the spec.
  if (blk_sz > func->hw_info.max_blk_size || blk_sz == 0) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (func->cur_blk_size == blk_sz) {
    return ZX_OK;
  }

  // This register is read-only if SMB is not set. DoRwTxn will use byte mode instead of block mode
  // in that case, so the register write can be skipped.
  if (hw_info_.caps & SDIO_CARD_MULTI_BLOCK) {
    zx_status_t st =
        WriteData16(0, SDIO_CIA_FBR_BASE_ADDR(fn_idx) + SDIO_CIA_FBR_BLK_SIZE_ADDR, blk_sz);
    if (st != ZX_OK) {
      fdf::error("Error setting blk size.fn: {} blk_sz: {} ret: {}", fn_idx, blk_sz,
                 zx_status_get_string(st));
      return st;
    }
  }

  func->cur_blk_size = blk_sz;
  return ZX_OK;
}

zx_status_t SdioControllerDevice::SdioGetBlockSize(uint8_t fn_idx, uint16_t* out_cur_blk_size) {
  if (!SdioFnIdxValid(fn_idx)) {
    return ZX_ERR_INVALID_ARGS;
  }

  std::lock_guard<std::mutex> lock(lock_);

  if (hw_info_.caps & SDIO_CARD_MULTI_BLOCK) {
    zx_status_t st = ReadData16(0, SDIO_CIA_FBR_BASE_ADDR(fn_idx) + SDIO_CIA_FBR_BLK_SIZE_ADDR,
                                out_cur_blk_size);
    if (st != ZX_OK) {
      fdf::error("Failed to get block size for fn: {} ret: {}", fn_idx, zx_status_get_string(st));
    }
    return st;
  }

  *out_cur_blk_size = funcs_[fn_idx].cur_blk_size;
  return ZX_OK;
}

zx_status_t SdioControllerDevice::SdioDoRwByte(bool write, uint8_t fn_idx, uint32_t addr,
                                               uint8_t write_byte, uint8_t* out_read_byte) {
  std::lock_guard<std::mutex> lock(lock_);
  return SdioDoRwByteLocked(write, fn_idx, addr, write_byte, out_read_byte);
}

zx::result<uint8_t> SdioControllerDevice::SdioReadByte(uint8_t function, uint32_t address,
                                                       bool suppress_error_messages) {
  std::lock_guard<std::mutex> lock(lock_);
  if (!SdioFnIdxValid(function)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (!function_power_on_[function]) {
    return zx::error(ZX_ERR_BAD_STATE);
  }
  if (shutdown_) {
    return zx::error(ZX_ERR_CANCELED);
  }

  return sdmmc_->SdioIoRwDirect(function, address, suppress_error_messages);
}

zx::result<uint8_t> SdioControllerDevice::SdioWriteByte(uint8_t function, uint32_t address,
                                                        uint8_t byte, bool read_after_write,
                                                        bool suppress_error_messages) {
  std::lock_guard<std::mutex> lock(lock_);
  if (!SdioFnIdxValid(function)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (!function_power_on_[function]) {
    return zx::error(ZX_ERR_BAD_STATE);
  }
  if (shutdown_) {
    return zx::error(ZX_ERR_CANCELED);
  }

  return sdmmc_->SdioIoRwDirect(function, address, byte, read_after_write, suppress_error_messages);
}

zx_status_t SdioControllerDevice::SdioDoRwByteLocked(bool write, uint8_t fn_idx, uint32_t addr,
                                                     uint8_t write_byte, uint8_t* out_read_byte) {
  if (!SdioFnIdxValid(fn_idx)) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (!function_power_on_[fn_idx]) {
    return ZX_ERR_BAD_STATE;
  }
  if (shutdown_) {
    return ZX_ERR_CANCELED;
  }

  out_read_byte = write ? nullptr : out_read_byte;
  write_byte = write ? write_byte : 0;
  return sdmmc_->SdioIoRwDirect(write, fn_idx, addr, write_byte, out_read_byte);
}

zx_status_t SdioControllerDevice::SdioGetInBandIntr(uint8_t fn_idx, zx::interrupt* out_irq) {
  if (!SdioFnIdxValid(fn_idx) || fn_idx == 0) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (!in_band_interrupt_supported_) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  if (const zx_status_t st = StartSdioIrqDispatcherIfNeeded(); st != ZX_OK) {
    return st;
  }

  return sdio_irqs_[fn_idx].duplicate(ZX_RIGHT_SAME_RIGHTS, out_irq);
}

void SdioControllerDevice::SdioAckInBandIntr(uint8_t fn_idx) {
  // Don't ack for function 0 interrupts. This should not be possible given the child devices we've
  // added, but check for it just in case.
  if (SdioFnIdxValid(fn_idx) && fn_idx != 0) {
    std::lock_guard<std::mutex> lock(lock_);
    interrupt_enabled_mask_ |= 1 << fn_idx;
    sdmmc_->AckInBandInterrupt();
  }
}

void SdioControllerDevice::Callback(fdf::Arena& arena, CallbackCompleter::Sync& completer) {
  const zx::time_boot irq_time = zx::clock::get_boot();

  if (shutdown_) {
    completer.buffer(arena).Reply();
    return;
  }

  uint8_t intr_byte;
  {
    std::lock_guard<std::mutex> lock(lock_);

    zx_status_t st = SdioDoRwByteLocked(false, 0, SDIO_CIA_CCCR_INTx_INTR_PEN_ADDR, 0, &intr_byte);
    if (st != ZX_OK) {
      fdf::error("Failed reading intr pending reg. status: {}", zx_status_get_string(st));
      completer.buffer(arena).Reply();
      return;
    }

    // Only trigger interrupts for functions that have ack'd the previous interrupt. Clear the
    // enabled bits for these functions.
    intr_byte &= interrupt_enabled_mask_;
    interrupt_enabled_mask_ &= ~intr_byte;
  }

  for (uint8_t i = 1; SdioFnIdxValid(i); i++) {
    if (intr_byte & (1 << i)) {
      sdio_irqs_[i].trigger(0, irq_time);
    }
  }

  completer.buffer(arena).Reply();
}

void SdioControllerDevice::FunctionPowerOn(uint8_t fn_idx) {
  if (!SdioFnIdxValid(fn_idx)) {
    return;
  }

  const std::bitset old_function_power_on = function_power_on_;
  function_power_on_.set(fn_idx);
  function_power_on_.set(0);  // Function 0 is always on if at least one other function is on.
  if (old_function_power_on.none() && vccq_off_with_controller_off_) {
    // The controller driver has already transitioned from OFF to ON, now it's our turn. If the
    // controller driver has not powered down the chip then there is nothing to do here.
    PowerOnReset();
  }

  // TODO(421962648): Unmask in-band interrupts, if needed.
}

void SdioControllerDevice::FunctionPowerOff(uint8_t fn_idx) {
  if (!SdioFnIdxValid(fn_idx)) {
    return;
  }

  // All requests are handled synchronously on the default dispatcher, so we don't need to wait for
  // anything to complete before letting the controller driver power down.
  function_power_on_.reset(fn_idx);
  // Clear the bit for function 0 if the last I/O function just powered down.
  if (function_power_on_ == 1) {
    function_power_on_.reset();
  }
}

zx_status_t SdioControllerDevice::SdioIoAbort(uint8_t fn_idx) {
  if (!SdioFnIdxValid(fn_idx) || fn_idx == 0) {
    return ZX_ERR_INVALID_ARGS;
  }

  return SdioDoRwByte(true, 0, SDIO_CIA_CCCR_ASx_ABORT_SEL_CR_ADDR, fn_idx, nullptr);
}

zx_status_t SdioControllerDevice::SdioIntrPending(uint8_t fn_idx, bool* out_pending) {
  if (!SdioFnIdxValid(fn_idx) || fn_idx == 0) {
    return ZX_ERR_INVALID_ARGS;
  }

  uint8_t intr_byte;
  zx_status_t st = SdioDoRwByte(false, 0, SDIO_CIA_CCCR_INTx_INTR_PEN_ADDR, 0, &intr_byte);
  if (st != ZX_OK) {
    fdf::error("Failed reading intr pending reg. status: {}", zx_status_get_string(st));
    return st;
  }

  *out_pending = intr_byte & (1 << fn_idx);
  return ZX_OK;
}

zx_status_t SdioControllerDevice::SdioDoVendorControlRwByte(bool write, uint8_t addr,
                                                            uint8_t write_byte,
                                                            uint8_t* out_read_byte) {
  // The vendor area of the CCCR is 0xf0 - 0xff.
  if (addr < kCccrVendorAddressMin) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  return SdioDoRwByte(write, 0, addr, write_byte, out_read_byte);
}

zx_status_t SdioControllerDevice::SdioRegisterVmo(uint8_t fn_idx, uint32_t vmo_id, zx::vmo vmo,
                                                  uint64_t offset, uint64_t size,
                                                  uint32_t vmo_rights) {
  if (!SdioFnIdxValid(fn_idx) || fn_idx == 0) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (shutdown_) {
    return ZX_ERR_CANCELED;
  }

  std::lock_guard<std::mutex> lock(lock_);
  return sdmmc_->RegisterVmo(vmo_id, fn_idx, std::move(vmo), offset, size, vmo_rights);
}

zx_status_t SdioControllerDevice::SdioUnregisterVmo(uint8_t fn_idx, uint32_t vmo_id,
                                                    zx::vmo* out_vmo) {
  if (!SdioFnIdxValid(fn_idx) || fn_idx == 0) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (shutdown_) {
    return ZX_ERR_CANCELED;
  }

  std::lock_guard<std::mutex> lock(lock_);
  return sdmmc_->UnregisterVmo(vmo_id, fn_idx, out_vmo);
}

zx_status_t SdioControllerDevice::SdioRequestCardReset() {
  if (function_power_on_.none()) {
    return ZX_ERR_BAD_STATE;
  }

  sdmmc_->HwReset();

  zx_status_t status = PowerOnReset();
  if (status == ZX_OK) {
    fdf::info("Reset card successfully");
  } else {
    fdf::error("Card reset failed: {}", zx_status_get_string(status));
  }

  return status;
}

zx_status_t SdioControllerDevice::SdioPerformTuning() {
  if (function_power_on_.none()) {
    return ZX_ERR_BAD_STATE;
  }
  if (shutdown_) {
    return ZX_ERR_CANCELED;
  }

  if (!tuned_) {
    // Tuning was not performed during initialization, so there is no need to do it now.
    return ZX_OK;
  }

  if (tuning_in_progress_.exchange(true)) {
    return ZX_ERR_ALREADY_BOUND;
  }

  zx_status_t status = sdmmc_->PerformTuning(SD_SEND_TUNING_BLOCK);
  tuning_in_progress_.store(false);
  return status;
}

zx::result<uint8_t> SdioControllerDevice::ReadCccrByte(uint32_t addr) {
  uint8_t byte = 0;
  zx_status_t status = SdioDoRwByteLocked(false, 0, addr, 0, &byte);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(byte);
}

zx_status_t SdioControllerDevice::PowerOnReset() {
  if (shutdown_) {
    return ZX_ERR_CANCELED;
  }

  std::lock_guard<std::mutex> lock(lock_);

  tuned_ = false;
  funcs_ = {};
  hw_info_ = {};

  // The device was turned off and is now back in the idle state. Change the bus settings to match
  // the device state so they stay in sync.
  if (zx_status_t status = sdmmc_->SetBusWidth(SDMMC_BUS_WIDTH_ONE); status != ZX_OK) {
    return status;
  }
  if (zx_status_t status = sdmmc_->SetTiming(SDMMC_TIMING_LEGACY); status != ZX_OK) {
    return status;
  }
  if (zx_status_t status = sdmmc_->SetBusFreq(kInitializationFrequencyHz); status != ZX_OK) {
    return status;
  }

  sdmmc_->ClearRca();
  return ProbeLocked();
}

// Use function overloads to convert the buffer depending on whether this is a Banjo or a FIDL call.
// We use Banjo for tracking buffer positions, so there is no conversion necessary in that case.

sdmmc_buffer_region_t GetBuffer(const fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion& buffer) {
  sdmmc_buffer_region_t out{};
  if (buffer.buffer.is_vmo_id()) {
    out.type = SDMMC_BUFFER_TYPE_VMO_ID;
    out.buffer.vmo_id = buffer.buffer.vmo_id();
  } else if (buffer.buffer.is_vmo()) {
    out.type = SDMMC_BUFFER_TYPE_VMO_HANDLE;
    out.buffer.vmo = buffer.buffer.vmo().get();
  } else {
    out.type = 0;
  }

  out.offset = buffer.offset;
  out.size = buffer.size;
  return out;
}

sdmmc_buffer_region_t GetBuffer(const sdmmc_buffer_region_t& buffer) { return buffer; }

template <typename T>
zx::result<SdioControllerDevice::SdioTxnPosition<T>> SdioControllerDevice::DoOneRwTxnRequest(
    uint8_t fn_idx, const SdioRwTxn<T>& txn, SdioTxnPosition<T> current_position) {
  const uint32_t func_blk_size = funcs_[fn_idx].cur_blk_size;
  const bool mbs = hw_info_.caps & SDIO_CARD_MULTI_BLOCK;
  const size_t max_transfer_size = func_blk_size * (mbs ? SDIO_IO_RW_EXTD_MAX_BLKS_PER_CMD : 1);

  size_t block_count = 0;  // The number of full blocks that are in the buffers processed so far.
  size_t total_size = 0;   // The total number of bytes that are in the buffers processed so far.
  size_t last_block_buffer_index = 0;  // The index of the last buffer to cross a block boundary.
  size_t last_block_buffer_size = 0;   // The offset where the new block starts in this buffer.

  sdmmc_buffer_region_t buffers[SDIO_IO_RW_EXTD_MAX_BLKS_PER_CMD];
  for (size_t i = 0; i < std::size(buffers) && i < current_position.buffers.size(); i++) {
    buffers[i] = GetBuffer(current_position.buffers[i]);
    if (buffers[i].type == 0) {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    if (i == 0) {
      ZX_ASSERT(current_position.first_buffer_offset < buffers[i].size);
      buffers[i].offset += current_position.first_buffer_offset;
      buffers[i].size -= current_position.first_buffer_offset;
    }

    // Trim the buffer to the max transfer size so that block boundaries can be checked.
    const size_t buffer_size = std::min(buffers[i].size, max_transfer_size - total_size);

    if ((total_size + buffer_size) / func_blk_size != block_count) {
      // This buffer crosses a block boundary, record the index and the offset at which the next
      // block begins.
      last_block_buffer_index = i;
      last_block_buffer_size = buffer_size - ((total_size + buffer_size) % func_blk_size);
      block_count = (total_size + buffer_size) / func_blk_size;
    }

    total_size += buffer_size;

    ZX_ASSERT(total_size <= max_transfer_size);
    if (total_size == max_transfer_size) {
      break;
    }
  }

  zx_status_t status;
  uint32_t txn_size = 0;
  if (block_count == 0) {
    // The collection of buffers didn't make up a full block.
    txn_size = static_cast<uint32_t>(total_size);

    // We know the entire buffers list is being used because the max transfer size is always at
    // least the block size. The first buffer may have had the size adjusted, so use the local
    // buffers array.
    const size_t num_buffers = std::min(current_position.buffers.size(), std::size(buffers));
    cpp20::span txn_buffers(buffers, num_buffers);
    status = sdmmc_->SdioIoRwExtended(hw_info_.caps, txn.write, fn_idx, current_position.address,
                                      txn.incr, 1, static_cast<uint32_t>(total_size), txn_buffers);
    last_block_buffer_index = num_buffers;
  } else {
    txn_size = static_cast<uint32_t>(block_count * func_blk_size);

    cpp20::span txn_buffers(buffers, last_block_buffer_index + 1);
    txn_buffers[last_block_buffer_index].size = last_block_buffer_size;
    status = sdmmc_->SdioIoRwExtended(hw_info_.caps, txn.write, fn_idx, current_position.address,
                                      txn.incr, static_cast<uint32_t>(block_count), func_blk_size,
                                      txn_buffers);

    if (last_block_buffer_index == 0) {
      last_block_buffer_size += current_position.first_buffer_offset;
    }

    ZX_ASSERT(last_block_buffer_size <= current_position.buffers[last_block_buffer_index].size);

    if (current_position.buffers[last_block_buffer_index].size == last_block_buffer_size) {
      last_block_buffer_index++;
      last_block_buffer_size = 0;
    }
  }

  if (status != ZX_OK) {
    (txn.write ? tx_errors_ : rx_errors_).Add(1);
    return zx::error(status);
  }

  return zx::ok(SdioTxnPosition<T>{
      .buffers = current_position.buffers.subspan(last_block_buffer_index),
      .first_buffer_offset = last_block_buffer_size,
      .address = current_position.address + (txn.incr ? txn_size : 0),
  });
}

// Explicit instantiation to ensure both methods are available to SdioFunctionDevice.
template zx_status_t SdioControllerDevice::SdioDoRwTxn<>(uint8_t,
                                                         const SdioRwTxn<sdmmc_buffer_region_t>&);
template zx_status_t SdioControllerDevice::SdioDoRwTxn<>(
    uint8_t, const SdioRwTxn<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>&);

template <typename T>
zx_status_t SdioControllerDevice::SdioDoRwTxn(uint8_t fn_idx, const SdioRwTxn<T>& txn) {
  if (!SdioFnIdxValid(fn_idx)) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (!function_power_on_[fn_idx]) {
    return ZX_ERR_BAD_STATE;
  }
  if (shutdown_) {
    return ZX_ERR_CANCELED;
  }

  std::lock_guard<std::mutex> lock(lock_);
  SdioTxnPosition<T> current_position = {
      .buffers = txn.buffers,
      .first_buffer_offset = 0,
      .address = txn.addr,
  };

  while (!current_position.buffers.empty()) {
    zx::result<SdioTxnPosition<T>> status = DoOneRwTxnRequest<T>(fn_idx, txn, current_position);
    if (status.is_error()) {
      return status.error_value();
    }
    current_position = status.value();
  }

  return ZX_OK;
}

zx_status_t SdioControllerDevice::SdioReset() {
  zx_status_t st = ZX_OK;
  uint8_t abort_byte;

  st = SdioDoRwByteLocked(false, 0, SDIO_CIA_CCCR_ASx_ABORT_SEL_CR_ADDR, 0, &abort_byte);
  if (st != ZX_OK) {
    abort_byte = SDIO_CIA_CCCR_ASx_ABORT_SOFT_RESET;
  } else {
    abort_byte |= SDIO_CIA_CCCR_ASx_ABORT_SOFT_RESET;
  }
  return SdioDoRwByteLocked(true, 0, SDIO_CIA_CCCR_ASx_ABORT_SEL_CR_ADDR, abort_byte, nullptr);
}

zx_status_t SdioControllerDevice::ProcessCccr() {
  uint8_t cccr_vsn, sdio_vsn, vsn_info, bus_speed, card_caps, uhs_caps, drv_strength;

  // version info
  zx_status_t status = SdioDoRwByteLocked(false, 0, SDIO_CIA_CCCR_CCCR_SDIO_VER_ADDR, 0, &vsn_info);
  if (status != ZX_OK) {
    fdf::error("Error reading CCCR reg: {}", zx_status_get_string(status));
    return status;
  }
  cccr_vsn = GetBits(vsn_info, SDIO_CIA_CCCR_CCCR_VER_MASK, SDIO_CIA_CCCR_CCCR_VER_LOC);
  sdio_vsn = GetBits(vsn_info, SDIO_CIA_CCCR_SDIO_VER_MASK, SDIO_CIA_CCCR_SDIO_VER_LOC);
  if ((cccr_vsn < SDIO_CCCR_FORMAT_VER_3) || (sdio_vsn < SDIO_SDIO_VER_3)) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  hw_info_.cccr_vsn = cccr_vsn;
  hw_info_.sdio_vsn = sdio_vsn;

  // card capabilities
  status = SdioDoRwByteLocked(false, 0, SDIO_CIA_CCCR_CARD_CAPS_ADDR, 0, &card_caps);
  if (status != ZX_OK) {
    fdf::error("Error reading CAPS reg: {}", zx_status_get_string(status));
    return status;
  }
  hw_info_.caps = 0;
  if (card_caps & SDIO_CIA_CCCR_CARD_CAP_SMB) {
    hw_info_.caps |= SDIO_CARD_MULTI_BLOCK;
  }
  if (card_caps & SDIO_CIA_CCCR_CARD_CAP_LSC) {
    hw_info_.caps |= SDIO_CARD_LOW_SPEED;
  }
  if (card_caps & SDIO_CIA_CCCR_CARD_CAP_4BLS) {
    hw_info_.caps |= SDIO_CARD_FOUR_BIT_BUS;
  }

  // speed
  status = SdioDoRwByteLocked(false, 0, SDIO_CIA_CCCR_BUS_SPEED_SEL_ADDR, 0, &bus_speed);
  if (status != ZX_OK) {
    fdf::error("Error reading SPEED reg: {}", zx_status_get_string(status));
    return status;
  }
  if (bus_speed & SDIO_CIA_CCCR_BUS_SPEED_SEL_SHS) {
    hw_info_.caps |= SDIO_CARD_HIGH_SPEED;
  }

  // Is UHS supported?
  status = SdioDoRwByteLocked(false, 0, SDIO_CIA_CCCR_UHS_SUPPORT_ADDR, 0, &uhs_caps);
  if (status != ZX_OK) {
    fdf::error("Error reading SPEED reg: {}", zx_status_get_string(status));
    return status;
  }
  if (uhs_caps & SDIO_CIA_CCCR_UHS_SDR50) {
    hw_info_.caps |= SDIO_CARD_UHS_SDR50;
  }
  if (uhs_caps & SDIO_CIA_CCCR_UHS_SDR104) {
    hw_info_.caps |= SDIO_CARD_UHS_SDR104;
  }
  if (uhs_caps & SDIO_CIA_CCCR_UHS_DDR50) {
    hw_info_.caps |= SDIO_CARD_UHS_DDR50;
  }

  // drv_strength
  status = SdioDoRwByteLocked(false, 0, SDIO_CIA_CCCR_DRV_STRENGTH_ADDR, 0, &drv_strength);
  if (status != ZX_OK) {
    fdf::error("Error reading SPEED reg: {}", zx_status_get_string(status));
    return status;
  }
  if (drv_strength & SDIO_CIA_CCCR_DRV_STRENGTH_SDTA) {
    hw_info_.caps |= SDIO_CARD_TYPE_A;
  }
  if (drv_strength & SDIO_CIA_CCCR_DRV_STRENGTH_SDTB) {
    hw_info_.caps |= SDIO_CARD_TYPE_B;
  }
  if (drv_strength & SDIO_CIA_CCCR_DRV_STRENGTH_SDTD) {
    hw_info_.caps |= SDIO_CARD_TYPE_D;
  }
  return status;
}

zx_status_t SdioControllerDevice::ProcessCis(uint8_t fn_idx) {
  zx_status_t st = ZX_OK;

  if (fn_idx >= SDIO_MAX_FUNCS) {
    return ZX_ERR_INVALID_ARGS;
  }
  uint32_t cis_ptr = 0;
  for (size_t i = 0; i < SDIO_CIS_ADDRESS_SIZE; i++) {
    uint8_t addr;
    st = SdioDoRwByteLocked(
        false, 0, static_cast<uint32_t>(SDIO_CIA_FBR_BASE_ADDR(fn_idx) + SDIO_CIA_FBR_CIS_ADDR + i),
        0, &addr);
    if (st != ZX_OK) {
      fdf::error("Error reading CIS of CCCR reg: {}", zx_status_get_string(st));
      return st;
    }
    cis_ptr |= addr << (i * 8);
  }
  if (!cis_ptr) {
    fdf::error("CIS address is invalid");
    return ZX_ERR_IO;
  }

  while (true) {
    uint8_t tuple_code, tuple_link;
    SdioFuncTuple cur_tup;
    st = SdioDoRwByteLocked(false, 0, cis_ptr + SDIO_CIS_TPL_FRMT_TCODE_OFF, 0, &tuple_code);
    if (st != ZX_OK) {
      fdf::error("Error reading tuple code for fn {}", fn_idx);
      break;
    }
    // Ignore null tuples
    if (tuple_code == SDIO_CIS_TPL_CODE_NULL) {
      cis_ptr++;
      continue;
    }
    if (tuple_code == SDIO_CIS_TPL_CODE_END) {
      break;
    }
    st = SdioDoRwByteLocked(false, 0, cis_ptr + SDIO_CIS_TPL_FRMT_TLINK_OFF, 0, &tuple_link);
    if (st != ZX_OK) {
      fdf::error("Error reading tuple size for fn {}", fn_idx);
      break;
    }
    if (tuple_link == SDIO_CIS_TPL_LINK_END) {
      break;
    }

    cur_tup.tuple_code = tuple_code;
    cur_tup.tuple_body_size = tuple_link;

    cis_ptr += SDIO_CIS_TPL_FRMT_TBODY_OFF;
    for (size_t i = 0; i < tuple_link; i++, cis_ptr++) {
      st = SdioDoRwByteLocked(false, 0, cis_ptr, 0, &cur_tup.tuple_body[i]);
      if (st != ZX_OK) {
        fdf::error("Error reading tuple body for fn {}", fn_idx);
        return st;
      }
    }

    if ((st = ParseFnTuple(fn_idx, cur_tup)) != ZX_OK) {
      break;
    }
  }
  return st;
}

zx_status_t SdioControllerDevice::ParseFnTuple(uint8_t fn_idx, const SdioFuncTuple& tup) {
  zx_status_t st = ZX_OK;
  switch (tup.tuple_code) {
    case SDIO_CIS_TPL_CODE_MANFID:
      st = ParseMfidTuple(fn_idx, tup);
      break;
    case SDIO_CIS_TPL_CODE_FUNCE:
      st = ParseFuncExtTuple(fn_idx, tup);
      break;
    default:
      break;
  }
  return st;
}

zx_status_t SdioControllerDevice::ParseFuncExtTuple(uint8_t fn_idx, const SdioFuncTuple& tup) {
  SdioFunction* func = &funcs_[fn_idx];
  if (fn_idx == 0) {
    if (tup.tuple_body_size < SDIO_CIS_TPL_FUNC0_FUNCE_MIN_BDY_SZ) {
      return ZX_ERR_IO;
    }
    func->hw_info.max_blk_size =
        SdioReadTupleBody(tup.tuple_body, SDIO_CIS_TPL_FUNCE_FUNC0_MAX_BLK_SIZE_LOC, 2);
    func->hw_info.max_blk_size = static_cast<uint32_t>(
        std::min<uint64_t>(sdmmc_->host_info().max_transfer_size, func->hw_info.max_blk_size));

    if (func->hw_info.max_blk_size == 0) {
      fdf::error("Invalid max block size for function 0");
      return ZX_ERR_IO_INVALID;
    }

    uint8_t speed_val = GetBitsU8(tup.tuple_body[3], SDIO_CIS_TPL_FUNCE_MAX_TRAN_SPEED_VAL_MASK,
                                  SDIO_CIS_TPL_FUNCE_MAX_TRAN_SPEED_VAL_LOC);
    uint8_t speed_unit = GetBitsU8(tup.tuple_body[3], SDIO_CIS_TPL_FUNCE_MAX_TRAN_SPEED_UNIT_MASK,
                                   SDIO_CIS_TPL_FUNCE_MAX_TRAN_SPEED_UNIT_LOC);
    // MAX_TRAN_SPEED is set in the function 0 CIS tuple but applies to all functions on the card.
    hw_info_.max_tran_speed = sdio_cis_tpl_funce_tran_speed_val[speed_val] *
                              sdio_cis_tpl_funce_tran_speed_unit[speed_unit];
    return ZX_OK;
  }

  if (tup.tuple_body_size < SDIO_CIS_TPL_FUNCx_FUNCE_MIN_BDY_SZ) {
    fdf::error("Invalid body size: {} for func_ext tuple", tup.tuple_body_size);
    return ZX_ERR_IO;
  }

  func->hw_info.max_blk_size =
      SdioReadTupleBody(tup.tuple_body, SDIO_CIS_TPL_FUNCE_FUNCx_MAX_BLK_SIZE_LOC, 2);
  if (func->hw_info.max_blk_size == 0) {
    fdf::error("Invalid max block size for function {}", fn_idx);
    return ZX_ERR_IO_INVALID;
  }

  return ZX_OK;
}

zx_status_t SdioControllerDevice::ParseMfidTuple(uint8_t fn_idx, const SdioFuncTuple& tup) {
  if (tup.tuple_body_size < SDIO_CIS_TPL_MANFID_MIN_BDY_SZ) {
    return ZX_ERR_IO;
  }
  SdioFunction* func = &funcs_[fn_idx];
  func->hw_info.manufacturer_id = SdioReadTupleBody(tup.tuple_body, 0, 2);
  func->hw_info.product_id = SdioReadTupleBody(tup.tuple_body, 2, 2);
  return ZX_OK;
}

zx_status_t SdioControllerDevice::ProcessFbr(uint8_t fn_idx) {
  zx_status_t st = ZX_OK;
  uint8_t fbr, fn_intf_code;

  SdioFunction* func = &funcs_[fn_idx];
  if ((st = SdioDoRwByteLocked(
           false, 0, SDIO_CIA_FBR_BASE_ADDR(fn_idx) + SDIO_CIA_FBR_STD_IF_CODE_ADDR, 0, &fbr)) !=
      ZX_OK) {
    fdf::error("Error reading intf code: {}", zx_status_get_string(st));
    return st;
  }
  fn_intf_code = GetBitsU8(fbr, SDIO_CIA_FBR_STD_IF_CODE_MASK, SDIO_CIA_FBR_STD_IF_CODE_LOC);
  if (fn_intf_code == SDIO_CIA_FBR_STD_IF_CODE_MASK) {
    // fn_code > 0Eh
    if ((st = SdioDoRwByteLocked(false, 0,
                                 SDIO_CIA_FBR_BASE_ADDR(fn_idx) + SDIO_CIA_FBR_STD_IF_CODE_EXT_ADDR,
                                 0, &fn_intf_code)) != ZX_OK) {
      fdf::error("Error while reading the extended intf code {}", zx_status_get_string(st));
      return st;
    }
  }
  func->hw_info.fn_intf_code = fn_intf_code;
  return ZX_OK;
}

zx_status_t SdioControllerDevice::InitFunc(uint8_t fn_idx) {
  zx_status_t st = ZX_OK;

  if ((st = ProcessFbr(fn_idx)) != ZX_OK) {
    return st;
  }

  if ((st = ProcessCis(fn_idx)) != ZX_OK) {
    return st;
  }

  // Enable all func for now. Should move to wifi driver ?
  if ((st = SdioEnableFnLocked(fn_idx)) != ZX_OK) {
    return st;
  }

  // Set default block size
  if ((st = SdioUpdateBlockSizeLocked(fn_idx, 0, true)) != ZX_OK) {
    return st;
  }

  return st;
}

zx_status_t SdioControllerDevice::SwitchFreq(uint32_t new_freq) {
  zx_status_t st;
  if ((st = sdmmc_->SetBusFreq(new_freq)) != ZX_OK) {
    fdf::error("Error while switching host bus frequency, retcode = {}", zx_status_get_string(st));
    return st;
  }
  return ZX_OK;
}

zx_status_t SdioControllerDevice::TrySwitchHs() {
  zx_status_t st = ZX_OK;
  uint8_t speed = 0;

  if (!(hw_info_.caps & SDIO_CARD_HIGH_SPEED)) {
    fdf::error("High speed not supported, retcode = {}", zx_status_get_string(st));
    return ZX_ERR_NOT_SUPPORTED;
  }
  st = SdioDoRwByteLocked(false, 0, SDIO_CIA_CCCR_BUS_SPEED_SEL_ADDR, 0, &speed);
  if (st != ZX_OK) {
    fdf::error("Error while reading CCCR reg, retcode = {}", zx_status_get_string(st));
    return st;
  }
  UpdateBitsU8(&speed, SDIO_CIA_CCCR_BUS_SPEED_BSS_MASK, SDIO_CIA_CCCR_BUS_SPEED_BSS_LOC,
               SDIO_BUS_SPEED_EN_HS);
  st = SdioDoRwByteLocked(true, 0, SDIO_CIA_CCCR_BUS_SPEED_SEL_ADDR, speed, nullptr);
  if (st != ZX_OK) {
    fdf::error("Error while writing to CCCR reg, retcode = {}", zx_status_get_string(st));
    return st;
  }
  // Switch the host timing
  if ((st = sdmmc_->SetTiming(SDMMC_TIMING_HS)) != ZX_OK) {
    fdf::error("failed to switch to hs timing on host : {}", zx_status_get_string(st));
    return st;
  }

  if ((st = SwitchFreq(SDIO_HS_MAX_FREQ)) != ZX_OK) {
    fdf::error("failed to switch to hs frequency on host : {}", zx_status_get_string(st));
    return st;
  }

  if ((st = SwitchBusWidth(SDIO_BW_4BIT)) != ZX_OK) {
    fdf::error("Switching to 4-bit bus width failed, retcode = {}", zx_status_get_string(st));
    return st;
  }
  return ZX_OK;
}

zx_status_t SdioControllerDevice::TrySwitchUhs() {
  zx_status_t st = ZX_OK;
  if ((st = SwitchBusWidth(SDIO_BW_4BIT)) != ZX_OK) {
    fdf::error("Switching to 4-bit bus width failed, retcode = {}", zx_status_get_string(st));
    return st;
  }

  uint8_t speed = 0;

  uint32_t new_freq = SDIO_DEFAULT_FREQ;
  uint8_t select_speed = SDIO_BUS_SPEED_SDR50;
  sdmmc_timing_t timing = SDMMC_TIMING_SDR50;

  st = SdioDoRwByteLocked(false, 0, SDIO_CIA_CCCR_BUS_SPEED_SEL_ADDR, 0, &speed);
  if (st != ZX_OK) {
    fdf::error("Error while reading CCCR reg, retcode = {}", zx_status_get_string(st));
    return st;
  }

  if ((sdmmc_->host_info().caps & SDMMC_HOST_CAP_SDR104) &&
      (hw_info_.caps & SDIO_CARD_UHS_SDR104)) {
    select_speed = SDIO_BUS_SPEED_SDR104;
    timing = SDMMC_TIMING_SDR104;
    new_freq = SDIO_UHS_SDR104_MAX_FREQ;
  } else if ((sdmmc_->host_info().caps & SDMMC_HOST_CAP_SDR50) &&
             (hw_info_.caps & SDIO_CARD_UHS_SDR50)) {
    select_speed = SDIO_BUS_SPEED_SDR50;
    timing = SDMMC_TIMING_SDR50;
    new_freq = SDIO_UHS_SDR50_MAX_FREQ;
  } else if ((sdmmc_->host_info().caps & SDMMC_HOST_CAP_DDR50) &&
             (hw_info_.caps & SDIO_CARD_UHS_DDR50)) {
    select_speed = SDIO_BUS_SPEED_DDR50;
    timing = SDMMC_TIMING_DDR50;
    new_freq = SDIO_UHS_DDR50_MAX_FREQ;
  } else {
    select_speed = SDIO_BUS_SPEED_SDR25;
    timing = SDMMC_TIMING_SDR25;
    new_freq = SDIO_UHS_SDR25_MAX_FREQ;
  }

  UpdateBitsU8(&speed, SDIO_CIA_CCCR_BUS_SPEED_BSS_MASK, SDIO_CIA_CCCR_BUS_SPEED_BSS_LOC,
               select_speed);

  st = SdioDoRwByteLocked(true, 0, SDIO_CIA_CCCR_BUS_SPEED_SEL_ADDR, speed, nullptr);
  if (st != ZX_OK) {
    fdf::error("Error while writing to CCCR reg, retcode = {}", zx_status_get_string(st));
    return st;
  }
  // Switch the host timing
  if ((st = sdmmc_->SetTiming(timing)) != ZX_OK) {
    fdf::error("failed to switch to uhs timing on host : {}", zx_status_get_string(st));
    return st;
  }

  if ((st = SwitchFreq(new_freq)) != ZX_OK) {
    fdf::error("failed to switch to uhs frequency on host : {}", zx_status_get_string(st));
    return st;
  }

  // Only tune for SDR50 if the host requires it.
  if (timing == SDMMC_TIMING_SDR104 ||
      (timing == SDMMC_TIMING_SDR50 &&
       !(sdmmc_->host_info().caps & SDMMC_HOST_CAP_NO_TUNING_SDR50))) {
    st = sdmmc_->PerformTuning(SD_SEND_TUNING_BLOCK);
    if (st != ZX_OK) {
      fdf::error("tuning failed {}", zx_status_get_string(st));
      return st;
    }
    tuned_ = true;
  }
  return ZX_OK;
}

zx_status_t SdioControllerDevice::Enable4BitBus() {
  zx_status_t st = ZX_OK;
  if ((hw_info_.caps & SDIO_CARD_LOW_SPEED) && !(hw_info_.caps & SDIO_CARD_FOUR_BIT_BUS)) {
    fdf::error("Switching to 4-bit bus unsupported");
    return ZX_ERR_NOT_SUPPORTED;
  }
  uint8_t bus_ctrl_reg;
  if ((st = SdioDoRwByteLocked(false, 0, SDIO_CIA_CCCR_BUS_INTF_CTRL_ADDR, 0, &bus_ctrl_reg)) !=
      ZX_OK) {
    fdf::error("Error reading the current bus width");
    return st;
  }
  UpdateBitsU8(&bus_ctrl_reg, SDIO_CIA_CCCR_INTF_CTRL_BW_MASK, SDIO_CIA_CCCR_INTF_CTRL_BW_LOC,
               SDIO_BW_4BIT);
  if ((st = SdioDoRwByteLocked(true, 0, SDIO_CIA_CCCR_BUS_INTF_CTRL_ADDR, bus_ctrl_reg, nullptr)) !=
      ZX_OK) {
    fdf::error("Error while switching the bus width");
    return st;
  }
  if ((st = sdmmc_->SetBusWidth(SDMMC_BUS_WIDTH_FOUR)) != ZX_OK) {
    fdf::error("failed to switch the host bus width to {}, retcode = {}",
               static_cast<int>(SDMMC_BUS_WIDTH_FOUR), zx_status_get_string(st));
    return ZX_ERR_INTERNAL;
  }

  return ZX_OK;
}

zx_status_t SdioControllerDevice::SwitchBusWidth(uint32_t bw) {
  zx_status_t st = ZX_OK;
  if (bw != SDIO_BW_1BIT && bw != SDIO_BW_4BIT) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  if (bw == SDIO_BW_4BIT) {
    if ((st = Enable4BitBus()) != ZX_OK) {
      return st;
    }
  }
  return ZX_OK;
}

zx_status_t SdioControllerDevice::ReadData16(uint8_t fn_idx, uint32_t addr, uint16_t* word) {
  uint8_t byte1 = 0, byte2 = 0;
  zx_status_t st = SdioDoRwByteLocked(false, 0, addr, 0, &byte1);
  if (st != ZX_OK) {
    fdf::error("Error reading from addr:0x{:x}, retcode: {}", addr, zx_status_get_string(st));
    return st;
  }

  st = SdioDoRwByteLocked(false, 0, addr + 1, 0, &byte2);
  if (st != ZX_OK) {
    fdf::error("Error reading from addr:0x{:x}, retcode: {}", addr + 1, zx_status_get_string(st));
    return st;
  }

  *word = static_cast<uint16_t>(byte2 << 8 | byte1);
  return ZX_OK;
}

zx_status_t SdioControllerDevice::WriteData16(uint8_t fn_idx, uint32_t addr, uint16_t word) {
  zx_status_t st = SdioDoRwByteLocked(true, 0, addr, static_cast<uint8_t>(word & 0xff), nullptr);
  if (st != ZX_OK) {
    fdf::error("Error writing to addr:0x{:x}, retcode: {}", addr, zx_status_get_string(st));
    return st;
  }

  st = SdioDoRwByteLocked(true, 0, addr + 1, static_cast<uint8_t>((word >> 8) & 0xff), nullptr);
  if (st != ZX_OK) {
    fdf::error("Error writing to addr:0x{:x}, retcode: {}", addr + 1, zx_status_get_string(st));
    return st;
  }

  return ZX_OK;
}

fdf::Logger& SdioControllerDevice::logger() { return parent_->logger(); }

}  // namespace sdmmc
