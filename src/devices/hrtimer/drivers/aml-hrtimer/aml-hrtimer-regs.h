// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_HRTIMER_DRIVERS_AML_HRTIMER_AML_HRTIMER_REGS_H_
#define SRC_DEVICES_HRTIMER_DRIVERS_AML_HRTIMER_AML_HRTIMER_REGS_H_

#include <hwreg/bitfields.h>

namespace hrtimer {

// ISA_TIMER_MUX.
struct IsaTimerMux : public hwreg::RegisterBase<IsaTimerMux, uint32_t> {
  DEF_BIT(19, TIMERD_EN);
  DEF_BIT(18, TIMERC_EN);
  DEF_BIT(17, TIMERB_EN);
  DEF_BIT(16, TIMERA_EN);
  DEF_BIT(15, TIMERD_MODE);
  DEF_BIT(14, TIMERC_MODE);
  DEF_BIT(13, TIMERB_MODE);
  DEF_BIT(12, TIMERA_MODE);
  DEF_FIELD(10, 8, TIMERE_input_clock_selection);  // one more bit that the others.
  DEF_FIELD(7, 6, TIMERD_input_clock_selection);
  DEF_FIELD(5, 4, TIMERC_input_clock_selection);
  DEF_FIELD(3, 2, TIMERB_input_clock_selection);
  DEF_FIELD(1, 0, TIMERA_input_clock_selection);

  static auto Get() { return hwreg::RegisterAddr<IsaTimerMux>(0x3c50 * 4); }
};

// ISA_TIMERA.
struct IsaTimerA : public hwreg::RegisterBase<IsaTimerA, uint32_t> {
  DEF_FIELD(31, 16, current_count_value);
  DEF_FIELD(15, 0, starting_count_value);

  static auto Get() { return hwreg::RegisterAddr<IsaTimerA>(0x3c51 * 4); }
};

// ISA_TIMERB.
struct IsaTimerB : public hwreg::RegisterBase<IsaTimerB, uint32_t> {
  DEF_FIELD(31, 16, current_count_value);
  DEF_FIELD(15, 0, starting_count_value);

  static auto Get() { return hwreg::RegisterAddr<IsaTimerB>(0x3c52 * 4); }
};

// ISA_TIMERC.
struct IsaTimerC : public hwreg::RegisterBase<IsaTimerC, uint32_t> {
  DEF_FIELD(31, 16, current_count_value);
  DEF_FIELD(15, 0, starting_count_value);

  static auto Get() { return hwreg::RegisterAddr<IsaTimerC>(0x3c53 * 4); }
};

// ISA_TIMERD.
struct IsaTimerD : public hwreg::RegisterBase<IsaTimerD, uint32_t> {
  DEF_FIELD(31, 16, current_count_value);
  DEF_FIELD(15, 0, starting_count_value);

  static auto Get() { return hwreg::RegisterAddr<IsaTimerD>(0x3c54 * 4); }
};

// ISA_TIMERE.
struct IsaTimerE : public hwreg::RegisterBase<IsaTimerE, uint32_t> {
  // No starting count.
  DEF_FIELD(31, 0, current_count_value);

  // There is a register gap from TIMERD.
  static auto Get() { return hwreg::RegisterAddr<IsaTimerE>(0x3c62 * 4); }
};

// ISA_TIMERE_HI.
struct IsaTimerEHi : public hwreg::RegisterBase<IsaTimerEHi, uint32_t> {
  DEF_FIELD(31, 0, current_count_value);

  static auto Get() { return hwreg::RegisterAddr<IsaTimerEHi>(0x3c63 * 4); }
};

// ISA_TIMER_MUX1.
struct IsaTimerMux1 : public hwreg::RegisterBase<IsaTimerMux1, uint32_t> {
  DEF_BIT(19, TIMERI_EN);
  DEF_BIT(18, TIMERH_EN);
  DEF_BIT(17, TIMERG_EN);
  DEF_BIT(16, TIMERF_EN);
  DEF_BIT(15, TIMERI_MODE);
  DEF_BIT(14, TIMERH_MODE);
  DEF_BIT(13, TIMERG_MODE);
  DEF_BIT(12, TIMERF_MODE);
  DEF_FIELD(7, 6, TIMERI_input_clock_selection);
  DEF_FIELD(5, 4, TIMERH_input_clock_selection);
  DEF_FIELD(3, 2, TIMERG_input_clock_selection);
  DEF_FIELD(1, 0, TIMERF_input_clock_selection);

  static auto Get() { return hwreg::RegisterAddr<IsaTimerMux1>(0x3c64 * 4); }
};

// ISA_TIMERF.
struct IsaTimerF : public hwreg::RegisterBase<IsaTimerF, uint32_t> {
  DEF_FIELD(31, 16, current_count_value);
  DEF_FIELD(15, 0, starting_count_value);

  static auto Get() { return hwreg::RegisterAddr<IsaTimerF>(0x3c65 * 4); }
};

// ISA_TIMERG.
struct IsaTimerG : public hwreg::RegisterBase<IsaTimerG, uint32_t> {
  DEF_FIELD(31, 16, current_count_value);
  DEF_FIELD(15, 0, starting_count_value);

  static auto Get() { return hwreg::RegisterAddr<IsaTimerG>(0x3c66 * 4); }
};

// ISA_TIMERH.
struct IsaTimerH : public hwreg::RegisterBase<IsaTimerH, uint32_t> {
  DEF_FIELD(31, 16, current_count_value);
  DEF_FIELD(15, 0, starting_count_value);

  static auto Get() { return hwreg::RegisterAddr<IsaTimerH>(0x3c67 * 4); }
};

// ISA_TIMERI.
struct IsaTimerI : public hwreg::RegisterBase<IsaTimerI, uint32_t> {
  DEF_FIELD(31, 16, current_count_value);
  DEF_FIELD(15, 0, starting_count_value);

  static auto Get() { return hwreg::RegisterAddr<IsaTimerI>(0x3c68 * 4); }
};

}  // namespace hrtimer

#endif  // SRC_DEVICES_HRTIMER_DRIVERS_AML_HRTIMER_AML_HRTIMER_REGS_H_
