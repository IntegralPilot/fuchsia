// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdmmc-device.h"

#include <endian.h>
#include <fuchsia/hardware/sdio/c/banjo.h>
#include <fuchsia/hardware/sdmmc/c/banjo.h>
#include <inttypes.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/sdio/hw.h>
#include <lib/zx/time.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <pretty/hexdump.h>

#include "sdmmc-root-device.h"

namespace {

constexpr zx::duration kVoltageStabilizationTime = zx::msec(5);
constexpr zx::duration kDataStabilizationTime = zx::msec(1);

constexpr uint32_t GetBits(uint32_t x, uint32_t mask, uint32_t loc) { return (x & mask) >> loc; }

constexpr void UpdateBits(uint32_t* x, uint32_t mask, uint32_t loc, uint32_t val) {
  *x &= ~mask;
  *x |= ((val << loc) & mask);
}

// Translates a FIDL sdmmc request (fuchsia_hardware_sdmmc::wire::SdmmcReq) into a Banjo one
// (sdmmc_req_t). The supplied buffer_region_ptr is used to house the sdmmc_req_t.buffers vector
// (hence buffer_region_ptr must outlive the returned sdmmc_req_t).
zx::result<sdmmc_req_t> FidlToBanjoReq(const fuchsia_hardware_sdmmc::wire::SdmmcReq& wire_req,
                                       sdmmc_buffer_region_t* buffer_region_ptr) {
  const sdmmc_req_t banjo_req = {
      .cmd_idx = wire_req.cmd_idx,
      .cmd_flags = wire_req.cmd_flags,
      .arg = wire_req.arg,
      .blocksize = wire_req.blocksize,
      .suppress_error_messages = wire_req.suppress_error_messages,
      .client_id = wire_req.client_id,
      .buffers_list = buffer_region_ptr,
      .buffers_count = wire_req.buffers.size(),
  };

  for (size_t i = 0; i < wire_req.buffers.size(); i++) {
    if (wire_req.buffers[i].buffer.is_vmo_id()) {
      buffer_region_ptr->type = SDMMC_BUFFER_TYPE_VMO_ID;
      buffer_region_ptr->buffer.vmo_id = wire_req.buffers[i].buffer.vmo_id();
    } else if (wire_req.buffers[i].buffer.is_vmo()) {
      buffer_region_ptr->type = SDMMC_BUFFER_TYPE_VMO_HANDLE;
      buffer_region_ptr->buffer.vmo = wire_req.buffers[i].buffer.vmo().get();
    } else {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    buffer_region_ptr->offset = wire_req.buffers[i].offset;
    buffer_region_ptr->size = wire_req.buffers[i].size;

    buffer_region_ptr++;
  }
  return zx::ok(banjo_req);
}

// Translates a Banjo sdmmc request (sdmmc_req_t) into a FIDL one
// (fuchsia_hardware_sdmmc::wire::SdmmcReq).
zx::result<fuchsia_hardware_sdmmc::wire::SdmmcReq> BanjoToFidlReq(const sdmmc_req_t& banjo_req,
                                                                  fdf::Arena* arena) {
  fuchsia_hardware_sdmmc::wire::SdmmcReq wire_req;

  wire_req.cmd_idx = banjo_req.cmd_idx;
  wire_req.cmd_flags = banjo_req.cmd_flags;
  wire_req.arg = banjo_req.arg;
  wire_req.blocksize = banjo_req.blocksize;
  wire_req.suppress_error_messages = banjo_req.suppress_error_messages;
  wire_req.client_id = banjo_req.client_id;

  wire_req.buffers.Allocate(*arena, banjo_req.buffers_count);
  for (size_t i = 0; i < banjo_req.buffers_count; i++) {
    if (banjo_req.buffers_list[i].type == SDMMC_BUFFER_TYPE_VMO_ID) {
      wire_req.buffers[i].buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(
          banjo_req.buffers_list[i].buffer.vmo_id);
    } else {
      if (banjo_req.buffers_list[i].type != SDMMC_BUFFER_TYPE_VMO_HANDLE) {
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      zx::vmo dup;
      // TODO(b/300145353): Remove duplication when removing Banjo.
      zx_status_t status = zx_handle_duplicate(banjo_req.buffers_list[i].buffer.vmo,
                                               ZX_RIGHT_SAME_RIGHTS, dup.reset_and_get_address());
      if (status != ZX_OK) {
        fdf::error("Failed to duplicate vmo: {}", zx_status_get_string(status));
        return zx::error(status);
      }
      wire_req.buffers[i].buffer =
          fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo(std::move(dup));
    }

    wire_req.buffers[i].offset = banjo_req.buffers_list[i].offset;
    wire_req.buffers[i].size = banjo_req.buffers_list[i].size;
  }
  return zx::ok(std::move(wire_req));
}

// Translates a collection of Banjo sdmmc requests into a FIDL one.
zx::result<fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>> BanjoToFidlReqVector(
    const sdmmc_req_t* req, size_t banjo_req_count, fdf::Arena* arena) {
  fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq> wire_req_vector;
  wire_req_vector.Allocate(*arena, banjo_req_count);

  for (size_t i = 0; i < banjo_req_count; i++) {
    zx::result<fuchsia_hardware_sdmmc::wire::SdmmcReq> wire_req = BanjoToFidlReq(*req, arena);
    if (wire_req.is_error()) {
      return zx::error(wire_req.error_value());
    }
    wire_req_vector[i] = std::move(*wire_req);
    req++;
  }
  return zx::ok(wire_req_vector);
}

}  // namespace

namespace sdmmc {

zx_status_t SdmmcDevice::Init(bool use_fidl) {
  // Reset any previous initializations in a way that works with unit testing.
  using_fidl_ = use_fidl;

  if (root_device_ == nullptr) {
    return ZX_ERR_BAD_STATE;
  }

  if (use_fidl) {
    auto client_end =
        root_device_->driver_incoming()->Connect<fuchsia_hardware_sdmmc::SdmmcService::Sdmmc>();
    if (client_end.is_error() || !client_end->is_valid()) {
      fdf::error("Failed to connect to FIDL SDMMC protocol: {}", client_end.status_string());
      return client_end.status_value();
    }
    client_ = fdf::WireSharedClient(std::move(*client_end), fdf::Dispatcher::GetCurrent()->get());

    fdf::Arena arena('SDMC');
    auto result = client_.sync().buffer(arena)->HostInfo();
    if (!result.ok()) {
      fdf::error("Failed to get HostInfo using FIDL SDMMC protocol: {}", result.status_string());
      return result.status();
    }
  } else {
    auto host = compat::ConnectBanjo<ddk::SdmmcProtocolClient>(root_device_->driver_incoming());
    if (!host.is_ok()) {
      fdf::error("Failed to connect to Banjo SDMMC protocol: {}", host.status_string());
      return host.status_value();
    }
    host_ = *host;

    if (!host_.is_valid()) {
      fdf::error("Failed to get valid Banjo SDMMC protocol.");
      return ZX_ERR_NOT_SUPPORTED;
    }
  }

  zx_status_t status = HostInfo(&host_info_);
  if (status != ZX_OK) {
    fdf::error("failed to get host info: {}", zx_status_get_string(status));
    return status;
  }

  fdf::debug("host caps dma {} 8-bit bus {} max_transfer_size {}", UseDma() ? 1 : 0,
             (host_info().caps & SDMMC_HOST_CAP_BUS_WIDTH_8) ? 1 : 0,
             host_info().max_transfer_size);

  // Reset the card.
  HwReset();

  // No matter what state the card is in, issuing the GO_IDLE_STATE command will put the card into
  // the idle state.
  status = SdmmcGoIdle();
  if (status != ZX_OK) {
    fdf::error("SDMMC_GO_IDLE_STATE failed: {}", zx_status_get_string(status));
    return status;
  }
  return ZX_OK;
}

zx_status_t SdmmcDevice::Request(const sdmmc_req_t& req, uint32_t response[4], uint32_t retries,
                                 zx::duration wait_time) {
  if (retries == 0) {
    retries = retries_;
  }

  zx_status_t st;
  while (((st = Request(&req, response)) != ZX_OK) && retries > 0) {
    retries--;
    if (wait_time.get() > 0) {
      zx::nanosleep(zx::deadline_after(wait_time));
    }
  }
  return st;
}

zx_status_t SdmmcDevice::RequestWithBlockRead(const sdmmc_req_t& req, uint32_t response[4],
                                              cpp20::span<uint8_t> read_data) {
  zx::vmo vmo;
  zx_status_t st = zx::vmo::create(read_data.size(), 0, &vmo);
  if (st != ZX_OK) {
    return st;
  }

  const sdmmc_buffer_region_t vmo_region = {
      .buffer = {.vmo = vmo.get()},
      .type = SDMMC_BUFFER_TYPE_VMO_HANDLE,
      .offset = 0,
      .size = read_data.size(),
  };

  sdmmc_req_t request = req;
  request.buffers_list = &vmo_region;
  request.buffers_count = 1;
  if ((st = Request(request, response)) != ZX_OK) {
    return st;
  }

  return vmo.read(read_data.data(), 0, read_data.size());
}

// SD/MMC shared ops

zx_status_t SdmmcDevice::SdmmcGoIdle() {
  sdmmc_req_t req = {};
  req.cmd_idx = SDMMC_GO_IDLE_STATE;
  req.arg = 0;
  req.cmd_flags = SDMMC_GO_IDLE_STATE_FLAGS;
  uint32_t unused_response[4];
  return Request(req, unused_response);
}

zx_status_t SdmmcDevice::SdmmcSendStatus(uint32_t* status) {
  sdmmc_req_t req = {};
  req.cmd_idx = SDMMC_SEND_STATUS;
  req.arg = RcaArg();
  req.cmd_flags = SDMMC_SEND_STATUS_FLAGS;
  uint32_t response[4];
  zx_status_t st = Request(req, response);
  if (st == ZX_OK) {
    *status = response[0];
  }
  return st;
}

zx_status_t SdmmcDevice::SdmmcStopTransmission(uint32_t* status) {
  zx_status_t st;
  for (uint32_t i = 0; i < kTryAttempts; i++) {
    sdmmc_req_t req = {};
    req.cmd_idx = SDMMC_STOP_TRANSMISSION;
    req.arg = 0;
    req.cmd_flags = SDMMC_STOP_TRANSMISSION_FLAGS;
    req.suppress_error_messages = i < (kTryAttempts - 1);
    uint32_t response[4];
    if ((st = Request(req, response)) == ZX_OK) {
      if (status) {
        *status = response[0];
      }
      break;
    }
  }
  return st;
}

zx_status_t SdmmcDevice::SdmmcWaitForState(uint32_t desired_state) {
  uint32_t current_state = 0;
  for (uint32_t i = 0; i < kTryAttempts; i++) {
    sdmmc_req_t req = {};
    req.cmd_idx = SDMMC_SEND_STATUS;
    req.arg = RcaArg();
    req.cmd_flags = SDMMC_SEND_STATUS_FLAGS;
    req.suppress_error_messages = i < (kTryAttempts - 1);
    uint32_t response[4];
    zx_status_t st = Request(req, response);
    current_state = MMC_STATUS_CURRENT_STATE(response[0]);
    if (st == ZX_OK && current_state == desired_state) {
      return ZX_OK;
    }
  }
  fdf::error("Failed to wait for state {} (got state {}).", desired_state, current_state);
  return ZX_ERR_TIMED_OUT;
}

zx_status_t SdmmcDevice::SdmmcIoRequest(
    fdf::Arena arena, fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq> reqs,
    sdmmc_buffer_region_t* buffer_region_ptr) {
  zx_status_t status = ZX_OK;
  if (!using_fidl_) {
    for (const auto& req : reqs) {
      const zx::result<sdmmc_req_t> banjo_req = FidlToBanjoReq(req, buffer_region_ptr);
      if (banjo_req.is_error()) {
        return banjo_req.status_value();  // Not retrying if request can't be translated to Banjo.
      }
      uint32_t unused_response[4];
      status = host_.Request(&*banjo_req, unused_response);
      if (status != ZX_OK) {
        break;
      }
    }
  } else {
    auto result = client_.sync().buffer(arena)->Request(reqs);
    if (!result.ok()) {
      fdf::error("Failed to call Request: {}", result.status_string());
      return result.status();  // Not retrying if FIDL error.
    }
    if (result->is_error()) {
      status = result->error_value();
    }
  }

  if (status != ZX_OK) {
    SdmmcStopForRetry();
  }
  return status;
}

void SdmmcDevice::SdmmcStopForRetry() {
  // Wait for the card to go idle (TRAN state) before retrying. SdmmcStopTransmission waits for
  // the busy signal on dat0, so the card should be back in TRAN immediately after.

  uint32_t status;
  if (SdmmcStopTransmission(&status) == ZX_OK &&
      MMC_STATUS_CURRENT_STATE(status) == MMC_STATUS_CURRENT_STATE_TRAN) {
    return;
  }

  SdmmcWaitForState(MMC_STATUS_CURRENT_STATE_TRAN);
}

// SD ops

zx_status_t SdmmcDevice::SdSendAppCmd() {
  sdmmc_req_t req = {};
  req.cmd_idx = SDMMC_APP_CMD;
  req.arg = RcaArg();
  req.cmd_flags = SDMMC_APP_CMD_FLAGS;
  uint32_t unused_response[4];
  return Request(req, unused_response);
}

zx_status_t SdmmcDevice::SdSendOpCond(uint32_t flags, uint32_t* ocr) {
  zx_status_t st = SdSendAppCmd();
  if (st != ZX_OK) {
    return st;
  }

  sdmmc_req_t req = {};
  req.cmd_idx = SD_APP_SEND_OP_COND;
  req.arg = flags;
  req.cmd_flags = SD_APP_SEND_OP_COND_FLAGS;
  uint32_t response[4];
  if ((st = Request(req, response)) != ZX_OK) {
    return st;
  }

  *ocr = response[0];
  return ZX_OK;
}

zx_status_t SdmmcDevice::SdSendIfCond() {
  // TODO what is this parameter?
  uint32_t arg = 0x1aa;
  sdmmc_req_t req = {};
  req.cmd_idx = SD_SEND_IF_COND;
  req.arg = arg;
  req.cmd_flags = SD_SEND_IF_COND_FLAGS;
  req.suppress_error_messages = true;
  uint32_t response[4];
  zx_status_t st = Request(req, response);
  if (st != ZX_OK) {
    fdf::debug("SD_SEND_IF_COND failed, retcode = {}", st);
    return st;
  }
  if ((response[0] & 0xfff) != arg) {
    // The card should have replied with the pattern that we sent.
    fdf::debug("SDMMC_SEND_IF_COND got bad reply = {}", response[0]);
    return ZX_ERR_BAD_STATE;
  } else {
    return ZX_OK;
  }
}

zx_status_t SdmmcDevice::SdSendRelativeAddr(uint16_t* card_status) {
  sdmmc_req_t req = {};
  req.cmd_idx = SD_SEND_RELATIVE_ADDR;
  req.arg = 0;
  req.cmd_flags = SD_SEND_RELATIVE_ADDR_FLAGS;

  uint32_t response[4];
  zx_status_t st = Request(req, response);
  if (st != ZX_OK) {
    fdf::debug("SD_SEND_RELATIVE_ADDR failed, retcode = {}", st);
    return st;
  }

  rca_ = static_cast<uint16_t>(response[0] >> 16);

  if (card_status != nullptr) {
    *card_status = response[0] & 0xffff;
  }

  return st;
}

zx_status_t SdmmcDevice::SdSelectCard() {
  sdmmc_req_t req = {};
  req.cmd_idx = SD_SELECT_CARD;
  req.arg = RcaArg();
  req.cmd_flags = SD_SELECT_CARD_FLAGS;
  uint32_t unused_response[4];
  return Request(req, unused_response);
}

zx_status_t SdmmcDevice::SdSendScr(std::array<uint8_t, 8>& scr) {
  zx_status_t st = SdSendAppCmd();
  if (st != ZX_OK) {
    return st;
  }

  sdmmc_req_t req = {};
  req.cmd_idx = SD_APP_SEND_SCR;
  req.arg = 0;
  req.cmd_flags = SD_APP_SEND_SCR_FLAGS;
  req.blocksize = 8;
  uint32_t unused_response[4];
  return RequestWithBlockRead(req, unused_response, {scr.data(), scr.size()});
}

zx_status_t SdmmcDevice::SdSetBusWidth(sdmmc_bus_width_t width) {
  if (width != SDMMC_BUS_WIDTH_ONE && width != SDMMC_BUS_WIDTH_FOUR) {
    return ZX_ERR_INVALID_ARGS;
  }

  zx_status_t st = SdSendAppCmd();
  if (st != ZX_OK) {
    return st;
  }

  sdmmc_req_t req = {};
  req.cmd_idx = SD_APP_SET_BUS_WIDTH;
  req.arg = (width == SDMMC_BUS_WIDTH_FOUR ? 2 : 0);
  req.cmd_flags = SD_APP_SET_BUS_WIDTH_FLAGS;
  uint32_t unused_response[4];
  return Request(req, unused_response);
}

zx_status_t SdmmcDevice::SdSwitchUhsVoltage(uint32_t ocr) {
  zx_status_t st = ZX_OK;
  sdmmc_req_t req = {};
  req.cmd_idx = SD_VOLTAGE_SWITCH;
  req.arg = 0;
  req.cmd_flags = SD_VOLTAGE_SWITCH_FLAGS;

  if (signal_voltage_ == SDMMC_VOLTAGE_V180) {
    return ZX_OK;
  }

  uint32_t unused_response[4];
  if ((st = Request(req, unused_response)) != ZX_OK) {
    fdf::debug("SD_VOLTAGE_SWITCH failed, retcode = {}", st);
    return st;
  }

  if ((st = SetBusFreq(0)) != ZX_OK) {
    fdf::debug("SD_VOLTAGE_SWITCH failed, retcode = {}", st);
    return st;
  }

  if ((st = SetSignalVoltage(SDMMC_VOLTAGE_V180)) != ZX_OK) {
    fdf::debug("SD_VOLTAGE_SWITCH failed, retcode = {}", st);
    return st;
  }

  // Wait 5ms for the voltage to stabilize. See section 3.6.1. in the SDHCI specification.
  zx::nanosleep(zx::deadline_after(kVoltageStabilizationTime));

  if ((st = SetBusFreq(kInitializationFrequencyHz)) != ZX_OK) {
    fdf::debug("SD_VOLTAGE_SWITCH failed, retcode = {}", st);
    return st;
  }

  // Wait 1ms for the data lines to stabilize.
  zx::nanosleep(zx::deadline_after(kDataStabilizationTime));

  signal_voltage_ = SDMMC_VOLTAGE_V180;
  return ZX_OK;
}

// SDIO specific ops

zx_status_t SdmmcDevice::SdioSendOpCond(uint32_t ocr, uint32_t* rocr) {
  zx_status_t st = ZX_OK;
  sdmmc_req_t req = {};
  req.cmd_idx = SDIO_SEND_OP_COND;
  req.arg = ocr;
  req.cmd_flags = SDIO_SEND_OP_COND_FLAGS;
  req.suppress_error_messages = true;
  for (size_t i = 0; i < 100; i++) {
    uint32_t response[4];
    if ((st = Request(req, response, 3, zx::msec(10))) != ZX_OK) {
      // fail on request error
      break;
    }
    // No need to wait for busy clear if probing
    if ((ocr == 0) || (response[0] & MMC_OCR_BUSY)) {
      *rocr = response[0];
      break;
    }
    zx::nanosleep(zx::deadline_after(zx::msec(10)));
  }
  return st;
}

zx_status_t SdmmcDevice::SdioIoRwDirect(bool write, uint32_t fn_idx, uint32_t reg_addr,
                                        uint8_t write_byte, uint8_t* read_byte) {
  uint32_t cmd_arg = 0;
  if (write) {
    cmd_arg |= SDIO_IO_RW_DIRECT_RW_FLAG;
    if (read_byte) {
      cmd_arg |= SDIO_IO_RW_DIRECT_RAW_FLAG;
    }
  }
  UpdateBits(&cmd_arg, SDIO_IO_RW_DIRECT_FN_IDX_MASK, SDIO_IO_RW_DIRECT_FN_IDX_LOC, fn_idx);
  UpdateBits(&cmd_arg, SDIO_IO_RW_DIRECT_REG_ADDR_MASK, SDIO_IO_RW_DIRECT_REG_ADDR_LOC, reg_addr);
  UpdateBits(&cmd_arg, SDIO_IO_RW_DIRECT_WRITE_BYTE_MASK, SDIO_IO_RW_DIRECT_WRITE_BYTE_LOC,
             write_byte);
  sdmmc_req_t req = {};
  req.cmd_idx = SDIO_IO_RW_DIRECT;
  req.arg = cmd_arg;
  if (reg_addr == SDIO_CIA_CCCR_ASx_ABORT_SEL_CR_ADDR) {
    req.cmd_flags = SDIO_IO_RW_DIRECT_ABORT_FLAGS;
    req.suppress_error_messages = true;
  } else {
    req.cmd_flags = SDIO_IO_RW_DIRECT_FLAGS;
  }
  uint32_t response[4];
  zx_status_t st = Request(req, response);
  if (st != ZX_OK) {
    // Let the platform driver handle logging of this error.
    fdf::debug("SDIO_IO_RW_DIRECT failed, retcode = {}", st);
    return st;
  }
  if (read_byte) {
    *read_byte = static_cast<uint8_t>(GetBits(response[0], SDIO_IO_RW_DIRECT_RESP_READ_BYTE_MASK,
                                              SDIO_IO_RW_DIRECT_RESP_READ_BYTE_LOC));
  }
  return ZX_OK;
}

zx::result<uint8_t> SdmmcDevice::SdioIoRwDirect(uint32_t function, uint32_t address,
                                                bool suppress_error_messages) {
  uint32_t cmd_arg = 0;
  UpdateBits(&cmd_arg, SDIO_IO_RW_DIRECT_FN_IDX_MASK, SDIO_IO_RW_DIRECT_FN_IDX_LOC, function);
  UpdateBits(&cmd_arg, SDIO_IO_RW_DIRECT_REG_ADDR_MASK, SDIO_IO_RW_DIRECT_REG_ADDR_LOC, address);
  const sdmmc_req_t request = {
      .cmd_idx = SDIO_IO_RW_DIRECT,
      .cmd_flags = SDIO_IO_RW_DIRECT_FLAGS,
      .arg = cmd_arg,
      .suppress_error_messages = suppress_error_messages,
  };

  uint32_t response[4];
  if (zx_status_t status = Request(request, response); status != ZX_OK) {
    // Let the platform driver handle logging of this error.
    fdf::debug("SDIO_IO_RW_DIRECT failed: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  const auto read_byte = static_cast<uint8_t>(GetBits(
      response[0], SDIO_IO_RW_DIRECT_RESP_READ_BYTE_MASK, SDIO_IO_RW_DIRECT_RESP_READ_BYTE_LOC));
  return zx::ok(read_byte);
}

zx::result<uint8_t> SdmmcDevice::SdioIoRwDirect(uint32_t function, uint32_t address, uint8_t byte,
                                                bool read_after_write,
                                                bool suppress_error_messages) {
  uint32_t cmd_arg = SDIO_IO_RW_DIRECT_RW_FLAG;
  if (read_after_write) {
    cmd_arg |= SDIO_IO_RW_DIRECT_RAW_FLAG;
  }
  UpdateBits(&cmd_arg, SDIO_IO_RW_DIRECT_FN_IDX_MASK, SDIO_IO_RW_DIRECT_FN_IDX_LOC, function);
  UpdateBits(&cmd_arg, SDIO_IO_RW_DIRECT_REG_ADDR_MASK, SDIO_IO_RW_DIRECT_REG_ADDR_LOC, address);
  UpdateBits(&cmd_arg, SDIO_IO_RW_DIRECT_WRITE_BYTE_MASK, SDIO_IO_RW_DIRECT_WRITE_BYTE_LOC, byte);
  const bool abort = function == 0 && address == SDIO_CIA_CCCR_ASx_ABORT_SEL_CR_ADDR;
  const sdmmc_req_t request = {
      .cmd_idx = SDIO_IO_RW_DIRECT,
      .cmd_flags = abort ? uint32_t{SDIO_IO_RW_DIRECT_ABORT_FLAGS} : SDIO_IO_RW_DIRECT_FLAGS,
      .arg = cmd_arg,
      .suppress_error_messages = abort || suppress_error_messages,
  };

  uint32_t response[4];
  if (zx_status_t status = Request(request, response); status != ZX_OK) {
    // Let the platform driver handle logging of this error.
    fdf::debug("SDIO_IO_RW_DIRECT failed: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  const auto read_byte = static_cast<uint8_t>(GetBits(
      response[0], SDIO_IO_RW_DIRECT_RESP_READ_BYTE_MASK, SDIO_IO_RW_DIRECT_RESP_READ_BYTE_LOC));
  return zx::ok(read_byte);
}

zx_status_t SdmmcDevice::SdioIoRwExtended(uint32_t caps, bool write, uint8_t fn_idx,
                                          uint32_t reg_addr, bool incr, uint32_t blk_count,
                                          uint32_t blk_size,
                                          cpp20::span<const sdmmc_buffer_region_t> buffers) {
  uint32_t cmd_arg = 0;
  if (write) {
    cmd_arg |= SDIO_IO_RW_EXTD_RW_FLAG;
  }
  UpdateBits(&cmd_arg, SDIO_IO_RW_EXTD_FN_IDX_MASK, SDIO_IO_RW_EXTD_FN_IDX_LOC, fn_idx);
  UpdateBits(&cmd_arg, SDIO_IO_RW_EXTD_REG_ADDR_MASK, SDIO_IO_RW_EXTD_REG_ADDR_LOC, reg_addr);
  if (incr) {
    cmd_arg |= SDIO_IO_RW_EXTD_OP_CODE_INCR;
  }

  if (blk_count > 1) {
    if (caps & SDIO_CARD_MULTI_BLOCK) {
      cmd_arg |= SDIO_IO_RW_EXTD_BLOCK_MODE;
      UpdateBits(&cmd_arg, SDIO_IO_RW_EXTD_BYTE_BLK_COUNT_MASK, SDIO_IO_RW_EXTD_BYTE_BLK_COUNT_LOC,
                 blk_count);
    } else {
      // Convert the request into byte mode?
      return ZX_ERR_NOT_SUPPORTED;
    }
  } else {
    // SDIO Spec Table 5-3
    uint32_t arg_blk_size = (blk_size == 512) ? 0 : blk_size;
    UpdateBits(&cmd_arg, SDIO_IO_RW_EXTD_BYTE_BLK_COUNT_MASK, SDIO_IO_RW_EXTD_BYTE_BLK_COUNT_LOC,
               arg_blk_size);
  }
  sdmmc_req_t req = {};
  req.cmd_idx = SDIO_IO_RW_DIRECT_EXTENDED;
  req.arg = cmd_arg;
  req.cmd_flags = write ? (SDIO_IO_RW_DIRECT_EXTENDED_FLAGS)
                        : (SDIO_IO_RW_DIRECT_EXTENDED_FLAGS | SDMMC_CMD_READ),
  req.blocksize = blk_size;
  req.client_id = fn_idx;
  req.buffers_list = buffers.data();
  req.buffers_count = buffers.size();

  uint32_t response[4] = {};
  zx_status_t st = Request(&req, response);
  if (st != ZX_OK) {
    fdf::error("SDIO_IO_RW_DIRECT_EXTENDED failed, retcode = {}", st);
    return st;
  }
  return ZX_OK;
}

// MMC ops

zx::result<uint32_t> SdmmcDevice::MmcSendOpCond(bool suppress_error_messages) {
  const sdmmc_req_t request = {
      .cmd_idx = MMC_SEND_OP_COND,
      .cmd_flags = MMC_SEND_OP_COND_FLAGS,
      .arg = 0,  // Use zero to request the device's OCR without changing its state.
      .suppress_error_messages = suppress_error_messages,
  };
  uint32_t response[4];
  if (zx_status_t status = Request(request, response); status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(response[0]);
}

zx_status_t SdmmcDevice::MmcWaitForReadyState(uint32_t ocr) {
  sdmmc_req_t req = {};
  req.cmd_idx = MMC_SEND_OP_COND;
  req.arg = ocr;
  req.cmd_flags = MMC_SEND_OP_COND_FLAGS;
  zx_status_t st;
  for (int i = 100; i; i--) {
    uint32_t response[4];
    if ((st = Request(req, response)) != ZX_OK) {
      // fail on request error
      break;
    }
    if (response[0] & MMC_OCR_BUSY) {
      break;
    }
    zx::nanosleep(zx::deadline_after(zx::msec(10)));
  }
  return st;
}

zx_status_t SdmmcDevice::MmcAllSendCid(std::array<uint8_t, SDMMC_CID_SIZE>& cid) {
  sdmmc_req_t req = {};
  req.cmd_idx = SDMMC_ALL_SEND_CID;
  req.arg = 0;
  req.cmd_flags = SDMMC_ALL_SEND_CID_FLAGS;
  uint32_t response[4];
  zx_status_t st = Request(req, response);
  auto* const cid_u32 = reinterpret_cast<uint32_t*>(cid.data());
  if (st == ZX_OK) {
    cid_u32[0] = response[0];
    cid_u32[1] = response[1];
    cid_u32[2] = response[2];
    cid_u32[3] = response[3];
  }
  return st;
}

zx_status_t SdmmcDevice::MmcSetRelativeAddr(uint16_t rca) {
  rca_ = rca;
  sdmmc_req_t req = {};
  req.cmd_idx = MMC_SET_RELATIVE_ADDR;
  req.arg = RcaArg();
  req.cmd_flags = MMC_SET_RELATIVE_ADDR_FLAGS;
  uint32_t unused_response[4];
  return Request(req, unused_response);
}

zx_status_t SdmmcDevice::MmcSendCsd(std::array<uint8_t, SDMMC_CSD_SIZE>& csd) {
  sdmmc_req_t req = {};
  req.cmd_idx = SDMMC_SEND_CSD;
  req.arg = RcaArg();
  req.cmd_flags = SDMMC_SEND_CSD_FLAGS;
  uint32_t response[4];
  zx_status_t st = Request(req, response);
  auto* const csd_u32 = reinterpret_cast<uint32_t*>(csd.data());
  if (st == ZX_OK) {
    csd_u32[0] = response[0];
    csd_u32[1] = response[1];
    csd_u32[2] = response[2];
    csd_u32[3] = response[3];
  }
  return st;
}

zx_status_t SdmmcDevice::MmcSendExtCsd(std::array<uint8_t, MMC_EXT_CSD_SIZE>& ext_csd) {
  // EXT_CSD is send in a data stage
  sdmmc_req_t req = {};
  req.cmd_idx = MMC_SEND_EXT_CSD;
  req.arg = 0;
  req.blocksize = 512;
  req.cmd_flags = MMC_SEND_EXT_CSD_FLAGS;
  uint32_t unused_response[4];
  zx_status_t st = RequestWithBlockRead(req, unused_response, {ext_csd.data(), ext_csd.size()});
  if (st != ZX_OK) {
    return st;
  }

  if (fdf::Logger::GlobalInstance()->GetSeverity() <= FUCHSIA_LOG_TRACE) {
    fdf::trace("EXT_CSD:");
    hexdump8_ex(ext_csd.data(), ext_csd.size(), 0);
  }

  return ZX_OK;
}

zx_status_t SdmmcDevice::MmcSleepOrAwake(bool sleep) {
  sdmmc_req_t req = {};
  req.cmd_idx = MMC_SLEEP_AWAKE;
  req.arg = RcaArg() | (sleep ? 1 : 0) << 15;
  req.cmd_flags = MMC_SLEEP_AWAKE_FLAGS;
  uint32_t response[4];
  zx_status_t status = Request(req, response);
  if (status != ZX_OK) {
    return status;
  }

  if (sleep) {
    if (MMC_STATUS_CURRENT_STATE(response[0]) != MMC_STATUS_CURRENT_STATE_STBY) {
      return ZX_ERR_BAD_STATE;
    }
  } else {
    if (MMC_STATUS_CURRENT_STATE(response[0]) != MMC_STATUS_CURRENT_STATE_SLP) {
      return ZX_ERR_BAD_STATE;
    }
  }
  return ZX_OK;
}

zx_status_t SdmmcDevice::MmcSelectCard(bool select) {
  sdmmc_req_t req = {};
  req.cmd_idx = MMC_SELECT_CARD;
  req.arg = select ? RcaArg() : 0;
  req.cmd_flags = MMC_SELECT_CARD_FLAGS;
  // Some cards don't respond to DESELECT_CARD. Wait for a response in case one is sent, but ignore
  // any errors.
  req.suppress_error_messages = !select;
  uint32_t unused_response[4];
  zx_status_t status = Request(req, unused_response);
  return select ? status : ZX_OK;
}

zx_status_t SdmmcDevice::MmcSwitch(uint8_t index, uint8_t value) {
  // Send the MMC_SWITCH command
  uint32_t arg = (3 << 24) |  // write byte
                 (index << 16) | (value << 8);
  sdmmc_req_t req = {};
  req.cmd_idx = MMC_SWITCH;
  req.arg = arg;
  req.cmd_flags = MMC_SWITCH_FLAGS;
  uint32_t unused_response[4];
  return Request(req, unused_response);
}

zx_status_t SdmmcDevice::HostInfo(sdmmc_host_info_t* info) {
  if (!using_fidl_) {
    return host_.HostInfo(info);
  }

  fdf::Arena arena('SDMC');
  auto result = client_.sync().buffer(arena)->HostInfo();
  if (!result.ok()) {
    fdf::error("HostInfo request failed: {}", result.status_string());
    return result.status();
  }

  if (result->is_error()) {
    return result->error_value();
  }
  auto& response = result.value();
  info->caps = static_cast<uint64_t>(response->info.caps);
  info->max_transfer_size = response->info.max_transfer_size;
  info->max_buffer_regions = response->info.max_buffer_regions;
  return ZX_OK;
}

zx_status_t SdmmcDevice::SetSignalVoltage(sdmmc_voltage_t voltage) {
  if (!using_fidl_) {
    return host_.SetSignalVoltage(voltage);
  }

  fuchsia_hardware_sdmmc::wire::SdmmcVoltage wire_voltage;
  switch (voltage) {
    case SDMMC_VOLTAGE_V330:
      wire_voltage = fuchsia_hardware_sdmmc::wire::SdmmcVoltage::kV330;
      break;
    case SDMMC_VOLTAGE_V180:
      wire_voltage = fuchsia_hardware_sdmmc::wire::SdmmcVoltage::kV180;
      break;
    case SDMMC_VOLTAGE_MAX:
      wire_voltage = fuchsia_hardware_sdmmc::wire::SdmmcVoltage::kMax;
      break;
  }

  fdf::Arena arena('SDMC');
  auto result = client_.sync().buffer(arena)->SetSignalVoltage(wire_voltage);
  if (!result.ok()) {
    fdf::error("SetSignalVoltage request failed: {}", result.status_string());
    return result.status();
  }

  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t SdmmcDevice::SetBusWidth(sdmmc_bus_width_t bus_width) {
  if (!using_fidl_) {
    return host_.SetBusWidth(bus_width);
  }

  fuchsia_hardware_sdmmc::wire::SdmmcBusWidth wire_bus_width;
  switch (bus_width) {
    case SDMMC_BUS_WIDTH_ONE:
      wire_bus_width = fuchsia_hardware_sdmmc::wire::SdmmcBusWidth::kOne;
      break;
    case SDMMC_BUS_WIDTH_FOUR:
      wire_bus_width = fuchsia_hardware_sdmmc::wire::SdmmcBusWidth::kFour;
      break;
    case SDMMC_BUS_WIDTH_EIGHT:
      wire_bus_width = fuchsia_hardware_sdmmc::wire::SdmmcBusWidth::kEight;
      break;
    case SDMMC_BUS_WIDTH_MAX:
      wire_bus_width = fuchsia_hardware_sdmmc::wire::SdmmcBusWidth::kMax;
      break;
  }

  fdf::Arena arena('SDMC');
  auto result = client_.sync().buffer(arena)->SetBusWidth(wire_bus_width);
  if (!result.ok()) {
    fdf::error("SetBusWidth request failed: {}", result.status_string());
    return result.status();
  }

  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t SdmmcDevice::SetBusFreq(uint32_t bus_freq) {
  if (bus_freq > max_frequency_) {
    bus_freq = max_frequency_;
  }

  if (!using_fidl_) {
    return host_.SetBusFreq(bus_freq);
  }

  fdf::Arena arena('SDMC');
  auto result = client_.sync().buffer(arena)->SetBusFreq(bus_freq);
  if (!result.ok()) {
    fdf::error("SetBusFreq request failed: {}", result.status_string());
    return result.status();
  }

  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t SdmmcDevice::SetTiming(sdmmc_timing_t timing) {
  if (!using_fidl_) {
    return host_.SetTiming(timing);
  }

  fuchsia_hardware_sdmmc::wire::SdmmcTiming wire_timing;
  switch (timing) {
    case SDMMC_TIMING_LEGACY:
      wire_timing = fuchsia_hardware_sdmmc::wire::SdmmcTiming::kLegacy;
      break;
    case SDMMC_TIMING_HS:
      wire_timing = fuchsia_hardware_sdmmc::wire::SdmmcTiming::kHs;
      break;
    case SDMMC_TIMING_HSDDR:
      wire_timing = fuchsia_hardware_sdmmc::wire::SdmmcTiming::kHsddr;
      break;
    case SDMMC_TIMING_HS200:
      wire_timing = fuchsia_hardware_sdmmc::wire::SdmmcTiming::kHs200;
      break;
    case SDMMC_TIMING_HS400:
      wire_timing = fuchsia_hardware_sdmmc::wire::SdmmcTiming::kHs400;
      break;
    case SDMMC_TIMING_SDR12:
      wire_timing = fuchsia_hardware_sdmmc::wire::SdmmcTiming::kSdr12;
      break;
    case SDMMC_TIMING_SDR25:
      wire_timing = fuchsia_hardware_sdmmc::wire::SdmmcTiming::kSdr25;
      break;
    case SDMMC_TIMING_SDR50:
      wire_timing = fuchsia_hardware_sdmmc::wire::SdmmcTiming::kSdr50;
      break;
    case SDMMC_TIMING_SDR104:
      wire_timing = fuchsia_hardware_sdmmc::wire::SdmmcTiming::kSdr104;
      break;
    case SDMMC_TIMING_DDR50:
      wire_timing = fuchsia_hardware_sdmmc::wire::SdmmcTiming::kDdr50;
      break;
    case SDMMC_TIMING_HS400_ENHANCED_STROBE:
      wire_timing = fuchsia_hardware_sdmmc::wire::SdmmcTiming::kHs400EnhancedStrobe;
      break;
    case SDMMC_TIMING_MAX:
      wire_timing = fuchsia_hardware_sdmmc::wire::SdmmcTiming::kMax;
      break;
  }

  fdf::Arena arena('SDMC');
  auto result = client_.sync().buffer(arena)->SetTiming(wire_timing);
  if (!result.ok()) {
    fdf::error("SetTiming request failed: {}", result.status_string());
    return result.status();
  }

  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t SdmmcDevice::HwReset() {
  if (!using_fidl_) {
    return host_.HwReset();
  }

  fdf::Arena arena('SDMC');
  auto result = client_.sync().buffer(arena)->HwReset();
  if (!result.ok()) {
    fdf::error("HwReset request failed: {}", result.status_string());
    return result.status();
  }

  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t SdmmcDevice::PerformTuning(uint32_t cmd_idx) {
  if (!using_fidl_) {
    return host_.PerformTuning(cmd_idx);
  }

  fdf::Arena arena('SDMC');
  auto result = client_.sync().buffer(arena)->PerformTuning(cmd_idx);
  if (!result.ok()) {
    fdf::error("PerformTuning request failed: {}", result.status_string());
    return result.status();
  }

  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t SdmmcDevice::RegisterInBandInterrupt(
    fdf::ClientEnd<fuchsia_hardware_sdmmc::InBandInterrupt> client_end) {
  if (!using_fidl_) {
    // Not supported by Banjo.
    return ZX_ERR_NOT_SUPPORTED;
  }

  fdf::Arena arena('SDMC');
  auto result = client_.sync().buffer(arena)->RegisterInBandInterrupt(std::move(client_end));
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

void SdmmcDevice::AckInBandInterrupt() {
  if (using_fidl_) {
    fdf::Arena arena('SDMC');
    auto result = client_.sync().buffer(arena)->AckInBandInterrupt();
    if (!result.ok()) {
      fdf::error("AckInBandInterrupt request failed: {}", result.status_string());
    }
  }
}

zx::result<SdmmcDevice::InitializeCommandQueueingResources> SdmmcDevice::InitializeCommandQueueing(
    zx::interrupt virtual_interrupt, zx::eventpair virtual_interrupt_lifeline) {
  if (using_fidl_) {
    fdf::Arena arena('SDMC');
    auto result = client_.sync().buffer(arena)->InitializeCommandQueueing(
        std::move(virtual_interrupt), std::move(virtual_interrupt_lifeline));
    if (!result.ok()) {
      return zx::error(result.status());
    }
    if (result->is_error()) {
      return result->take_error();
    }
    return zx::ok(InitializeCommandQueueingResources{
        .cqhci_mmio = std::move(result.value()->cqhci_mmio),
        .cqhci_mmio_offset = result.value()->cqhci_mmio_offset,
        .sdhci_mmio = std::move(result.value()->sdhci_mmio),
        .sdhci_mmio_offset = result.value()->sdhci_mmio_offset,
        .bti = std::move(result.value()->bti),
        .interrupt = std::move(result.value()->interrupt),
    });
  }
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<> SdmmcDevice::EnableCqhci() {
  if (!using_fidl_) {
    // Not supported by Banjo.
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  fdf::Arena arena('SDMC');
  fdf::WireUnownedResult result = client_.sync().buffer(arena)->EnableCqhci();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  if (result->is_error()) {
    return result->take_error();
  }
  return zx::ok();
}

zx::result<> SdmmcDevice::DisableCqhci() {
  if (!using_fidl_) {
    // Not supported by Banjo.
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  fdf::Arena arena('SDMC');
  fdf::WireUnownedResult result = client_.sync().buffer(arena)->DisableCqhci();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  if (result->is_error()) {
    return result->take_error();
  }
  return zx::ok();
}

zx_status_t SdmmcDevice::RegisterVmo(uint32_t vmo_id, uint8_t client_id, zx::vmo vmo,
                                     uint64_t offset, uint64_t size, uint32_t vmo_rights) {
  if (!using_fidl_) {
    return host_.RegisterVmo(vmo_id, client_id, std::move(vmo), offset, size, vmo_rights);
  }

  fdf::Arena arena('SDMC');
  auto result = client_.sync().buffer(arena)->RegisterVmo(
      vmo_id, client_id, std::move(vmo), offset, size,
      static_cast<fuchsia_hardware_sdmmc::SdmmcVmoRight>(vmo_rights));
  if (!result.ok()) {
    fdf::error("RegisterVmo request failed: {}", result.status_string());
    return result.status();
  }

  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t SdmmcDevice::UnregisterVmo(uint32_t vmo_id, uint8_t client_id, zx::vmo* out_vmo) {
  if (!using_fidl_) {
    return host_.UnregisterVmo(vmo_id, client_id, out_vmo);
  }

  fdf::Arena arena('SDMC');
  auto result = client_.sync().buffer(arena)->UnregisterVmo(vmo_id, client_id);
  if (!result.ok()) {
    fdf::error("UnregisterVmo request failed: {}", result.status_string());
    return result.status();
  }

  if (result->is_error()) {
    return result->error_value();
  }
  *out_vmo = std::move(result.value()->vmo);
  return ZX_OK;
}

zx_status_t SdmmcDevice::Request(const sdmmc_req_t* req, uint32_t out_response[4]) {
  if (!using_fidl_) {
    return host_.Request(req, out_response);
  }

  fdf::Arena arena('SDMC');
  zx::result<fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>> wire_req_vector =
      BanjoToFidlReqVector(req, 1, &arena);
  if (wire_req_vector.is_error()) {
    return wire_req_vector.error_value();
  }

  auto result = client_.sync().buffer(arena)->Request(*wire_req_vector);
  if (!result.ok()) {
    fdf::error("Request request failed: {}", result.status_string());
    return result.status();
  }

  if (result->is_error()) {
    return result->error_value();
  }
  for (int i = 0; i < 4; i++) {
    out_response[i] = result.value()->response[i];
  }
  return ZX_OK;
}

fdf::Logger& SdmmcDevice::logger() { return root_device_->logger(); }

}  // namespace sdmmc
