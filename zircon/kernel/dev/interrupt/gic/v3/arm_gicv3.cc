// Copyright 2017 The Fuchsia Authors
// Copyright (c) 2017, Google Inc. All rights reserved.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <assert.h>
#include <bits.h>
#include <inttypes.h>
#include <lib/arch/intrin.h>
#include <lib/ktrace.h>
#include <lib/root_resource_filter.h>
#include <lib/zbi-format/driver-config.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <arch/arm64/hypervisor/gic/gicv3.h>
#include <arch/regs.h>
#include <dev/interrupt.h>
#include <dev/interrupt/arm_gic_common.h>
#include <dev/interrupt/arm_gic_hw_interface.h>
#include <dev/interrupt/arm_gicv3_init.h>
#include <kernel/cpu.h>
#include <kernel/stats.h>
#include <kernel/thread.h>
#include <lk/init.h>
#include <pdev/interrupt.h>
#include <vm/vm.h>
#include <vm/vm_aspace.h>

#include "arm_gicv3_regs.h"

#define LOCAL_TRACE 0

#include <arch/arm64.h>
#define IFRAME_PC(frame) ((frame)->elr)

// Base and stride of the mapped apertures, exported to other files in this driver.
vaddr_t arm_gicv3_gicd_base = 0;
vaddr_t arm_gicv3_gicr_base = 0;
uint64_t arm_gicv3_gicr_stride = 0;

namespace {

uint64_t gicd_offset = 0;
uint64_t gicr_offset = 0;
uint64_t gic_mmio_phys = 0;
uint32_t ipi_base = 0;
uint32_t gic_max_int;

bool gic_is_valid_interrupt(interrupt_vector_t vector, uint32_t flags) {
  return (vector < gic_max_int);
}

uint32_t gic_get_base_vector() {
  // ARM Generic Interrupt Controller v3&4 chapter 2.2
  // INTIDs 0-15 are local CPU interrupts
  return 16;
}

uint32_t gic_get_max_vector() { return gic_max_int; }

bool gic_wait_for_mask(uint64_t reg, uint64_t mask, uint64_t expect) {
  int count = 1000000;
  while ((arm_gicv3_read32(reg) & mask) != expect) {
    count -= 1;
    if (!count) {
      LTRACEF("arm_gicv3: wait timeout reg:0x%lx, val:0x%x, mask:0x%lx\n", reg,
              arm_gicv3_read32(reg), mask);
      return false;
    }
  }
  return true;
}

void gic_wait_for_rwp(uint64_t reg) {
  // Maintain the current log message for RWP timeouts.
  if (!gic_wait_for_mask(reg, GICD_CTLR_RWP, 0)) {
    LTRACEF("arm_gicv3: rwp timeout 0x%x\n", arm_gicv3_read32(reg));
  }
}

void gic_set_enable(uint vector, bool enable) {
  uint32_t reg = vector / 32;
  uint32_t mask = static_cast<uint32_t>(1ULL << (vector % 32));
  if (vector < 32) {
    cpu_num_t cpu_id = arch_curr_cpu_num();
    if (enable) {
      arm_gicv3_write32(GICR_ISENABLER0(cpu_id), mask);
    } else {
      arm_gicv3_write32(GICR_ICENABLER0(cpu_id), mask);
    }
    gic_wait_for_rwp(GICR_CTLR(cpu_id));
  } else {
    if (enable) {
      arm_gicv3_write32(GICD_ISENABLER(reg), mask);
    } else {
      arm_gicv3_write32(GICD_ICENABLER(reg), mask);
    }
    gic_wait_for_rwp(GICD_CTLR);
  }
}

// Redistributors for each PE need to woken up before they will
// distribute interrupts.
// https://developer.arm.com/documentation/198123/0302/Configuring-the-Arm-GIC
void gic_redistributor_sleep(bool sleep) {
  cpu_num_t cpu = arch_curr_cpu_num();
  DEBUG_ASSERT(arch_ints_disabled());

  // GICR_WAKER could be RW or RAZ/WI.  When GICD_CTLR.DS is 1, GICR_WAKER is
  // RW.  However, when GICD_CTLR.DS is 0, GICR_WAKER could be RW or RAZ/WI
  // depending on whether the access is Secure/Non-secure and FEAT_RME.
  //
  // Instead of checking those things we're going to take a shortcut.  In the
  // case we're writing a 1 to WAKER_PROCESSOR_SLEEP we'll read back GICR_WAKER
  // to determine if it's RW or RAZ/WI.  If the former, we'll
  // gic_wait_for_mask.  If the latter, we'll bail out.
  uint waker = arm_gicv3_read32(GICR_WAKER(cpu));
  if (sleep) {
    waker |= WAKER_PROCESSOR_SLEEP;
  } else {
    waker &= ~WAKER_PROCESSOR_SLEEP;
  }
  arm_gicv3_write32(GICR_WAKER(cpu), waker);
  if (sleep) {
    const uint read_back = arm_gicv3_read32(GICR_WAKER(cpu));
    if ((read_back & WAKER_PROCESSOR_SLEEP) == 0) {
      // Our write didn't take.  Must be RAZ/WI.  Don't bother waiting.
      return;
    }
  }

  uint64_t val = sleep ? WAKER_CHILDREN_ASLEEP : 0;
  gic_wait_for_mask(GICR_WAKER(cpu), WAKER_CHILDREN_ASLEEP, val);
}

void gic_init_percpu_early() {
  cpu_num_t cpu = arch_curr_cpu_num();

  // wake up the redistributor
  gic_redistributor_sleep(false);

  // redistributer config: configure sgi/ppi as non-secure group 1.
  arm_gicv3_write32(GICR_IGROUPR0(cpu), ~0);
  arm_gicv3_write32(GICR_IGRPMOD0(cpu), 0);
  gic_wait_for_rwp(GICR_CTLR(cpu));

  // redistributer config: clear and mask sgi/ppi.
  arm_gicv3_write32(GICR_ICENABLER0(cpu), 0xffffffff);
  arm_gicv3_write32(GICR_ICPENDR0(cpu), ~0);
  gic_wait_for_rwp(GICR_CTLR(cpu));

  // TODO lpi init

  // enable system register interface
  uint32_t sre = gic_read_sre();
  if (!(sre & 0x1)) {
    gic_write_sre(sre | 0x1);
    sre = gic_read_sre();
    DEBUG_ASSERT(sre & 0x1);
  }

  // set priority threshold to max.
  gic_write_pmr(0xff);

  // enable group 1 interrupts.
  gic_write_igrpen(1);
}

zx_status_t gic_init() {
  LTRACE_ENTRY;

  DEBUG_ASSERT(arch_ints_disabled());

  auto map_region = [](uint64_t phys_base, uint64_t size, const char* name) -> zx::result<vaddr_t> {
    void* ptr;
    zx_status_t status = VmAspace::kernel_aspace()->AllocPhysical(
        name, size, &ptr, PAGE_SIZE_SHIFT, phys_base, 0,
        ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE | ARCH_MMU_FLAG_UNCACHED_DEVICE);
    if (status == ZX_OK) {
      return zx::ok(reinterpret_cast<vaddr_t>(ptr));
    }
    return zx::error{status};
  };

  // map in the distributor registers
  auto result = map_region(gic_mmio_phys + gicd_offset, GICD_REG_SIZE, "gicd");
  if (!result.is_ok()) {
    printf("GICv3: failed to map distributor registers");
    return result.error_value();
  }
  arm_gicv3_gicd_base = result.value();
  dprintf(SPEW, "GICv3: distributor mmio region [%#lx, %#lx)\n", arm_gicv3_gicd_base,
          arm_gicv3_gicd_base + GICD_REG_SIZE);

  uint pidr2 = arm_gicv3_read32(GICD_PIDR2);
  uint rev = BITS_SHIFT(pidr2, 7, 4);
  if (rev != GICV3 && rev != GICV4) {
    return ZX_ERR_NOT_FOUND;
  }

  // Compute the stride of the redistributor registers.
  // Override what was passed in from the bootloader, since this is the truth based on the GIC
  // revision.
  // TODO: remove the gicr_stride variable passed in from bootloaders
  if (rev == GICV3) {
    arm_gicv3_gicr_stride = 0x20000;
  } else if (rev == GICV4) {
    arm_gicv3_gicr_stride = 0x40000;
  }

  // map in the redistributor registers
  size_t redistributer_size = arch_max_num_cpus() * arm_gicv3_gicr_stride;
  result = map_region(gic_mmio_phys + gicr_offset, redistributer_size, "gicr");
  if (!result.is_ok()) {
    printf("GICv3: failed to map redistributor registers");
    return result.error_value();
  }
  arm_gicv3_gicr_base = result.value();
  dprintf(SPEW, "GICv3: redistributor mmio region [%#lx, %#lx)\n", arm_gicv3_gicr_base,
          arm_gicv3_gicr_base + redistributer_size);

  uint32_t typer = arm_gicv3_read32(GICD_TYPER);
  gic_max_int = (BITS(typer, 4, 0) + 1) * 32;

  printf("GICv3 detected: rev %u, max interrupts %u, TYPER %#x\n", rev, gic_max_int, typer);

  // disable the distributor
  arm_gicv3_write32(GICD_CTLR, 0);
  gic_wait_for_rwp(GICD_CTLR);
  __isb(ARM_MB_SY);

  // distributor config: mask and clear all spis, set group 1.
  uint i;
  for (i = 32; i < gic_max_int; i += 32) {
    arm_gicv3_write32(GICD_ICENABLER(i / 32), ~0);
    arm_gicv3_write32(GICD_ICPENDR(i / 32), ~0);
    arm_gicv3_write32(GICD_IGROUPR(i / 32), ~0);
    arm_gicv3_write32(GICD_IGRPMODR(i / 32), 0);
  }
  gic_wait_for_rwp(GICD_CTLR);

  // enable distributor with ARE, group 1 enable
  arm_gicv3_write32(GICD_CTLR, CTLR_ENABLE_G0 | CTLR_ENABLE_G1NS | CTLR_ARE_S);
  gic_wait_for_rwp(GICD_CTLR);

  // ensure we're running on cpu 0
  DEBUG_ASSERT(arch_curr_cpu_num() == 0);

  // set spi to target cpu 0 and its associated affinity. must do this after ARE enable
  uint max_cpu = BITS_SHIFT(typer, 7, 5);
  if (max_cpu > 0) {
    for (i = 32; i < gic_max_int; i++) {
      arm_gicv3_write64(GICD_IROUTER(i), arch_cpu_num_to_mpidr(0));
    }
  }

  gic_init_percpu_early();

  arch::DeviceMemoryBarrier();
  __isb(ARM_MB_SY);

  return ZX_OK;
}

// Extract AFF3, AFF2, and AFF1 field out of a mpidr and format according to the ICC_SGI1R
// register.
constexpr uint64_t mpidr_aff_mask_to_sgir_mask(uint64_t mpidr) {
  uint64_t mask = ((mpidr & MPIDR_AFF3_MASK) >> MPIDR_AFF3_SHIFT) << 48;
  mask |= ((mpidr & MPIDR_AFF2_MASK) >> MPIDR_AFF2_SHIFT) << 32;
  mask |= ((mpidr & MPIDR_AFF1_MASK) >> MPIDR_AFF1_SHIFT) << 16;
  return mask;
}

// Send a pending IPI for the AFF3-1 cluster we've been accumulating a mask for.
void send_sgi_for_cluster(unsigned int irq, uint64_t aff321, uint64_t aff0_mask) {
  if (aff0_mask) {
    DEBUG_ASSERT((aff0_mask & 0xffff) == aff0_mask);
    const uint64_t sgi1r = ((irq & 0xf) << 24) | mpidr_aff_mask_to_sgir_mask(aff321) | aff0_mask;
    gic_write_sgi1r(sgi1r);
  }
}

zx_status_t arm_gic_sgi(const unsigned int irq, const unsigned int flags, unsigned int cpu_mask) {
  LTRACEF("irq %u, flags %u, cpu_mask %#x\n", irq, flags, cpu_mask);

  if (flags != ARM_GIC_SGI_FLAG_NS) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (irq >= 16) {
    return ZX_ERR_INVALID_ARGS;
  }

  arch::ThreadMemoryBarrier();

  uint64_t curr_aff321 = 0;  // Current AFF3-1 we're dealing with.
  uint64_t aff0_mask = 0;    // 16 bit mask of the AFF0 we're accumulating.

  for (cpu_num_t cpu = 0; cpu_mask && cpu < arch_max_num_cpus(); cpu++) {
    const uint64_t mpidr = arch_cpu_num_to_mpidr(cpu);
    const uint64_t aff321 = mpidr & (MPIDR_AFF3_MASK | MPIDR_AFF2_MASK | MPIDR_AFF1_MASK);
    const uint64_t aff0 = mpidr & (MPIDR_AFF0_MASK);

    // Without the RS field set, we can only deal with the first
    // 16 cpus within a single cluster.
    DEBUG_ASSERT(aff0 < 16);

    if (aff321 != curr_aff321) {
      // AFF3-1 has changed, see if we need to fire a pending IPI
      send_sgi_for_cluster(irq, curr_aff321, aff0_mask);
      curr_aff321 = aff321;
      aff0_mask = 0;
    }

    // This cpu is within the current aff mask we're looking at, accumulate.
    if (cpu_mask & (1u << cpu)) {
      cpu_mask &= ~(1u << cpu);
      aff0_mask |= 1u << aff0;
    }
  }

  // Fire any leftover accumulated mask.
  send_sgi_for_cluster(irq, curr_aff321, aff0_mask);

  return ZX_OK;
}

zx_status_t gic_mask_interrupt(interrupt_vector_t vector) {
  LTRACEF("vector %u\n", vector);

  if (vector >= gic_max_int) {
    return ZX_ERR_INVALID_ARGS;
  }

  gic_set_enable(vector, false);

  return ZX_OK;
}

zx_status_t gic_unmask_interrupt(interrupt_vector_t vector) {
  LTRACEF("vector %u\n", vector);

  if (vector >= gic_max_int) {
    return ZX_ERR_INVALID_ARGS;
  }

  gic_set_enable(vector, true);

  return ZX_OK;
}

zx_status_t gic_deactivate_interrupt(interrupt_vector_t vector) {
  if (vector >= gic_max_int) {
    return ZX_ERR_INVALID_ARGS;
  }

  uint32_t reg = 1 << (vector % 32);
  arm_gicv3_write32(GICD_ICACTIVER(vector / 32), reg);

  return ZX_OK;
}

zx_status_t gic_configure_interrupt(interrupt_vector_t vector, enum interrupt_trigger_mode tm,
                                    enum interrupt_polarity pol) {
  LTRACEF("vector %u, trigger mode %s, polarity %s\n", vector, interrupt_trigger_mode_string(tm),
          interrupt_polarity_string(pol));

  if (vector <= 15 || vector >= gic_max_int) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (pol != interrupt_polarity::HIGH) {
    // TODO: polarity should actually be configure through a GPIO controller
    return ZX_ERR_NOT_SUPPORTED;
  }

  uint reg = vector / 16;
  uint mask = 0x2 << ((vector % 16) * 2);
  uint32_t val = arm_gicv3_read32(GICD_ICFGR(reg));
  if (tm == interrupt_trigger_mode::EDGE) {
    val |= mask;
  } else {
    val &= ~mask;
  }
  arm_gicv3_write32(GICD_ICFGR(reg), val);

  const uint32_t clear_reg = vector / 32;
  const uint32_t clear_mask = 1 << (vector % 32);
  arm_gicv3_write32(GICD_ICPENDR(clear_reg), clear_mask);

  return ZX_OK;
}

zx_status_t gic_get_interrupt_config(interrupt_vector_t vector, interrupt_trigger_mode* tm,
                                     interrupt_polarity* pol) {
  LTRACEF("vector %u\n", vector);

  if (vector >= gic_max_int) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (tm) {
    *tm = interrupt_trigger_mode::EDGE;
  }
  if (pol) {
    *pol = interrupt_polarity::HIGH;
  }

  return ZX_OK;
}

zx_status_t gic_set_affinity(interrupt_vector_t vector, cpu_mask_t mask) {
  LTRACEF("vector %u, mask %#x\n", vector, mask);

  // Don't complain if we are asked to set affinity for the boot CPU, this is
  // what we always do by default.
  if (mask == cpu_num_to_mask(BOOT_CPU_ID)) {
    return ZX_OK;
  }

  return ZX_ERR_NOT_SUPPORTED;
}

interrupt_vector_t gic_remap_interrupt(interrupt_vector_t vector) {
  LTRACEF("vector %u\n", vector);
  return vector;
}

// called from assembly
void gic_handle_irq(iframe_t* frame) {
  // get the current vector
  uint32_t iar = gic_read_iar();
  unsigned vector = iar & 0x3ff;

  LTRACEF_LEVEL(2, "iar %#x, vector %u\n", iar, vector);

  if (vector >= 0x3fe) {
    // spurious
    // TODO check this
    return;
  }

  // tracking external hardware irqs in this variable
  if (vector >= 32) {
    CPU_STATS_INC(interrupts);
  }

  ktrace::Scope trace = KTRACE_CPU_BEGIN_SCOPE("kernel:irq", "irq", ("irq #", vector));

  LTRACEF_LEVEL(2, "iar 0x%x cpu %u currthread %p vector %u pc %#" PRIxPTR "\n", iar,
                arch_curr_cpu_num(), Thread::Current::Get(), vector, (uintptr_t)IFRAME_PC(frame));

  // deliver the interrupt
  pdev_invoke_int_if_present(vector);
  gic_write_eoir(vector);

  LTRACEF_LEVEL(2, "cpu %u exit\n", arch_curr_cpu_num());
}

zx_status_t gic_send_ipi(cpu_mask_t target, mp_ipi ipi) {
  uint gic_ipi_num = static_cast<uint32_t>(ipi) + ipi_base;

  // filter out targets outside of the range of cpus we care about
  target &= static_cast<cpu_mask_t>(((1UL << arch_max_num_cpus()) - 1));
  if (target != 0) {
    LTRACEF("target 0x%x, gic_ipi %u\n", target, gic_ipi_num);
    arm_gic_sgi(gic_ipi_num, ARM_GIC_SGI_FLAG_NS, target);
  }

  return ZX_OK;
}

void arm_ipi_halt_handler() {
  LTRACEF("cpu %u\n", arch_curr_cpu_num());

  arch_disable_ints();
  while (true) {
    __wfi();
  }
}

void gic_init_percpu() {
  mp_set_curr_cpu_online(true);
  unmask_interrupt(static_cast<uint32_t>(mp_ipi::GENERIC) + ipi_base);
  unmask_interrupt(static_cast<uint32_t>(mp_ipi::RESCHEDULE) + ipi_base);
  unmask_interrupt(static_cast<uint32_t>(mp_ipi::INTERRUPT) + ipi_base);
  unmask_interrupt(static_cast<uint32_t>(mp_ipi::HALT) + ipi_base);
}

void gic_shutdown() {
  // Turn off all GIC0 interrupts at the distributor.
  arm_gicv3_write32(GICD_CTLR, 0);
}

// Returns true if any PPIs are enabled on the calling CPU.
[[maybe_unused]] bool is_ppi_enabled() {
  DEBUG_ASSERT(arch_ints_disabled());

  // PPIs are 16-31.
  uint32_t mask = 0xffff0000;

  cpu_num_t cpu_num = arch_curr_cpu_num();
  uint32_t reg = arm_gicv3_read32(GICR_ICENABLER0(cpu_num));

  return (reg & mask);
}

// Returns true if any SPIs are enabled on the calling CPU.
[[maybe_unused]] bool is_spi_enabled() {
  DEBUG_ASSERT(arch_ints_disabled());

  const cpu_num_t cpu_num = arch_curr_cpu_num();
  const uint64_t mpidr = arch_cpu_num_to_mpidr(cpu_num);
  const uint64_t aff_mask = mpidr & ARM64_MPIDR_MASK;

  // Check each SPI to see if it's routed to this CPU and enabled.
  for (uint i = 32u; i < gic_max_int; ++i) {
    if ((arm_gicv3_read64(GICD_IROUTER(i)) & ARM64_MPIDR_MASK) == aff_mask) {
      uint32_t reg_n = i / 32;
      uint32_t bit_n = 1U << (i % 32);
      if (arm_gicv3_read32(GICD_ISENABLER(reg_n)) & bit_n) {
        return true;
      }
    }
  }

  return false;
}

void gic_shutdown_cpu() {
  DEBUG_ASSERT(arch_ints_disabled());

  // If we're running on a secondary CPU there's a good chance this CPU will be powered off
  // shortly (PSCI_CPU_OFF).  Sending an interrupt to a CPU that's been powered off may result in
  // an "erronerous state" (see Power State Coordination Interface (PSCI) System Software on ARM
  // specification, 5.5.2).  So before we shutdown the GIC, make sure we've migrated/disabled any
  // and all peripheral interrupts targeted at this CPU (PPIs and SPIs).
  //
  // Note, we don't perform these checks on the boot CPU because we don't call PSCI_CPU_OFF on the
  // boot CPU, and we likely still have PPIs and SPIs targeting the boot CPU.
  DEBUG_ASSERT(arch_curr_cpu_num() == BOOT_CPU_ID || !is_ppi_enabled());
  DEBUG_ASSERT(arch_curr_cpu_num() == BOOT_CPU_ID || !is_spi_enabled());
  // TODO(maniscalco): If/when we start using LPIs, make sure none are targeted at this CPU.

  // Disable group 1 interrupts at the CPU interface.
  gic_write_igrpen(0);

  // Mark the PE as offline. This will keep the redistributor from routing
  // interrupts and for any interrupts targeting it, trigger a wake-request to
  // the power controller.
  gic_redistributor_sleep(true);
}

zx_status_t gic_suspend_cpu() {
  DEBUG_ASSERT(arch_ints_disabled());

  gic_redistributor_sleep(true);

  return ZX_OK;
}

zx_status_t gic_resume_cpu() {
  gic_init_percpu_early();
  gic_init_percpu();
  return ZX_OK;
}

bool gic_msi_is_supported() { return false; }

bool gic_msi_supports_masking() { return false; }

void gic_msi_mask_unmask(const msi_block_t* block, uint msi_id, bool mask) { PANIC_UNIMPLEMENTED; }

zx_status_t gic_msi_alloc_block(uint requested_irqs, bool can_target_64bit, bool is_msix,
                                msi_block_t* out_block) {
  PANIC_UNIMPLEMENTED;
}

void gic_msi_free_block(msi_block_t* block) { PANIC_UNIMPLEMENTED; }

void gic_msi_register_handler(const msi_block_t* block, uint msi_id, interrupt_handler_t handler) {
  PANIC_UNIMPLEMENTED;
}

zx_status_t gic_get_status(interrupt_vector_t vector, bool& out_pending, bool& out_enabled) {
  out_pending = out_enabled = false;
  if (vector >= gic_max_int) {
    return ZX_ERR_INVALID_ARGS;
  }

  const uint32_t reg = vector / 32;
  const uint32_t mask = static_cast<uint32_t>(1ULL << (vector % 32));
  if (vector < 32) {
    cpu_num_t cpu_id = arch_curr_cpu_num();
    out_enabled = (arm_gicv3_read32(GICR_ISENABLER0(cpu_id)) & mask) != 0;
    out_pending = (arm_gicv3_read32(GICR_ISPENDR0(cpu_id)) & mask) != 0;
  } else {
    out_enabled = (arm_gicv3_read32(GICD_ISENABLER(reg)) & mask) != 0;
    out_pending = (arm_gicv3_read32(GICD_ISPENDR(reg)) & mask) != 0;
  }

  return ZX_OK;
}

const struct pdev_interrupt_ops gic_ops = {
    .mask = gic_mask_interrupt,
    .unmask = gic_unmask_interrupt,
    .deactivate = gic_deactivate_interrupt,
    .configure = gic_configure_interrupt,
    .get_config = gic_get_interrupt_config,
    .set_affinity = gic_set_affinity,
    .is_valid = gic_is_valid_interrupt,
    .get_base_vector = gic_get_base_vector,
    .get_max_vector = gic_get_max_vector,
    .remap = gic_remap_interrupt,
    .send_ipi = gic_send_ipi,
    .init_percpu_early = gic_init_percpu_early,
    .init_percpu = gic_init_percpu,
    .handle_irq = gic_handle_irq,
    .shutdown = gic_shutdown,
    .shutdown_cpu = gic_shutdown_cpu,
    .suspend_cpu = gic_suspend_cpu,
    .resume_cpu = gic_resume_cpu,
    .msi_is_supported = gic_msi_is_supported,
    .msi_supports_masking = gic_msi_supports_masking,
    .msi_mask_unmask = gic_msi_mask_unmask,
    .msi_alloc_block = gic_msi_alloc_block,
    .msi_free_block = gic_msi_free_block,
    .msi_register_handler = gic_msi_register_handler,
    .get_status = gic_get_status,
};

}  // anonymous namespace

void ArmGicInitEarly(const zbi_dcfg_arm_gic_v3_driver_t& config) {
  ASSERT(config.mmio_phys);

  gic_mmio_phys = config.mmio_phys;
  gicd_offset = config.gicd_offset;
  gicr_offset = config.gicr_offset;
  arm_gicv3_gicr_stride = config.gicr_stride;
  ipi_base = config.ipi_base;
}

void ArmGicInitPostVm(const zbi_dcfg_arm_gic_v3_driver_t& config) {
  if (gic_init() != ZX_OK) {
    if (config.optional) {
      // failed to detect gic v3 but it's marked optional. continue
      return;
    }
    printf("GICv3: failed to detect GICv3, interrupts will be broken\n");
    return;
  }

  dprintf(SPEW,
          "GICv3: IPI base %u, MMIO phys %#lx, GICD offset %#lx, "
          "GICR offset/stride %#lx/%#lx\n",
          ipi_base, gic_mmio_phys, gicd_offset, gicr_offset, arm_gicv3_gicr_stride);

  pdev_register_interrupts(&gic_ops);

  zx_status_t status =
      gic_register_sgi_handler(static_cast<uint32_t>(mp_ipi::GENERIC) + ipi_base,
                               interrupt_handler_t{nullptr, [](void*) { mp_mbx_generic_irq(); }});
  DEBUG_ASSERT(status == ZX_OK);
  status = gic_register_sgi_handler(
      static_cast<uint32_t>(mp_ipi::RESCHEDULE) + ipi_base,
      interrupt_handler_t{nullptr, [](void*) { mp_mbx_reschedule_irq(); }});
  DEBUG_ASSERT(status == ZX_OK);
  status =
      gic_register_sgi_handler(static_cast<uint32_t>(mp_ipi::INTERRUPT) + ipi_base,
                               interrupt_handler_t{nullptr, [](void*) { mp_mbx_interrupt_irq(); }});
  DEBUG_ASSERT(status == ZX_OK);
  status =
      gic_register_sgi_handler(static_cast<uint32_t>(mp_ipi::HALT) + ipi_base,
                               interrupt_handler_t{nullptr, [](void*) { arm_ipi_halt_handler(); }});
  DEBUG_ASSERT(status == ZX_OK);

  gicv3_hw_interface_register();
}

void ArmGicInitLate(const zbi_dcfg_arm_gic_v3_driver_t& config) {
  ASSERT(gic_mmio_phys);

  // Place the physical address of the GICv3 registers on the MMIO deny list.
  // Users will not be able to create MMIO resources which permit mapping of the
  // GIC registers, even if they have access to the root resource.
  //
  // Unlike GICv2, only the distributor and re-distributor registers are memory
  // mapped. There is one block of distributor registers for the system, and
  // one block of redistributor registers for each CPU.
  root_resource_filter_add_deny_region(gic_mmio_phys + gicd_offset, GICD_REG_SIZE,
                                       ZX_RSRC_KIND_MMIO);
  for (cpu_num_t i = 0; i < arch_max_num_cpus(); i++) {
    root_resource_filter_add_deny_region(gic_mmio_phys + gicr_offset + (arm_gicv3_gicr_stride * i),
                                         arm_gicv3_gicr_stride, ZX_RSRC_KIND_MMIO);
  }
}
