// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_NAND_DRIVERS_AML_SPINAND_AML_SPIFC_H_
#define SRC_DEVICES_NAND_DRIVERS_AML_SPINAND_AML_SPIFC_H_

#include <lib/driver/mmio/cpp/mmio.h>
#include <zircon/types.h>

#include "spi-op.h"

namespace amlspinand {

// SPI rx mode flags
enum class SpiRxMode {
  kSpiRxDefault = 0x0,
  kSpiRxDual = (0x1 << 12),
  kSpiRxQuad = (0x1 << 13),
};

// SPI tx mode flags
enum class SpiTxMode {
  kSpiTxDefault = 0x0,
  kSpiTxDual = (0x1 << 9),
  kSpiTxQuad = (0x1 << 10),
};

struct TransferMsg {
  OpCmd cmd;
  uint32_t addr;
  uint32_t addr_len;
  SpiRxMode rx_mode;
  SpiTxMode tx_mode;
  bool placeholder;
};

class AmlSpiFlashController {
 public:
  explicit AmlSpiFlashController(fdf::MmioBuffer spifc_mmio, fdf::MmioBuffer clk_mmio)
      : spifc_mmio_(std::move(spifc_mmio)), clk_mmio_(std::move(clk_mmio)) {}
  virtual ~AmlSpiFlashController() = default;

  void Init();
  zx_status_t Xfer(TransferMsg msg, uint32_t transfer_len, const void *dout, void *din);
  zx_status_t SetSpeed(uint32_t hz);

 private:
  void SetRxOpMode(SpiRxMode mode, OpCmd cmd);
  void SetTxOpMode(SpiTxMode mode, OpCmd cmd);
  void UserCmd(OpCmd cmd, uint32_t addr, uint32_t addr_len);
  zx_status_t UserCmdDout(OpCmd cmd, bool continuous_write, uint32_t addr, uint32_t addr_len,
                          const uint8_t *buf, uint32_t len);
  zx_status_t UserCmdDin(OpCmd cmd, uint32_t addr, uint32_t addr_len, uint8_t *buf, uint32_t len);
  void SetClk(uint32_t hz);

  fdf::MmioBuffer spifc_mmio_;
  fdf::MmioBuffer clk_mmio_;
  uint32_t current_hz_ = 0;
};

}  // namespace amlspinand

#endif  // SRC_DEVICES_NAND_DRIVERS_AML_SPINAND_AML_SPIFC_H_
