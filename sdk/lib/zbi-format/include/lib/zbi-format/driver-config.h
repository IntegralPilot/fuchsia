// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library
//   zbi (//sdk/fidl/zbi/driver-config.fidl)
// by zither, a Fuchsia platform tool.

#ifndef LIB_ZBI_FORMAT_DRIVER_CONFIG_H_
#define LIB_ZBI_FORMAT_DRIVER_CONFIG_H_

#include <stdint.h>

#if defined(__cplusplus)
extern "C" {
#endif

// ZBI_TYPE_KERNEL_DRIVER item types (for zbi_header_t.extra)
typedef uint32_t zbi_kernel_driver_t;

// 'PSCI'
#define ZBI_KERNEL_DRIVER_ARM_PSCI ((zbi_kernel_driver_t)(0x49435350u))

// 'PSCS'
#define ZBI_KERNEL_DRIVER_ARM_PSCI_CPU_SUSPEND ((zbi_kernel_driver_t)(0x53435350u))

// 'GIC2'
#define ZBI_KERNEL_DRIVER_ARM_GIC_V2 ((zbi_kernel_driver_t)(0x32434947u))

// 'AIC3'
#define ZBI_KERNEL_DRIVER_APPLE_AIC3 ((zbi_kernel_driver_t)(0x33434941u))

// 'GIC3'
#define ZBI_KERNEL_DRIVER_ARM_GIC_V3 ((zbi_kernel_driver_t)(0x33434947u))

// 'ATIM'
#define ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER ((zbi_kernel_driver_t)(0x4d495441u))

// 'ATMM'
#define ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER_MMIO ((zbi_kernel_driver_t)(0x4d4d5441u))

// 'AS5U'
#define ZBI_KERNEL_DRIVER_APPLE_S5L_UART ((zbi_kernel_driver_t)(0x55355341u))

// 'PL0U'
#define ZBI_KERNEL_DRIVER_PL011_UART ((zbi_kernel_driver_t)(0x55304c50u))

// 'AMLU'
#define ZBI_KERNEL_DRIVER_AMLOGIC_UART ((zbi_kernel_driver_t)(0x554c4d41u))

// 'AMLH'
#define ZBI_KERNEL_DRIVER_AMLOGIC_HDCP ((zbi_kernel_driver_t)(0x484c4d41u))

// 'DW8U'
#define ZBI_KERNEL_DRIVER_DW8250_UART ((zbi_kernel_driver_t)(0x44573855u))

// 'RMLH' (typoed, originally intended to by 'AMLR')
#define ZBI_KERNEL_DRIVER_AMLOGIC_RNG ((zbi_kernel_driver_t)(0x484c4d52u))

// 'WD32'
#define ZBI_KERNEL_DRIVER_GENERIC32_WATCHDOG ((zbi_kernel_driver_t)(0x32334457u))

// 'GENI'
#define ZBI_KERNEL_DRIVER_GENI_UART ((zbi_kernel_driver_t)(0x494E4547u))

// '8250'
#define ZBI_KERNEL_DRIVER_I8250_PIO_UART ((zbi_kernel_driver_t)(0x30353238u))

// '825M'
#define ZBI_KERNEL_DRIVER_I8250_MMIO32_UART ((zbi_kernel_driver_t)(0x4d353238u))

// '825B'
#define ZBI_KERNEL_DRIVER_I8250_MMIO8_UART ((zbi_kernel_driver_t)(0x42353238u))

// 'MMTP'
#define ZBI_KERNEL_DRIVER_MOTMOT_POWER ((zbi_kernel_driver_t)(0x4d4d5450u))

// '370P'
#define ZBI_KERNEL_DRIVER_AS370_POWER ((zbi_kernel_driver_t)(0x50303733u))

// 'MNFP'
#define ZBI_KERNEL_DRIVER_MOONFLOWER_POWER ((zbi_kernel_driver_t)(0x4d4e4650u))

// 'IRSP'
#define ZBI_KERNEL_DRIVER_IRIS_POWER ((zbi_kernel_driver_t)(0x49525350u))

// 'IMXU'
#define ZBI_KERNEL_DRIVER_IMX_UART ((zbi_kernel_driver_t)(0x55584d49u))

// 'PLIC'
#define ZBI_KERNEL_DRIVER_RISCV_PLIC ((zbi_kernel_driver_t)(0x43494C50u))

// 'RTIM'
#define ZBI_KERNEL_DRIVER_RISCV_GENERIC_TIMER ((zbi_kernel_driver_t)(0x4D495452u))

// 'PXAU'
#define ZBI_KERNEL_DRIVER_PXA_UART ((zbi_kernel_driver_t)(0x50584155u))

// 'EXYU'
#define ZBI_KERNEL_DRIVER_EXYNOS_USI_UART ((zbi_kernel_driver_t)(0x45585955u))

// `QRNG`
#define ZBI_KERNEL_DRIVER_QCOM_RNG ((zbi_kernel_driver_t)(0x474E5251u))

// 'SMMU'
#define ZBI_KERNEL_DRIVER_ARM_SMMU ((zbi_kernel_driver_t)(0x554D4D51u))

// 'CPEM'
#define ZBI_KERNEL_DRIVER_CPU_ENERGY_MODEL ((zbi_kernel_driver_t)(0x4d455043u))

// Kernel driver struct that can be used for simple drivers.
// Used by ZBI_KERNEL_DRIVER_PL011_UART, ZBI_KERNEL_DRIVER_AMLOGIC_UART, and
// ZBI_KERNEL_DRIVER_GENI_UART, ZBI_KERNEL_DRIVER_I8250_MMIO_UART.
typedef struct {
  uint64_t mmio_phys;
  uint32_t irq;
  uint32_t flags;
} zbi_dcfg_simple_t;

typedef uint32_t zbi_kernel_driver_irq_flags_t;

// When no flag is set, implies no information was obtained, and the
// kernel will apply default configuration as it sees fit.
#define ZBI_KERNEL_DRIVER_IRQ_FLAGS_EDGE_TRIGGERED ((zbi_kernel_driver_irq_flags_t)(1u << 0))
#define ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED ((zbi_kernel_driver_irq_flags_t)(1u << 1))

// Interpretation depends on whether is edge or level triggered.
// When `LEVEL_TRIGGERED` refers to `ACTIVE_LOW`.
// When `EDGE_TRIGGERED` refers to `HIGH_TO_LOW`.
#define ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_LOW ((zbi_kernel_driver_irq_flags_t)(1u << 2))

// Interpretation depends on whether is edge or level triggered.
// When `LEVEL_TRIGGERED` refers to `ACTIVE_HIGH`.
// When `EDGE_TRIGGERED` refers to `LOW_TO_HIGH`.
#define ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH ((zbi_kernel_driver_irq_flags_t)(1u << 3))

// Used by ZBI_KERNEL_DRIVER_I8250_PIO_UART.
typedef struct {
  uint16_t base;
  uint16_t reserved;
  uint32_t irq;
} zbi_dcfg_simple_pio_t;

// for ZBI_KERNEL_DRIVER_ARM_PSCI
typedef struct {
  uint8_t use_hvc;
  uint8_t reserved[7];
  uint64_t shutdown_args[3];
  uint64_t reboot_args[3];
  uint64_t reboot_bootloader_args[3];
  uint64_t reboot_recovery_args[3];
} zbi_dcfg_arm_psci_driver_t;

// for ZBI_KERNEL_DRIVER_ARM_GIC_V2
typedef struct {
  uint64_t mmio_phys;
  uint64_t msi_frame_phys;
  uint64_t gicd_offset;
  uint64_t gicc_offset;
  uint64_t gich_offset;
  uint64_t gicv_offset;
  uint32_t ipi_base;
  uint8_t optional;
  uint8_t use_msi;
  uint16_t reserved;
} zbi_dcfg_arm_gic_v2_driver_t;

// for ZBI_KERNEL_DRIVER_ARM_GIC_V3
typedef struct {
  uint64_t mmio_phys;
  uint64_t gicd_offset;
  uint64_t gicr_offset;
  uint64_t gicr_stride;
  uint64_t reserved0;
  uint32_t ipi_base;
  uint8_t optional;
  uint8_t reserved1[3];
} zbi_dcfg_arm_gic_v3_driver_t;

// for ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER
typedef struct {
  uint32_t irq_phys;
  uint32_t irq_virt;
  uint32_t irq_sphys;
  uint32_t freq_override;
} zbi_dcfg_arm_generic_timer_driver_t;

typedef struct {
  // Base address for the frame's EL1 view.
  uint64_t mmio_phys_el1;

  // Base address for the frame's EL0 view.
  // This is optional.
  uint64_t mmio_phys_el0;

  // IRQ information for physical timer. This is mandatory.
  uint32_t irq_phys;

  // Same scheme as `DcfgSimple::irq`. This is mandatory.
  uint32_t irq_phys_flags;

  // IRQ information for virtual timer.
  // This is optional.
  // When is not present both `irq_virt` and `irq_virt_flags` will be zero.
  uint32_t irq_virt;

  // Same scheme as `DcfgSimple::irq`.
  uint32_t irq_virt_flags;
} zbi_dcfg_arm_generic_timer_mmio_frame_t;

// for ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER_MMIO
typedef struct {
  // Base address of `CNTCTLBase` frame.
  uint64_t mmio_phys;

  // The frequency of the main counter for the timer.
  uint32_t frequency;

  // Bitmask containing the set of active frames.
  // The `i-th` frame is considered active iff the `i`-th bit is set.
  // Note: While there may be up to 8 frames, both missing and disabled frames are treated
  // as inactive. Disabled frame information will be present, while missing frames will be zeroed.
  uint8_t active_frames_mask;
  uint8_t reserved0[3];

  // Information for each individual frame.
  // Inactive frames must be zero-filled.
  zbi_dcfg_arm_generic_timer_mmio_frame_t frames[8];
} zbi_dcfg_arm_generic_timer_mmio_driver_t;

// for ZBI_KERNEL_DRIVER_AMLOGIC_HDCP
typedef struct {
  uint64_t preset_phys;
  uint64_t hiu_phys;
  uint64_t hdmitx_phys;
} zbi_dcfg_amlogic_hdcp_driver_t;

// for ZBI_KERNEL_DRIVER_AMLOGIC_RNG
typedef struct {
  uint64_t rng_data_phys;
  uint64_t rng_status_phys;
  uint64_t rng_refresh_interval_usec;
} zbi_dcfg_amlogic_rng_driver_t;

// Defines a register write action for a generic kernel watchdog driver.  An
// action consists of the following steps.
//
// 1) Read from the register located a physical address |addr|
// 2) Clear all of the bits in the value which was read using the |clr_mask|
// 3) Set all of the bits in the value using the |set_mask|
// 4) Write this value back to the address located at addr
typedef struct {
  uint64_t addr;
  uint32_t clr_mask;
  uint32_t set_mask;
} zbi_dcfg_generic32_watchdog_action_t;

typedef uint32_t zbi_kernel_driver_generic32_watchdog_flags_t;

#define ZBI_KERNEL_DRIVER_GENERIC32_WATCHDOG_FLAGS_ENABLED \
  ((zbi_kernel_driver_generic32_watchdog_flags_t)(1u << 0))

// 1ms
#define ZBI_KERNEL_DRIVER_GENERIC32_WATCHDOG_MIN_PERIOD ((int64_t)(1000000u))

// Definitions of actions which may be taken by a generic 32 bit watchdog timer
// kernel driver which may be passed by a bootloader.  Field definitions are as
// follows.
typedef struct {
  // The address and masks needed to "pet" (aka, dismiss) a hardware watchdog timer.
  zbi_dcfg_generic32_watchdog_action_t pet_action;

  // The address and masks needed to enable a hardware watchdog timer.  If enable
  // is an unsupported operation, the addr of the |enable_action| shall be zero.
  zbi_dcfg_generic32_watchdog_action_t enable_action;

  // The address and masks needed to disable a hardware watchdog timer.  If
  // disable is an unsupported operation, the addr of the |disable_action| shall
  // be zero.
  zbi_dcfg_generic32_watchdog_action_t disable_action;

  // The period of the watchdog timer given in nanoseconds.  When enabled, the
  // watchdog timer driver must pet the watch dog at least this often.  The value
  // must be at least 1 mSec, typically much larger (on the order of a second or
  // two).
  int64_t watchdog_period_nsec;

  // Storage for additional flags.  Currently, only one flag is defined,
  // "FLAG_ENABLED".  When this flag is set, it indicates that the watchdog timer
  // was left enabled by the bootloader at startup.
  zbi_kernel_driver_generic32_watchdog_flags_t flags;
  uint32_t reserved;
} zbi_dcfg_generic32_watchdog_t;

// for ZBI_KERNEL_DRIVER_RISCV_PLIC
typedef struct {
  // Physical address of the PLIC MMIO region.
  uint64_t mmio_phys;

  // Size in bytes of the PLIC MMIO region.
  uint32_t size_bytes;

  // Number of supported IRQs.
  uint32_t num_irqs;
} zbi_dcfg_riscv_plic_driver_t;

// for ZBI_KERNEL_DRIVER_RISCV_GENERIC_TIMER
typedef struct {
  uint32_t freq_hz;
  uint32_t reserved;
} zbi_dcfg_riscv_generic_timer_driver_t;

typedef uint32_t zbi_arm_psci_cpu_suspend_state_flags_t;

// If set, when entering the associated low power state the CPU's architectural timer will be
// turned off, making it an unsuitable source for exiting the low power state.
// A different source must be programmed.
#define ZBI_ARM_PSCI_CPU_SUSPEND_STATE_FLAGS_LOCAL_TIMER_STOPS \
  ((zbi_arm_psci_cpu_suspend_state_flags_t)(1u << 0))

// If set, the PSCI CPU Suspend operation will affect the entire power domain, implying all other
// CPUs of the power domain must be in a low power mode. That is, the last CPU in the power
// domain is the one to enter this power state.
#define ZBI_ARM_PSCI_CPU_SUSPEND_STATE_FLAGS_TARGETS_POWER_DOMAIN \
  ((zbi_arm_psci_cpu_suspend_state_flags_t)(1u << 1))

// The ZBI_KERNEL_DRIVER_ARM_PSCI_CPU_SUSPEND's payload consists on any number of
// `DcfgArmPsciCpuSuspendState` entries.
//
// The length of the item is `sizeof(zbi_dcfg_arm_psci_cou_suspend_state_t)` times the number of
// entries. Each entry describes an 'idle state' that can be entered through PSCI CPU Suspend call.
//
// Entries in the table may be in any order, and only a single item of type
// ZBI_KERNEL_DRIVER_ARM_PSCI_CPU_SUSPEND should be present in the ZBI.
typedef struct {
  // Unique identifier representing this suspend state.
  uint32_t id;

  // PSCI power_state as described in "Section 5.4.2. of Arm Power State Coordination Interface"
  // v1.3.
  uint32_t power_state;
  zbi_arm_psci_cpu_suspend_state_flags_t flags;

  // Latency in microseconds to enter the low power state.
  uint32_t entry_latency_us;

  // Latency in microseconds to exit the low power state.
  uint32_t exit_latency_us;

  // Minimum time in microseconds, including `entry_latency`, to stay in this low power state.
  // Spending less time would be inefficient energy-wise.
  uint32_t min_residency_us;
} zbi_dcfg_arm_psci_cpu_suspend_state_t;

typedef uint32_t zbi_qcom_rng_flags_t;

// The driver has been handed off, configured and initialized.
#define ZBI_QCOM_RNG_FLAGS_ENABLED ((zbi_qcom_rng_flags_t)(1u << 0))

typedef struct {
  // Base address of the register banks for the device.
  uint64_t mmio_phys;
  zbi_qcom_rng_flags_t flags;
  uint8_t reserved[4];
} zbi_dcfg_qcom_rng_t;

#define ZBI_KERNEL_DRIVER_SMMU_MAX_IRQS ((uint32_t)(128u))

#define ZBI_KERNEL_DRIVER_SMMU_MAX_HANDOFF_SMRS ((uint32_t)(16u))

// The IRQ flags as they would be reported by a device tree description.
typedef uint32_t zbi_arm_smmu_irq_flags_t;

// A rising edge triggered interrupt.
#define ZBI_ARM_SMMU_IRQ_FLAGS_RISING_EDGE ((zbi_arm_smmu_irq_flags_t)(1u << 0))

// A falling edge triggered interrupt.
#define ZBI_ARM_SMMU_IRQ_FLAGS_FALLING_EDGE ((zbi_arm_smmu_irq_flags_t)(1u << 1))

// An active high level triggered interrupt.
#define ZBI_ARM_SMMU_IRQ_FLAGS_ACTIVE_HIGH ((zbi_arm_smmu_irq_flags_t)(1u << 2))

// An active low level triggered interrupt.
#define ZBI_ARM_SMMU_IRQ_FLAGS_ACTIVE_LOW ((zbi_arm_smmu_irq_flags_t)(1u << 3))

typedef struct {
  // The IRQ number reported as an "absolute" GIC interrupt index.  SMMU
  // interrupts are expected to be SPIs, and the SPI range of a GIC's
  // interrupt indices starts at 32.  Because of this, it can be assumed that
  // any interrupt number which is < 32 indicates an invalid interrupt.
  uint32_t num;

  // The IRQ flags as they would be reported by a device tree description.
  zbi_arm_smmu_irq_flags_t flags;
} zbi_dcfg_arm_smmu_irq_t;

typedef struct {
  // Base physical address of the SMMU address space.  This is also the start of
  // Global Register Space 0 in the SMMU.  The effective size of these registers
  // will be determined at runtime from the ID registers present in this first
  // page, specifically the number of context banks in IDR1.NUMCB.
  //
  // See:
  //
  // ARM System Memory Management Unit Architecture Specification
  // SMMU architecture version 2.0
  // ARM IHI 0062D.c
  //
  // for more details.
  uint64_t mmio_phys;

  // The number of context banks that a driver should limit itself to using,
  // or 0 if all discovered context banks are available for use.
  uint32_t num_context_banks_override;

  // The number of stream match registers that a driver should limit itself
  // to using, or 0 if all discovered SMRs are available for use.
  uint32_t num_smr_override;

  // An array of context bank interrupt definitions discovered during device
  // enumeration.  |irq_cnt| reports the total number of valid members while
  // |irqs|, while |global_irq_cnt| reports the total number of global
  // interrupts in |irqs|, with the rest of the interrupts being context bank
  // interrupts.  So:
  //
  // + IRQs [0, global_irq_cnt) are global interrupts
  // + IRQs [global_irq_cnt, irq_cnt) are context bank interrupts
  //   corresponding to context banks [0, irq_cnt - global_irq_cnt)
  uint32_t irq_cnt;
  uint32_t global_irq_cnt;
  zbi_dcfg_arm_smmu_irq_t irqs[128];

  // An array of stream IDs (defined using SMR value/mask encoding) which are
  // being "handed off" from the bootloader to the HLOS.  Downstream drivers
  // should take care to preserve the existing configuration for these stream
  // ids.  |handoff_smr_cnt| reports the number of members of |handoff_smrs|
  // which are valid.
  //
  // Note:  Stream ID ranges in SMRs are encoded using two 16 bit fields, a
  // value field and a mask field.  These two fields are packed into a single
  // 32-bit integer, with the mask packed into the upper 16 bits, and the
  // value packed into the lower 16 bits.
  uint32_t handoff_smr_cnt;
  uint32_t handoff_smrs[16];
} zbi_dcfg_arm_smmu_driver_t;

#define ZBI_KERNEL_DRIVER_CPU_ENERGY_MODEL_MAX_OPPS ((uint32_t)(32u))

// Operating Performance Point (OPP) definition for CPU Energy Model.
typedef struct {
  // CPU frequency in kilohertz (kHz).
  uint32_t frequency_khz;

  // Relative compute capacity scale (unit-less, normalized relative to max capacity across power
  // domains, e.g. 1024 or 1000 scale).
  uint32_t capacity;

  // Power consumption in microwatts (uW).
  uint32_t power_uw;

  // Operating voltage in millivolts (mV).
  uint32_t voltage_mv;
} zbi_cpu_energy_model_opp_t;

// Performance / Power Domain definition for CPU Energy Model.
//
// The ZBI_KERNEL_DRIVER_IRIS_POWER / ZBI_KERNEL_DRIVER_CPU_ENERGY_MODEL payload
// consists of one or more `CpuEnergyModelDomain` entries. The length of the
// item is `sizeof(zbi_cpu_energy_model_domain_t)` times the number of domains.
typedef struct {
  uint64_t cpu_mask;
  uint64_t max_rate;
  uint32_t domain_id;
  uint32_t opp_count;
  zbi_cpu_energy_model_opp_t opps[32];
} zbi_cpu_energy_model_domain_t;

// T8122 AIC3 core MMIO range and event-register offset within that range.
typedef struct {
  uint64_t mmio_phys;
  uint32_t mmio_size;
  uint32_t event_offset;
} zbi_dcfg_apple_aic3_t;

#if defined(__cplusplus)
}
#endif

#endif  // LIB_ZBI_FORMAT_DRIVER_CONFIG_H_
