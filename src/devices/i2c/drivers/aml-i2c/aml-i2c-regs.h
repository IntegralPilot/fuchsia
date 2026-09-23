// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_I2C_DRIVERS_AML_I2C_AML_I2C_REGS_H_
#define SRC_DEVICES_I2C_DRIVERS_AML_I2C_AML_I2C_REGS_H_

#include <zircon/assert.h>
#include <zircon/types.h>

#include <limits>

#include <hwreg/bitfields.h>

namespace aml_i2c {

constexpr zx_off_t kControlReg = 0x00;
constexpr zx_off_t kTargetAddrReg = 0x04;
constexpr zx_off_t kTokenList0Reg = 0x08;
constexpr zx_off_t kTokenList1Reg = 0x0c;
constexpr zx_off_t kWriteData0Reg = 0x10;
constexpr zx_off_t kWriteData1Reg = 0x14;
constexpr zx_off_t kReadData0Reg = 0x18;
constexpr zx_off_t kReadData1Reg = 0x1c;

class Control : public hwreg::RegisterBase<Control, uint32_t> {
 public:
  static constexpr uint32_t kQtrClkDlyMax = 0x3ff;

  // There is a separate set of bits in the control register (QTR_CLK_EXT) that
  // extends this field to 12 bits. Only expose the main 10-bit field in order
  // to simplify the driver logic (delay values for normal bus frequencies won't
  // use anywhere near 10 bits anyway).
  DEF_FIELD(21, 12, qtr_clk_dly);
  DEF_BIT(3, error);
  DEF_BIT(2, status);
  DEF_BIT(1, ack_ignore);
  DEF_BIT(0, start);

  static auto Get() { return hwreg::RegisterAddr<Control>(kControlReg); }
};

class TargetAddr : public hwreg::RegisterBase<TargetAddr, uint32_t> {
 public:
  static constexpr uint32_t kSclLowDelayMax = 0xfff;

  DEF_BIT(28, use_cnt_scl_low);
  DEF_FIELD(27, 16, scl_low_dly);
  DEF_FIELD(7, 1, target_address);
  DEF_RSVDZ_BIT(0);  // R/W bit, must be zero.

  static auto Get() { return hwreg::RegisterAddr<TargetAddr>(kTargetAddrReg); }
};

class TokenList : public hwreg::RegisterBase<TokenList, uint64_t> {
 public:
  enum class Token : uint64_t {
    kEnd,
    kStart,
    kTargetAddrWr,
    kTargetAddrRd,
    kData,
    kDataLast,
    kStop,
  };

  void Push(const Token token) {
    ZX_ASSERT((kTokenSizeBits * token_count_) < std::numeric_limits<uint64_t>::digits);

    const uint64_t token_mask = static_cast<uint64_t>(token) << (kTokenSizeBits * token_count_++);
    set_reg_value(reg_value() | token_mask);
  }

  template <typename T>
  TokenList& ReadFrom(T* reg_io) = delete;

  template <typename T>
  TokenList& WriteTo(T* reg_io) {
    reg_io->Write32(reg_value() & std::numeric_limits<uint32_t>::max(), reg_addr());
    reg_io->Write32(reg_value() >> std::numeric_limits<uint32_t>::digits,
                    reg_addr() + sizeof(uint32_t));
    set_reg_value(0);
    token_count_ = 0;
    return *this;
  }

  static auto Get() { return hwreg::RegisterAddr<TokenList>(kTokenList0Reg); }

 private:
  static constexpr uint32_t kTokenSizeBits = 4;

  uint32_t token_count_ = 0;
};

class WriteData : public hwreg::RegisterBase<WriteData, uint64_t> {
 public:
  static constexpr uint32_t kMaxWriteBytesPerTransfer = 8;

  void Push(const uint8_t byte) {
    ZX_ASSERT(byte_count_ < kMaxWriteBytesPerTransfer);

    const uint64_t byte_mask = static_cast<uint64_t>(byte)
                               << (std::numeric_limits<uint8_t>::digits * byte_count_++);
    set_reg_value(reg_value() | byte_mask);
  }

  template <typename T>
  WriteData& ReadFrom(T* reg_io) = delete;

  template <typename T>
  WriteData& WriteTo(T* reg_io) {
    reg_io->Write32(reg_value() & std::numeric_limits<uint32_t>::max(), reg_addr());
    reg_io->Write32(reg_value() >> std::numeric_limits<uint32_t>::digits,
                    reg_addr() + sizeof(uint32_t));
    set_reg_value(0);
    byte_count_ = 0;
    return *this;
  }

  static auto Get() { return hwreg::RegisterAddr<WriteData>(kWriteData0Reg); }

 private:
  uint32_t byte_count_ = 0;
};

class ReadData : public hwreg::RegisterBase<ReadData, uint64_t> {
 public:
  static constexpr uint32_t kMaxReadBytesPerTransfer = 8;

  uint8_t Pop() {
    ZX_ASSERT(byte_count_ > 0);
    byte_count_--;
    const uint8_t byte = reg_value() & std::numeric_limits<uint8_t>::max();
    set_reg_value(reg_value() >> std::numeric_limits<uint8_t>::digits);
    return byte;
  }

  template <typename T>
  ReadData& ReadFrom(T* reg_io) {
    const uint64_t lower = reg_io->Read32(reg_addr());
    const uint64_t upper = static_cast<uint64_t>(reg_io->Read32(reg_addr() + sizeof(uint32_t)))
                           << std::numeric_limits<uint32_t>::digits;
    set_reg_value(upper | lower);
    byte_count_ = kMaxReadBytesPerTransfer;
    return *this;
  }

  template <typename T>
  ReadData& WriteTo(T* reg_io) {
    ZX_ASSERT(reg_value() == 0);
    reg_io->Write32(0, reg_addr());
    reg_io->Write32(0, reg_addr() + sizeof(uint32_t));
    return *this;
  }

  static auto Get() { return hwreg::RegisterAddr<ReadData>(kReadData0Reg); }

 private:
  uint32_t byte_count_ = 0;
};

}  // namespace aml_i2c

#endif  // SRC_DEVICES_I2C_DRIVERS_AML_I2C_AML_I2C_REGS_H_
