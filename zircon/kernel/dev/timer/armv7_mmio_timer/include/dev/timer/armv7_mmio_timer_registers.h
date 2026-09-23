// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_TIMER_ARMV7_MMIO_TIMER_INCLUDE_DEV_TIMER_ARMV7_MMIO_TIMER_REGISTERS_H_
#define ZIRCON_KERNEL_DEV_TIMER_ARMV7_MMIO_TIMER_INCLUDE_DEV_TIMER_ARMV7_MMIO_TIMER_REGISTERS_H_

#include <hwreg/bitfields.h>
#include <hwreg/mmio.h>

#if true
using EnablePrinting = ::hwreg::EnablePrinter;
#else
using EnablePrinting = void;
#endif

namespace armv7_mmio_timer_registers {

// Section I2.3.1 of the Arm Architecture Reference Manual ARM DDI 0487K.a
namespace CNTCTLBase {

// Section I6.7.7
class CNTFRQ : public hwreg::RegisterBase<CNTFRQ, uint32_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x00;
  static auto Get() { return hwreg::RegisterAddr<CNTFRQ>(kAddr); }
};

// Section I6.7.9
class CNTNSAR : public hwreg::RegisterBase<CNTNSAR, uint32_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x04;
  static auto Get() { return hwreg::RegisterAddr<CNTNSAR>(kAddr); }

  DEF_BIT(0, NS0);
  DEF_BIT(1, NS1);
  DEF_BIT(2, NS2);
  DEF_BIT(3, NS3);
  DEF_BIT(4, NS4);
  DEF_BIT(5, NS5);
  DEF_BIT(6, NS6);
  DEF_BIT(7, NS7);
};

// Section I6.7.16
class CNTTIDR : public hwreg::RegisterBase<CNTTIDR, uint32_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x08;
  static auto Get() { return hwreg::RegisterAddr<CNTTIDR>(kAddr); }

  uint32_t bits(uint32_t ndx) const { return (reg_value() >> (ndx * 4)) & 0xF; }
  bool implemented(uint32_t ndx) const { return (bits(ndx) & 0x1) != 0; }
  bool virt_impl(uint32_t ndx) const { return (bits(ndx) & 0x2) != 0; }
  bool el0_impl(uint32_t ndx) const { return (bits(ndx) & 0x2) != 0; }

  DEF_FIELD(3, 0, Frame0);
  DEF_FIELD(7, 4, Frame1);
  DEF_FIELD(11, 8, Frame2);
  DEF_FIELD(15, 12, Frame3);
  DEF_FIELD(19, 16, Frame4);
  DEF_FIELD(23, 20, Frame5);
  DEF_FIELD(27, 24, Frame6);
  DEF_FIELD(31, 28, Frame7);
};

// Section I6.7.1
class CNTACR : public hwreg::RegisterBase<CNTACR, uint32_t, EnablePrinting> {
 public:
  static constexpr uint32_t kRegCount = 8;
  static constexpr uint32_t kAddr = 0x40;
  static auto Get(uint32_t ndx) { return hwreg::RegisterAddr<CNTACR>(kAddr + (ndx << 2)); }

  DEF_BIT(0, RPCT);
  DEF_BIT(1, RVCT);
  DEF_BIT(2, RFRQ);
  DEF_BIT(3, RVOFF);
  DEF_BIT(4, RWVT);
  DEF_BIT(5, RWPT);
};

// Section I6.7.22
class CNTVOFF : public hwreg::RegisterBase<CNTVOFF, uint64_t, EnablePrinting> {
 public:
  static constexpr uint32_t kRegCount = 8;
  static constexpr uint32_t kAddr = 0x80;
  static auto Get(uint32_t ndx) { return hwreg::RegisterAddr<CNTVOFF>(kAddr + (ndx << 3)); }
};

// Section I6.7.23
class CounterID : public hwreg::RegisterBase<CounterID, uint32_t, EnablePrinting> {
 public:
  static constexpr uint32_t kRegCount = 12;
  static constexpr uint32_t kAddr = 0xFD0;
  static auto Get(uint32_t ndx) { return hwreg::RegisterAddr<CounterID>(kAddr + (ndx << 2)); }
};

}  // namespace CNTCTLBase

// Section I2.3.2 of the Arm Architecture Reference Manual ARM DDI 0487K.a
namespace CNTBase {

// Section I6.7.13
class CNTPCT : public hwreg::RegisterBase<CNTPCT, uint64_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x00;
  static auto Get() { return hwreg::RegisterAddr<CNTPCT>(kAddr); }
};

// Section I6.7.20
class CNTVCT : public hwreg::RegisterBase<CNTVCT, uint64_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x08;
  static auto Get() { return hwreg::RegisterAddr<CNTVCT>(kAddr); }
};

// Section I6.7.7
class CNTFRQ : public hwreg::RegisterBase<CNTFRQ, uint32_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x10;
  static auto Get() { return hwreg::RegisterAddr<CNTFRQ>(kAddr); }
};

// Section I6.7.4
class CNTEL0ACR : public hwreg::RegisterBase<CNTEL0ACR, uint32_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x14;
  static auto Get() { return hwreg::RegisterAddr<CNTEL0ACR>(kAddr); }
  DEF_BIT(0, EL0PCTEN);
  DEF_BIT(1, EL0VCTEN);
  DEF_BIT(8, EL0VTEN);
  DEF_BIT(9, EL0PTEN);
};

// Section I6.7.21
class CNTVOFF : public hwreg::RegisterBase<CNTVOFF, uint64_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x18;
  static auto Get() { return hwreg::RegisterAddr<CNTVOFF>(kAddr); }
};

// Section I6.7.11
class CNTP_CVAL : public hwreg::RegisterBase<CNTP_CVAL, uint64_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x20;
  static auto Get() { return hwreg::RegisterAddr<CNTP_CVAL>(kAddr); }
};

// Section I6.7.7
class CNTP_TVAL : public hwreg::RegisterBase<CNTP_TVAL, uint32_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x28;
  static auto Get() { return hwreg::RegisterAddr<CNTP_TVAL>(kAddr); }
};

// Section I6.7.10
class CNTP_CTL : public hwreg::RegisterBase<CNTP_CTL, uint32_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x2c;
  static auto Get() { return hwreg::RegisterAddr<CNTP_CTL>(kAddr); }

  DEF_BIT(0, ENABLE);
  DEF_BIT(1, IMASK);
  DEF_BIT(2, ISTATUS);
};

// Section I6.7.18
class CNTV_CVAL : public hwreg::RegisterBase<CNTV_CVAL, uint64_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x30;
  static auto Get() { return hwreg::RegisterAddr<CNTV_CVAL>(kAddr); }
};

// Section I6.7.19
class CNTV_TVAL : public hwreg::RegisterBase<CNTV_TVAL, uint32_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x38;
  static auto Get() { return hwreg::RegisterAddr<CNTV_TVAL>(kAddr); }
};

// Section I6.7.17
class CNTV_CTL : public hwreg::RegisterBase<CNTV_CTL, uint32_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x3c;
  static auto Get() { return hwreg::RegisterAddr<CNTV_CTL>(kAddr); }

  DEF_BIT(0, ENABLE);
  DEF_BIT(1, IMASK);
  DEF_BIT(2, ISTATUS);
};

// Section I6.7.23
class CounterID : public hwreg::RegisterBase<CounterID, uint32_t, EnablePrinting> {
 public:
  static constexpr uint32_t kRegCount = 12;
  static constexpr uint32_t kAddr = 0xFD0;
  static auto Get(uint32_t ndx) { return hwreg::RegisterAddr<CounterID>(kAddr + (ndx << 2)); }
};

}  // namespace CNTBase

// Section I2.3.2 of the Arm Architecture Reference Manual ARM DDI 0487K.a
namespace CNTEL0Base {

// Section I6.7.13
class CNTPCT : public hwreg::RegisterBase<CNTPCT, uint64_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x00;
  static auto Get() { return hwreg::RegisterAddr<CNTPCT>(kAddr); }
};

// Section I6.7.20
class CNTVCT : public hwreg::RegisterBase<CNTVCT, uint64_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x08;
  static auto Get() { return hwreg::RegisterAddr<CNTVCT>(kAddr); }
};

// Section I6.7.7
class CNTFRQ : public hwreg::RegisterBase<CNTFRQ, uint32_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x10;
  static auto Get() { return hwreg::RegisterAddr<CNTFRQ>(kAddr); }
};

// Section I6.7.11
class CNTP_CVAL : public hwreg::RegisterBase<CNTP_CVAL, uint64_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x20;
  static auto Get() { return hwreg::RegisterAddr<CNTP_CVAL>(kAddr); }
};

// Section I6.7.7
class CNTP_TVAL : public hwreg::RegisterBase<CNTP_TVAL, uint32_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x28;
  static auto Get() { return hwreg::RegisterAddr<CNTP_TVAL>(kAddr); }
};

// Section I6.7.10
class CNTP_CTL : public hwreg::RegisterBase<CNTP_CTL, uint32_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x2c;
  static auto Get() { return hwreg::RegisterAddr<CNTP_CTL>(kAddr); }

  DEF_BIT(0, ENABLE);
  DEF_BIT(1, IMASK);
  DEF_BIT(2, ISTATUS);
};

// Section I6.7.18
class CNTV_CVAL : public hwreg::RegisterBase<CNTV_CVAL, uint64_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x30;
  static auto Get() { return hwreg::RegisterAddr<CNTV_CVAL>(kAddr); }
};

// Section I6.7.19
class CNTV_TVAL : public hwreg::RegisterBase<CNTV_TVAL, uint32_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x38;
  static auto Get() { return hwreg::RegisterAddr<CNTV_TVAL>(kAddr); }
};

// Section I6.7.17
class CNTV_CTL : public hwreg::RegisterBase<CNTV_CTL, uint32_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x3c;
  static auto Get() { return hwreg::RegisterAddr<CNTV_CTL>(kAddr); }

  DEF_BIT(0, ENABLE);
  DEF_BIT(1, IMASK);
  DEF_BIT(2, ISTATUS);
};

// Section I6.7.23
class CounterID : public hwreg::RegisterBase<CounterID, uint32_t, EnablePrinting> {
 public:
  static constexpr uint32_t kRegCount = 12;
  static constexpr uint32_t kAddr = 0xFD0;
  static auto Get(uint32_t ndx) { return hwreg::RegisterAddr<CounterID>(kAddr + (ndx << 2)); }
};

}  // namespace CNTEL0Base

// In each of the timer frames, there are two sets of identical registers at
// different offsets:
//
// 1) a compare register
// 2) a "timer" (countdown) register, and
// 3) a control register.
//
// One set of these registers operates against the PCT counter, while the other
// operates against the VCT counter.  Since they are at different offsets, they
// need to have different names and different offsets encoded in their `hwreg`
// type.
//
// This binding of two otherwise identical sets of registers to different types
// (differing only by offset) makes is a bit difficult to write a single generic
// "timer control" class, forcing us to template the class to account for the
// type differences.
//
// Instead of doing this, we go ahead give a generic definition which can be
// used for either the PCT or VCT timer registers starting at offset zero,
// allowing us to handle the offset at runtime via the `hwreg::RegisterMmio`
// class, allowing us to have a single type for the timer hardware, regardless
// of whether it is the PCT or VCT hardware.
namespace CNTPVRegs {

// Section I6.7.11 (PCT) or Section I6.7.18 (VCT)
class CVAL : public hwreg::RegisterBase<CVAL, uint64_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x00;
  static auto Get() { return hwreg::RegisterAddr<CVAL>(kAddr); }
};

// Section I6.7.7 (PCT) or Section I6.7.19 (VCT)
class TVAL : public hwreg::RegisterBase<TVAL, uint32_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x08;
  static auto Get() { return hwreg::RegisterAddr<TVAL>(kAddr); }
};

// Section I6.7.10 (PCT) or Section I6.7.17 (VCT)
class CTL : public hwreg::RegisterBase<CTL, uint32_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x0c;
  static auto Get() { return hwreg::RegisterAddr<CTL>(kAddr); }

  DEF_BIT(0, ENABLE);
  DEF_BIT(1, IMASK);
  DEF_BIT(2, ISTATUS);
};

// Likewise, the counter itself is at offset 0x0 for the PCT counter, and 0x8
// for the VCT counter. Define a generic version so we can handle the difference
// at runtime a bit more easily.
//
// Section I6.7.13 (PCT) or Section I6.7.20 (VCT)
class CT : public hwreg::RegisterBase<CT, uint64_t, EnablePrinting> {
 public:
  static constexpr uint32_t kAddr = 0x00;
  static auto Get() { return hwreg::RegisterAddr<CT>(kAddr); }
};

}  // namespace CNTPVRegs

}  // namespace armv7_mmio_timer_registers

#endif  // ZIRCON_KERNEL_DEV_TIMER_ARMV7_MMIO_TIMER_INCLUDE_DEV_TIMER_ARMV7_MMIO_TIMER_REGISTERS_H_
