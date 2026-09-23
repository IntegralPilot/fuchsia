// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_INTERRUPT_GIC_V3_ARM_GICV3_REGS_H_
#define ZIRCON_KERNEL_DEV_INTERRUPT_GIC_V3_ARM_GICV3_REGS_H_

#include <lib/arch/intrin.h>
#include <lib/mmio-ptr/mmio-ptr.h>

#include <arch/arm64.h>

// Accessors for all of the GICv3 registers if configured.
// The registers are mapped in to separate mmio mappings:
// arm_gicv3_gicd_base for all of the distributor registers,
// arm_gicv3_gicr_base for all of the redistributor registers (per cpu).

// exported from arm_gicv3.cc
extern vaddr_t arm_gicv3_gicd_base;
extern vaddr_t arm_gicv3_gicr_base;
extern uint64_t arm_gicv3_gicr_stride;

#define BIT_32(bit) (1u << (bit))
#define BIT_64(bit) (1ul << (bit))

// TODO: break into separate D and R accessor functions to be a little safer.
template <typename T>
inline MMIO_PTR volatile T* arm_gicv3_reg(uint64_t reg) {
  return reinterpret_cast<MMIO_PTR volatile T*>(reg);
}

inline uint32_t arm_gicv3_read32(uint64_t reg) { return MmioRead32(arm_gicv3_reg<uint32_t>(reg)); }

inline void arm_gicv3_write32(uint64_t reg, uint32_t value) {
  MmioWrite32(value, arm_gicv3_reg<uint32_t>(reg));
}

inline uint64_t arm_gicv3_read64(uint64_t reg) { return MmioRead64(arm_gicv3_reg<uint64_t>(reg)); }

inline void arm_gicv3_write64(uint64_t reg, uint64_t value) {
  MmioWrite64(value, arm_gicv3_reg<uint64_t>(reg));
}

#define GICR_STRIDE arm_gicv3_gicr_stride
#define GICD_REG_SIZE (0x10000)

#define GICD_BASE arm_gicv3_gicd_base
#define GICR_BASE arm_gicv3_gicr_base

// CPU interface system registers.

#define ICC_CTLR_EL1 "S3_0_C12_C12_4"
#define ICC_PMR_EL1 "S3_0_C4_C6_0"
#define ICC_IAR1_EL1 "S3_0_C12_C12_0"
#define ICC_SRE_EL1 "S3_0_C12_C12_5"
#define ICC_BPR1_EL1 "S3_0_C12_C12_3"
#define ICC_IGRPEN1_EL1 "S3_0_C12_C12_7"
#define ICC_EOIR1_EL1 "S3_0_C12_C12_1"
#define ICC_DIR_EL1 "S3_0_C12_C11_1"
#define ICC_SGI1R_EL1 "S3_0_C12_C11_5"

// Distributor registers.

#define GICD_CTLR (GICD_BASE + 0x0000)
#define GICD_TYPER (GICD_BASE + 0x0004)
#define GICD_IIDR (GICD_BASE + 0x0008)
#define GICD_IGROUPR(n) (GICD_BASE + 0x0080 + (n) * 4)
#define GICD_ISENABLER(n) (GICD_BASE + 0x0100 + (n) * 4)
#define GICD_ICENABLER(n) (GICD_BASE + 0x0180 + (n) * 4)
#define GICD_ISPENDR(n) (GICD_BASE + 0x0200 + (n) * 4)
#define GICD_ICPENDR(n) (GICD_BASE + 0x0280 + (n) * 4)
#define GICD_ISACTIVER(n) (GICD_BASE + 0x0300 + (n) * 4)
#define GICD_ICACTIVER(n) (GICD_BASE + 0x0380 + (n) * 4)
#define GICD_IPRIORITYR(n) (GICD_BASE + 0x0400 + (n) * 4)
#define GICD_ITARGETSR(n) (GICD_BASE + 0x0800 + (n) * 4)
#define GICD_ICFGR(n) (GICD_BASE + 0x0c00 + (n) * 4)
#define GICD_IGRPMODR(n) (GICD_BASE + 0x0d00 + (n) * 4)
#define GICD_NSACR(n) (GICD_BASE + 0x0e00 + (n) * 4)
#define GICD_SGIR (GICD_BASE + 0x0f00)
#define GICD_CPENDSGIR(n) (GICD_BASE + 0x0f10 + (n) * 4)
#define GICD_SPENDSGIR(n) (GICD_BASE + 0x0f20 + (n) * 4)
#define GICD_IROUTER(n) (GICD_BASE + 0x6000 + (n) * 8)

// GICD_CTLR bit definitions.

#define CTLR_ENABLE_G0 BIT_32(0)
#define CTLR_ENABLE_G1NS BIT_32(1)
#define CTLR_ENABLE_G1S BIT_32(2)
#define CTLR_RES0 BIT_32(3)
#define CTLR_ARE_S BIT_32(4)
#define CTLR_ARE_NS BIT_32(5)
#define CTLR_DS BIT_32(6)
#define CTLR_E1NWF BIT_32(7)
#define GICD_CTLR_RWP BIT_32(31)

// Peripheral identification registers.

#define GICD_CIDR0 (GICD_BASE + 0xfff0)
#define GICD_CIDR1 (GICD_BASE + 0xfff4)
#define GICD_CIDR2 (GICD_BASE + 0xfff8)
#define GICD_CIDR3 (GICD_BASE + 0xfffc)
#define GICD_PIDR0 (GICD_BASE + 0xffe0)
#define GICD_PIDR1 (GICD_BASE + 0xffe4)
#define GICD_PIDR2 (GICD_BASE + 0xffe8)
#define GICD_PIDR3 (GICD_BASE + 0xffec)

// GICD_PIDR bit definitions and masks.

#define GICD_PIDR2_ARCHREV_SHIFT 4
#define GICD_PIDR2_ARCHREV_MASK 0xf

// GICR_WAKER bit definitions and masks.

#define WAKER_CHILDREN_ASLEEP BIT_32(2)
#define WAKER_PROCESSOR_SLEEP BIT_32(1)

// Redistributor registers.
// Arranged as 2 or 3 banks (+1 empty bank) of 64KiB depending if it's
// GICv3 or GICv4.
#define GICR_RD_BASE (GICR_BASE)
#define GICR_SGI_BASE (GICR_BASE + 0x10000)
#define GICR_VLPI_BASE (GICR_BASE + 0x20000)  // GICv4 only

#define GICR_CTLR(i) (GICR_RD_BASE + GICR_STRIDE * (i) + 0x0000)
#define GICR_IIDR(i) (GICR_RD_BASE + GICR_STRIDE * (i) + 0x0004)
#define GICR_TYPER(i, n) (GICR_RD_BASE + GICR_STRIDE * (i) + 0x0008 + (n) * 4)
#define GICR_STATUSR(i) (GICR_RD_BASE + GICR_STRIDE * (i) + 0x0010)
#define GICR_WAKER(i) (GICR_RD_BASE + GICR_STRIDE * (i) + 0x0014)
#define GICR_IGROUPR0(i) (GICR_SGI_BASE + GICR_STRIDE * (i) + 0x0080)
#define GICR_IGRPMOD0(i) (GICR_SGI_BASE + GICR_STRIDE * (i) + 0x0d00)
#define GICR_ISENABLER0(i) (GICR_SGI_BASE + GICR_STRIDE * (i) + 0x0100)
#define GICR_ICENABLER0(i) (GICR_SGI_BASE + GICR_STRIDE * (i) + 0x0180)
#define GICR_ISPENDR0(i) (GICR_SGI_BASE + GICR_STRIDE * (i) + 0x0200)
#define GICR_ICPENDR0(i) (GICR_SGI_BASE + GICR_STRIDE * (i) + 0x0280)
#define GICR_ISACTIVER0(i) (GICR_SGI_BASE + GICR_STRIDE * (i) + 0x0300)
#define GICR_ICACTIVER0(i) (GICR_SGI_BASE + GICR_STRIDE * (i) + 0x0380)
#define GICR_IPRIORITYR0(i) (GICR_SGI_BASE + GICR_STRIDE * (i) + 0x0400)
#define GICR_ICFGR0(i) (GICR_SGI_BASE + GICR_STRIDE * (i) + 0x0c00)
#define GICR_ICFGR1(i) (GICR_SGI_BASE + GICR_STRIDE * (i) + 0x0c04)
#define GICR_NSACR(i) (GICR_SGI_BASE + GICR_STRIDE * (i) + 0x0e00)

static inline void gic_write_ctlr(uint32_t val) {
  __asm__ volatile("msr " ICC_CTLR_EL1 ", %0" ::"r"(static_cast<uint64_t>(val)));
  __isb(ARM_MB_SY);
}

static inline void gic_write_pmr(uint32_t val) {
  __asm__ volatile("msr " ICC_PMR_EL1 ", %0" ::"r"(static_cast<uint64_t>(val)));
  __isb(ARM_MB_SY);
  arch::DeviceMemoryBarrier();
}

static inline void gic_write_igrpen(uint32_t val) {
  __asm__ volatile("msr " ICC_IGRPEN1_EL1 ", %0" ::"r"(static_cast<uint64_t>(val)));
  __isb(ARM_MB_SY);
}

static inline uint32_t gic_read_sre() {
  uint64_t temp;
  __asm__ volatile("mrs %0, " ICC_SRE_EL1 : "=r"(temp));
  return static_cast<uint32_t>(temp);
}

static inline void gic_write_sre(uint32_t val) {
  __asm__ volatile("msr " ICC_SRE_EL1 ", %0" ::"r"(static_cast<uint64_t>(val)));
  __isb(ARM_MB_SY);
}

static inline void gic_write_eoir(uint32_t val) {
  __asm__ volatile("msr " ICC_EOIR1_EL1 ", %0" ::"r"(static_cast<uint64_t>(val)));
  __isb(ARM_MB_SY);
}

static inline void gic_write_dir(uint32_t val) {
  __asm__ volatile("msr " ICC_DIR_EL1 ", %0" ::"r"(static_cast<uint64_t>(val)));
  __isb(ARM_MB_SY);
}

static inline uint32_t gic_read_iar() {
  uint64_t temp;
  __asm__ volatile("mrs %0, " ICC_IAR1_EL1 : "=r"(temp));
  arch::DeviceMemoryBarrier();
  return static_cast<uint32_t>(temp);
}

static inline void gic_write_sgi1r(uint64_t val) {
  __asm__ volatile("msr " ICC_SGI1R_EL1 ", %0" ::"r"(val));
  __isb(ARM_MB_SY);
  arch::DeviceMemoryBarrier();
}

#endif  // ZIRCON_KERNEL_DEV_INTERRUPT_GIC_V3_ARM_GICV3_REGS_H_
