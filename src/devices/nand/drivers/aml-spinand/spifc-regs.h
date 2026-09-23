// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_NAND_DRIVERS_AML_SPINAND_SPIFC_REGS_H_
#define SRC_DEVICES_NAND_DRIVERS_AML_SPINAND_SPIFC_REGS_H_

#include <zircon/types.h>

#include <hwreg/bitfields.h>

#include "hwreg/internal.h"

namespace amlspinand {

inline constexpr uint32_t kSpifcAhbCtrl = 0x00;
inline constexpr uint32_t kSpifcClkCtrl = 0x04;
inline constexpr uint32_t kSpifcUserCtrl0 = 0x200;
inline constexpr uint32_t kSpifcUserCtrl1 = 0x204;
inline constexpr uint32_t kSpifcUserCtrl2 = 0x208;
inline constexpr uint32_t kSpifcUserCtrl3 = 0x20c;
inline constexpr uint32_t kSpifcUserAddr = 0x210;
inline constexpr uint32_t kSpifcAhbReqCtrl = 0x214;
inline constexpr uint32_t kSpifcActiming0 = 0x220;
inline constexpr uint32_t kSpifcDbufCtrl = 0x240;
inline constexpr uint32_t kSpifcDbufData = 0x244;
inline constexpr uint32_t kSpifcUserDbufAddr = 0x248;
inline constexpr uint32_t kSpifcFlashStatus = 0x280;
inline constexpr uint32_t kSpifcStatus = 0x284;
inline constexpr uint32_t kSpifcCtrl = 0x288;

class AhbCtrl : public hwreg::RegisterBase<AhbCtrl, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_BIT(31, bus_en);
  DEF_BIT(30, decerr_en);
  DEF_BIT(29, force_incr);
  DEF_BIT(19, cwf_en);
  DEF_FIELD(18, 17, rdbuf_size);
  DEF_BIT(16, host_en);
  DEF_BIT(14, clean_buf2);
  DEF_BIT(13, clean_buf1);
  DEF_BIT(12, clean_buf0);

  static auto Get() { return hwreg::RegisterAddr<AhbCtrl>(kSpifcAhbCtrl); }
};

class UserCtrl0 : public hwreg::RegisterBase<UserCtrl0, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_BIT(31, req_en);
  DEF_BIT(30, req_done);
  DEF_BIT(0, data_update);

  static auto Get() { return hwreg::RegisterAddr<UserCtrl0>(kSpifcUserCtrl0); }
};

class UserCtrl1 : public hwreg::RegisterBase<UserCtrl1, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_BIT(30, cmd_cycle_en);
  DEF_FIELD(29, 28, cmd_mode);
  DEF_FIELD(27, 20, cmd_code);
  DEF_BIT(19, addr_cycle_en);
  DEF_FIELD(18, 17, addr_mode);
  DEF_FIELD(16, 15, addr_nbytes);
  DEF_BIT(14, dout_en);
  DEF_BIT(13, dout_aes_en);
  DEF_BIT(12, dout_src);
  DEF_FIELD(11, 10, dout_mode);
  DEF_FIELD(9, 0, dout_nbytes);

  static auto Get() { return hwreg::RegisterAddr<UserCtrl1>(kSpifcUserCtrl1); }
};

class UserCtrl2 : public hwreg::RegisterBase<UserCtrl2, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_BIT(31, placeholder_en);
  DEF_FIELD(30, 29, placeholder_mode);
  DEF_FIELD(28, 23, placeholder_clk_cycle);
  DEF_FIELD(19, 16, cmd_dir);
  DEF_FIELD(15, 8, data_after_first_byte);
  DEF_FIELD(7, 0, first_byte_data);

  static auto Get() { return hwreg::RegisterAddr<UserCtrl2>(kSpifcUserCtrl2); }
};

class UserCtrl3 : public hwreg::RegisterBase<UserCtrl3, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_BIT(31, din_en);
  DEF_BIT(30, din_dest);
  DEF_BIT(29, din_aes_en);
  DEF_FIELD(28, 27, din_mode);
  DEF_FIELD(25, 16, din_nbytes);

  static auto Get() { return hwreg::RegisterAddr<UserCtrl3>(kSpifcUserCtrl3); }
};

class UserAddr : public hwreg::RegisterBase<UserAddr, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_FIELD(31, 0, addr);

  static auto Get() { return hwreg::RegisterAddr<UserAddr>(kSpifcUserAddr); }
};

class AhbReqCtrl : public hwreg::RegisterBase<AhbReqCtrl, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_BIT(31, req_en);
  DEF_BIT(30, cmd_en);
  DEF_FIELD(29, 28, cmd_mode);
  DEF_FIELD(27, 20, cmd_code);
  DEF_BIT(19, addr_cycle_en);
  DEF_FIELD(18, 17, addr_mode);
  DEF_FIELD(16, 15, addr_data_width);
  DEF_FIELD(13, 10, input_switch_time);
  DEF_FIELD(9, 8, din_mode);
  DEF_BIT(7, din_aes_en);
  DEF_FIELD(1, 0, req_data_size);

  static auto Get() { return hwreg::RegisterAddr<AhbReqCtrl>(kSpifcAhbReqCtrl); }
};

class Actiming0 : public hwreg::RegisterBase<Actiming0, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_FIELD(31, 30, tslch);
  DEF_FIELD(29, 28, tclsh);
  DEF_FIELD(20, 16, tshwl);
  DEF_FIELD(15, 12, tshshl2);
  DEF_FIELD(11, 8, tshsl1);
  DEF_FIELD(7, 0, twhsl);

  static auto Get() { return hwreg::RegisterAddr<Actiming0>(kSpifcActiming0); }
};

class DbufCtrl : public hwreg::RegisterBase<DbufCtrl, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_BIT(31, rw);
  DEF_BIT(30, auto_update_addr);
  DEF_FIELD(7, 0, dbuf_addr);

  static auto Get() { return hwreg::RegisterAddr<DbufCtrl>(kSpifcDbufCtrl); }
};

class DbufData : public hwreg::RegisterBase<DbufData, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_FIELD(31, 0, dbuf_data);

  static auto Get() { return hwreg::RegisterAddr<DbufData>(kSpifcDbufData); }
};

class UserDbufAddr : public hwreg::RegisterBase<UserDbufAddr, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_FIELD(7, 0, dbuf_offset);

  static auto Get() { return hwreg::RegisterAddr<UserDbufAddr>(kSpifcUserDbufAddr); }
};

inline constexpr uint32_t kClkTreeSpifcClkCtrl = 0x0;
class ClkCtrl : public hwreg::RegisterBase<ClkCtrl, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_FIELD(10, 9, mux_sel);
  DEF_BIT(8, gate_en);
  DEF_FIELD(7, 0, div);

  static auto Get() { return hwreg::RegisterAddr<ClkCtrl>(kClkTreeSpifcClkCtrl); }
};

}  // namespace amlspinand

#endif  // SRC_DEVICES_NAND_DRIVERS_AML_SPINAND_SPIFC_REGS_H_
