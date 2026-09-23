// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-spifc.h"

#include <lib/driver/logging/cpp/logger.h>
#include <string.h>
#include <unistd.h>

#include <fbl/algorithm.h>

#include "spifc-regs.h"

namespace amlspinand {

static constexpr uint32_t kCacheSizeInByte = 512;

void AmlSpiFlashController::SetRxOpMode(SpiRxMode mode, OpCmd cmd) {
  uint32_t val = 0;

  if (mode == SpiRxMode::kSpiRxDual && cmd == OpCmd::kCmdReadBufDual) {
    val = 0x1;
  } else if (mode == SpiRxMode::kSpiRxQuad && cmd == OpCmd::kCmdReadBufQuad) {
    val = (0x1 << 1);
  }

  UserCtrl3::Get().ReadFrom(&spifc_mmio_).set_din_mode(val).WriteTo(&spifc_mmio_);
}

void AmlSpiFlashController::SetTxOpMode(SpiTxMode mode, OpCmd cmd) {
  uint32_t val = 0;

  if (mode == SpiTxMode::kSpiTxQuad && cmd == OpCmd::kCmdProgLoadQuad) {
    val = (0x1 << 1);
  }

  UserCtrl1::Get().ReadFrom(&spifc_mmio_).set_dout_mode(val).WriteTo(&spifc_mmio_);
}

void AmlSpiFlashController::UserCmd(OpCmd cmd, uint32_t addr, uint32_t addr_len) {
  // Adjust to register format
  auto addr_nbytes = addr_len ? (addr_len - 1) : 0;

  UserCtrl1::Get()
      .FromValue(0)
      .set_cmd_cycle_en(1)
      .set_addr_cycle_en(!!addr_len)
      .set_cmd_code(static_cast<uint8_t>(cmd))
      .set_addr_nbytes(addr_nbytes)
      .WriteTo(&spifc_mmio_);
  UserCtrl2::Get().FromValue(0).WriteTo(&spifc_mmio_);
  UserCtrl3::Get().FromValue(0).WriteTo(&spifc_mmio_);
  UserAddr::Get().FromValue(addr).WriteTo(&spifc_mmio_);
  UserCtrl0::Get().FromValue(0).set_req_en(1).WriteTo(&spifc_mmio_);

  // Wait the command finished
  while (!(UserCtrl0::Get().ReadFrom(&spifc_mmio_).req_done()))
    ;
}

zx_status_t AmlSpiFlashController::UserCmdDout(OpCmd cmd, bool continuous_write, uint32_t addr,
                                               uint32_t addr_len, const uint8_t *buf,
                                               uint32_t len) {
  // kCmdProgLoad and kCmdProgLoadQuad do not support continuous write,
  // We need to change the commands to kCmdProgLoadRandom and kCmdProgLoadRandomQuad
  auto real_cmd = cmd;
  if (continuous_write) {
    if (cmd == OpCmd::kCmdProgLoad) {
      real_cmd = OpCmd::kCmdProgLoadRandom;
    } else if (cmd == OpCmd::kCmdProgLoadQuad) {
      real_cmd = OpCmd::kCmdProgLoadRandomQuad;
    }
  }

  // write DBUF and auto update address
  DbufCtrl::Get().FromValue(0).set_rw(1).set_auto_update_addr(1).WriteTo(&spifc_mmio_);

  uint32_t temp_buf[kCacheSizeInByte / sizeof(uint32_t)] = {};
  memcpy(temp_buf, buf, len);
  auto words = fbl::round_up(len, sizeof(uint32_t)) / sizeof(uint32_t);
  for (uint32_t i = 0; i < words; i++) {
    DbufData::Get().FromValue(temp_buf[i]).WriteTo(&spifc_mmio_);
  }

  // adjust to register format
  auto addr_nbytes = addr_len ? (addr_len - 1) : 0;
  UserCtrl1::Get()
      .ReadFrom(&spifc_mmio_)
      .set_cmd_cycle_en(1)
      .set_cmd_mode(0)
      .set_cmd_code(static_cast<uint8_t>(real_cmd))
      .set_addr_cycle_en(!!addr_len)
      .set_addr_mode(0)
      .set_addr_nbytes(addr_nbytes)
      .set_dout_en(1)
      .set_dout_aes_en(0)
      .set_dout_src(0)
      .set_dout_nbytes(len)
      .WriteTo(&spifc_mmio_);
  // disable placeholder data
  UserCtrl2::Get().FromValue(0).WriteTo(&spifc_mmio_);
  // disable data in
  UserCtrl3::Get().FromValue(0).WriteTo(&spifc_mmio_);
  // clear buffer start address
  UserDbufAddr::Get().FromValue(0).WriteTo(&spifc_mmio_);
  UserAddr::Get().FromValue(addr).WriteTo(&spifc_mmio_);

  UserCtrl0::Get().FromValue(0).set_req_en(1).WriteTo(&spifc_mmio_);
  while (!(UserCtrl0::Get().ReadFrom(&spifc_mmio_).req_done()))
    ;

  return ZX_OK;
}

zx_status_t AmlSpiFlashController::UserCmdDin(OpCmd cmd, uint32_t addr, uint32_t addr_len,
                                              uint8_t *buf, uint32_t len) {
  uint32_t temp_buf[kCacheSizeInByte / sizeof(uint32_t)] = {};
  // adjust to register format
  auto addr_nbytes = addr_len ? (addr_len - 1) : 0;

  // enable and set command address
  UserCtrl1::Get()
      .FromValue(0)
      .set_cmd_cycle_en(1)
      .set_cmd_code(static_cast<uint8_t>(cmd))
      .set_addr_cycle_en(!!addr_len)
      .set_addr_nbytes(addr_nbytes)
      .WriteTo(&spifc_mmio_);
  UserAddr::Get().FromValue(addr).WriteTo(&spifc_mmio_);

  // disable placeholder data
  UserCtrl2::Get().FromValue(0).WriteTo(&spifc_mmio_);
  // enable data in
  UserCtrl3::Get()
      .ReadFrom(&spifc_mmio_)
      .set_din_en(1)
      .set_din_dest(0)
      .set_din_aes_en(0)
      .set_din_nbytes(len)
      .WriteTo(&spifc_mmio_);
  // clear buffer start address
  UserDbufAddr::Get().FromValue(0).WriteTo(&spifc_mmio_);
  UserCtrl0::Get().FromValue(0).set_req_en(1).WriteTo(&spifc_mmio_);

  // check the command and data status
  while (!(UserCtrl0::Get().ReadFrom(&spifc_mmio_).req_done()))
    ;
  while (!(UserCtrl0::Get().ReadFrom(&spifc_mmio_).data_update()))
    ;

  DbufCtrl::Get().FromValue(0).set_auto_update_addr(1).WriteTo(&spifc_mmio_);
  auto words = fbl::round_up(len, sizeof(uint32_t)) / sizeof(uint32_t);
  for (uint32_t i = 0; i < words; i++) {
    temp_buf[i] = DbufData::Get().ReadFrom(&spifc_mmio_).dbuf_data();
  }
  memcpy(buf, temp_buf, len);

  return ZX_OK;
}

void AmlSpiFlashController::SetClk(uint32_t hz) {
  uint8_t div = 0;

  switch (hz) {
    case 96'000'000:
      div = 7;
      break;
    case 48'000'000:
      div = 15;
      break;
    default:
      div = 31;  // 24'000'000
      break;
  }
  ClkCtrl::Get().ReadFrom(&clk_mmio_).set_div(div).WriteTo(&clk_mmio_);
}

zx_status_t AmlSpiFlashController::SetSpeed(uint32_t hz) {
  if (hz == 0 || hz == current_hz_) {
    return ZX_OK;
  }

  SetClk(hz);
  Actiming0::Get()
      .FromValue(0)
      .set_tslch(1)
      .set_tclsh(1)
      .set_tshwl(7)
      .set_tshshl2(7)
      .set_tshsl1(7)
      .set_twhsl(2)
      .WriteTo(&spifc_mmio_);
  current_hz_ = hz;

  return ZX_OK;
}

void AmlSpiFlashController::Init() {
  // Disable AHB
  AhbReqCtrl::Get().ReadFrom(&spifc_mmio_).set_req_en(0).WriteTo(&spifc_mmio_);
  AhbCtrl::Get().ReadFrom(&spifc_mmio_).set_bus_en(0).WriteTo(&spifc_mmio_);
  UserDbufAddr::Get().FromValue(0).WriteTo(&spifc_mmio_);
}

zx_status_t AmlSpiFlashController::Xfer(TransferMsg msg, uint32_t transfer_len, const void *dout,
                                        void *din) {
  zx_status_t status = ZX_OK;

  auto len = transfer_len;
  if (din == nullptr && dout == nullptr) {
    UserCmd(msg.cmd, msg.addr, msg.addr_len);
  } else if (dout != nullptr) {
    auto buf = reinterpret_cast<const uint8_t *>(dout);
    auto addr = msg.addr;
    bool continuous = false;
    SetTxOpMode(msg.tx_mode, msg.cmd);
    while (len > 0) {
      auto transfer_size = std::min(len, kCacheSizeInByte);
      status = UserCmdDout(msg.cmd, continuous, addr, msg.addr_len, buf, transfer_size);
      if (status != ZX_OK) {
        FDF_LOG(ERROR, "data transfer didn't finish");
        break;
      }
      buf += transfer_size;
      len -= transfer_size;
      addr += transfer_size;
      continuous = true;
    }
  } else if (din != nullptr) {
    auto buf = reinterpret_cast<uint8_t *>(din);
    // Combine one byte placeholder
    auto addr = msg.placeholder ? msg.addr << 8 : msg.addr;
    auto addr_len = msg.placeholder ? msg.addr_len + 1 : msg.addr_len;
    SetRxOpMode(msg.rx_mode, msg.cmd);
    while (len > 0) {
      auto transfer_size = std::min(len, kCacheSizeInByte);
      status = UserCmdDin(msg.cmd, addr, addr_len, buf, transfer_size);
      if (status != ZX_OK) {
        FDF_LOG(ERROR, "data transfer didn't finish");
        break;
      }
      buf += transfer_size;
      len -= transfer_size;
      if (msg.placeholder) {
        addr = addr >> 8;
        addr += transfer_size;
        addr = addr << 8;
      } else {
        addr += transfer_size;
      }
    }
  } else {
    return ZX_ERR_INVALID_ARGS;
  }

  return status;
}

}  // namespace amlspinand
