// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_NAND_DRIVERS_AML_SPINAND_SPI_OP_H_
#define SRC_DEVICES_NAND_DRIVERS_AML_SPINAND_SPI_OP_H_

#include <zircon/types.h>

#include <string_view>
namespace amlspinand {

// Flash operation command
enum class OpCmd {
  kCmdDefault = 0x0,
  kCmdWriteDisable = 0x4,
  kCmdWriteEnable = 0x6,
  kCmdGetFeature = 0xf,
  kCmdSetFeature = 0x1f,
  kCmdReadCellArray = 0x13,
  kCmdReadBuf = 0x3,
  kCmdReadBufFast = 0xb,
  kCmdReadBufDual = 0x3b,
  kCmdReadBufQuad = 0x6b,
  kCmdProgLoad = 0x2,
  kCmdProgLoadQuad = 0x32,
  kCmdProgLoadRandom = 0x84,
  kCmdProgLoadRandomQuad = 0x34,
  kCmdProgExec = 0x10,
  kCmdBlockErase = 0xd8,
  kCmdReadId = 0x9f,
  kCmdReset = 0xff,
};

enum class SpiDataDir {
  kSpiDataIn,
  kSpiDataOut,
};

struct TransferCmd {
  TransferCmd(OpCmd opcode = OpCmd::kCmdDefault, uint8_t buswidth = 0)
      : opcode(opcode), buswidth(buswidth) {}

  OpCmd opcode;
  uint8_t buswidth;
};

struct TransferAddr {
  TransferAddr(uint8_t nbytes = 0, uint8_t buswidth = 0, uint32_t val = 0)
      : nbytes(nbytes), buswidth(buswidth), val(val) {}

  uint8_t nbytes;
  uint8_t buswidth;
  uint32_t val;
};

struct TransferPlaceholder {
  TransferPlaceholder(uint8_t nbytes = 0, uint8_t buswidth = 0)
      : nbytes(nbytes), buswidth(buswidth) {}

  uint8_t nbytes;
  uint8_t buswidth;
};

struct TransferData {
  TransferData(uint8_t buswidth = 0, SpiDataDir dir = SpiDataDir::kSpiDataIn, uint32_t nbytes = 0,
               uint8_t *buf_in = nullptr, uint8_t *buf_out = nullptr)
      : buswidth(buswidth), dir(dir), nbytes(nbytes), buf_in(buf_in), buf_out(buf_out) {}

  uint8_t buswidth;
  SpiDataDir dir;
  uint32_t nbytes;
  uint8_t *buf_in;
  uint8_t *buf_out;
};

struct SpiOp {
  SpiOp(TransferCmd cmd = {}, TransferAddr addr = {}, TransferPlaceholder placeholder = {},
        TransferData data = {})
      : cmd(cmd), addr(addr), placeholder(placeholder), data(data) {}

  TransferCmd cmd;
  TransferAddr addr;
  TransferPlaceholder placeholder;
  TransferData data;

  // Standard SPI NAND flash operations
  static SpiOp Reset() { return SpiOp(TransferCmd(OpCmd::kCmdReset, 1)); }
  static SpiOp WrEnable(bool en) {
    return SpiOp(TransferCmd(en ? OpCmd::kCmdWriteEnable : OpCmd::kCmdWriteDisable, 1));
  }
  static SpiOp ReadId(uint8_t nbytes, uint8_t *buf, uint32_t len) {
    return SpiOp(TransferCmd(OpCmd::kCmdReadId, 1), TransferAddr(1, 1, 0x0),
                 TransferPlaceholder(nbytes, 1),
                 TransferData(1, SpiDataDir::kSpiDataIn, len, buf, nullptr));
  }
  static SpiOp SetFeature(uint32_t reg, uint8_t *buf) {
    return SpiOp(TransferCmd(OpCmd::kCmdSetFeature, 1), TransferAddr(1, 1, reg), {},
                 TransferData(1, SpiDataDir::kSpiDataOut, 1, nullptr, buf));
  }
  static SpiOp GetFeature(uint32_t reg, uint8_t *buf) {
    return SpiOp(TransferCmd(OpCmd::kCmdGetFeature, 1), TransferAddr(1, 1, reg), {},
                 TransferData(1, SpiDataDir::kSpiDataIn, 1, buf, nullptr));
  }
  static SpiOp BlockErase(uint32_t addr) {
    return SpiOp(TransferCmd(OpCmd::kCmdBlockErase, 1), TransferAddr(3, 1, addr), {}, {});
  }
  static SpiOp PageRead(uint32_t addr) {
    return SpiOp(TransferCmd(OpCmd::kCmdReadCellArray, 1), TransferAddr(3, 1, addr), {}, {});
  }
  static SpiOp PageReadFromCache(bool fast, uint32_t addr, uint8_t nbytes, uint8_t *buf,
                                 uint32_t len) {
    return SpiOp(TransferCmd(fast ? OpCmd::kCmdReadBufFast : OpCmd::kCmdReadBuf, 1),
                 TransferAddr(2, 1, addr), TransferPlaceholder(nbytes, 1),
                 TransferData(2, SpiDataDir::kSpiDataIn, len, buf, nullptr));
  }
  static SpiOp PageReadFromCacheX2(uint32_t addr, uint8_t nbytes, uint8_t *buf, uint32_t len) {
    return SpiOp(TransferCmd(OpCmd::kCmdReadBufDual, 1), TransferAddr(2, 1, addr),
                 TransferPlaceholder(nbytes, 1),
                 TransferData(2, SpiDataDir::kSpiDataIn, len, buf, nullptr));
  }
  static SpiOp PageReadFromCacheX4(uint32_t addr, uint8_t nbytes, uint8_t *buf, uint32_t len) {
    return SpiOp(TransferCmd(OpCmd::kCmdReadBufQuad, 1), TransferAddr(2, 1, addr),
                 TransferPlaceholder(nbytes, 1),
                 TransferData(4, SpiDataDir::kSpiDataIn, len, buf, nullptr));
  }
  static SpiOp ProgExec(uint32_t addr) {
    return SpiOp(TransferCmd(OpCmd::kCmdProgExec, 1), TransferAddr(3, 1, addr), {}, {});
  }
  static SpiOp ProgLoad(bool reset, uint32_t addr, uint8_t *buf, uint32_t len) {
    return SpiOp(TransferCmd(reset ? OpCmd::kCmdProgLoad : OpCmd::kCmdProgLoadRandom, 1),
                 TransferAddr(2, 1, addr), {},
                 TransferData(1, SpiDataDir::kSpiDataOut, len, nullptr, buf));
  }
  static SpiOp ProgLoadX4(bool reset, uint32_t addr, uint8_t *buf, uint32_t len) {
    return SpiOp(TransferCmd(reset ? OpCmd::kCmdProgLoadQuad : OpCmd::kCmdProgLoadRandomQuad, 1),
                 TransferAddr(2, 1, addr), {},
                 TransferData(4, SpiDataDir::kSpiDataOut, len, nullptr, buf));
  }
};

// feature register
constexpr uint32_t kBlockLockReg = 0xa0;
constexpr uint32_t kBlockAllUnlocked = 0x00;

// configuration register
constexpr uint32_t kCfgReg = 0xb0;
constexpr uint32_t kCfgOtpEnable = (0x1 << 6);
constexpr uint32_t kCfgEccEnable = (0x1 << 4);
constexpr uint32_t kCfgQuadEnable = 0x1;

// status register
constexpr uint32_t kStatusReg = 0xc0;
constexpr uint32_t kStatusBusy = 0x1;
constexpr uint32_t kStatusEraseFailed = (0x1 << 2);
constexpr uint32_t kStatusProgFailed = (0x1 << 3);
constexpr uint32_t kStatusEccMask = 0x30;
constexpr uint32_t kStatusEccNoBitflips = 0x0 << 4;
constexpr uint32_t kStatusEccHasBitflips = 0x1 << 4;
constexpr uint32_t kStatusEccUncorrectErr = 0x2 << 4;

}  // namespace amlspinand

#endif  // SRC_DEVICES_NAND_DRIVERS_AML_SPINAND_SPI_OP_H_
