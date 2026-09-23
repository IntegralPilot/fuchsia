// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2012-2015 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "arm_gicv2.h"

#include <assert.h>
#include <bits.h>
#include <debug.h>
#include <inttypes.h>
#include <lib/ktrace.h>
#include <lib/root_resource_filter.h>
#include <lib/system-topology.h>
#include <lib/zbi-format/driver-config.h>
#include <sys/types.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <arch/arm64.h>
#include <arch/arm64/hypervisor/gic/gicv2.h>
#include <arch/ops.h>
#include <arch/regs.h>
#include <dev/interrupt.h>
#include <dev/interrupt/arm_gic_common.h>
#include <dev/interrupt/arm_gicv2_init.h>
#include <dev/interrupt/arm_gicv2_regs.h>
#include <dev/interrupt/arm_gicv2m.h>
#include <dev/interrupt/arm_gicv2m_msi.h>
#include <kernel/cpu.h>
#include <kernel/lockdep.h>
#include <kernel/stats.h>
#include <kernel/thread.h>
#include <ktl/iterator.h>
#include <lk/init.h>
#include <pdev/interrupt.h>
#include <vm/vm.h>
#include <vm/vm_aspace.h>

#include <ktl/enforce.h>

#define LOCAL_TRACE 0

#define IFRAME_PC(frame) ((frame)->elr)

// Values read from the config.
vaddr_t arm_gicv2_gic_base = 0;
uint64_t arm_gicv2_gicd_offset = 0;
uint64_t arm_gicv2_gicc_offset = 0;
uint64_t arm_gicv2_gich_offset = 0;
uint64_t arm_gicv2_gicv_offset = 0;

namespace {

uint64_t mmio_phys = 0;
uint64_t msi_frame_phys = 0;
uint32_t ipi_base = 0;
bool use_msi = false;

constexpr uint MAX_INT = 1024;
uint max_irqs = 0;

arm_gicv2::CpuMaskTranslator mask_translator;
DECLARE_SINGLETON_SPINLOCK(gic_lock);

zx_status_t gic_configure_interrupt(interrupt_vector_t vector, interrupt_trigger_mode tm,
                                    interrupt_polarity pol);

uint32_t read_gicd_targetsr(int target_reg) { return arm_gicv2_read32(GICD_ITARGETSR(target_reg)); }

}  // anonymous namespace

uint8_t gic_determine_local_mask(
    fit::inline_function<uint32_t(int), sizeof(void*)> fetch_gicd_targetsr_reg) {
  union {
    char bytes[4];
    uint32_t value;
  } decoder;

  // The first 7 GICD_ITARGETSR registers only return values that apply to the
  // calling CPU. These are interrupt target registers so each byte is a cpu_mask
  // of what cpus are targeted by their cpu_mask from GIC's perspective. The
  // upside is that we can scan these target regsiters to determine what our
  // cpu_mask is from the GIC's perspective.
  for (int target_reg = 0; target_reg < 8; target_reg++) {
    decoder.value = fetch_gicd_targetsr_reg(target_reg);
    for (int i = 0; i < 4; i++) {
      if (decoder.bytes[i] != 0) {
        return decoder.bytes[i];
      }
    }
  }

  printf("GICv2: unable to determine local GIC mask!\n");
  return 0;
}

namespace {

bool gic_is_valid_interrupt(uint vector, uint32_t flags) { return (vector < max_irqs); }

uint32_t gic_get_base_vector() {
  // ARM Generic Interrupt Controller v2 chapter 2.1
  // INTIDs 0-15 are local CPU interrupts
  return 16;
}

uint32_t gic_get_max_vector() { return max_irqs; }

void gic_set_enable(uint vector, bool enable) {
  int reg = vector / 32;
  uint32_t mask = (uint32_t)(1ULL << (vector % 32));

  if (enable) {
    arm_gicv2_write32(GICD_ISENABLER(reg), mask);
  } else {
    arm_gicv2_write32(GICD_ICENABLER(reg), mask);
  }
}

void gic_init_percpu_early() {
  arm_gicv2_write32(GICC_CTLR, 0x1);  // EnableGrp1
  arm_gicv2_write32(GICC_PMR, 0xff);  // unmask interrupts at all priority levels
}

unsigned int arm_gic_max_cpu() { return BITS_SHIFT(arm_gicv2_read32(GICD_TYPER), 7, 5) + 1; }

zx_status_t arm_gic_init() {
  uint i;

  uint32_t typer = arm_gicv2_read32(GICD_TYPER);
  uint32_t it_lines_number = BITS_SHIFT(typer, 4, 0);
  max_irqs = (it_lines_number + 1) * 32;
  printf("GICv2 detected: max interrupts %u, max_cpus %u, TYPER %#x\n", max_irqs, arm_gic_max_cpu(),
         typer);
  DEBUG_ASSERT(max_irqs <= MAX_INT);

  for (i = 0; i < max_irqs; i += 32) {
    arm_gicv2_write32(GICD_ICENABLER(i / 32), ~0);
    arm_gicv2_write32(GICD_ICPENDR(i / 32), ~0);
  }

  if (arm_gic_max_cpu() > 0) {
    // Set external interrupts to target cpu 0
    for (i = 32; i < max_irqs; i += 4) {
      arm_gicv2_write32(GICD_ITARGETSR(i / 4), 0x01010101);
    }
  }
  // Initialize all the SPIs to edge triggered
  for (i = GIC_BASE_SPI; i < max_irqs; i++) {
    gic_configure_interrupt(i, interrupt_trigger_mode::EDGE, interrupt_polarity::HIGH);
  }

  arm_gicv2_write32(GICD_CTLR, 1);  // enable GIC0

  gic_init_percpu_early();

  return ZX_OK;
}

zx_status_t arm_gic_sgi(unsigned int irq, unsigned int flags, unsigned int cpu_mask) {
  unsigned int val = ((flags & ARM_GIC_SGI_FLAG_TARGET_FILTER_MASK) << 24) |
                     ((cpu_mask & 0xff) << 16) | ((flags & ARM_GIC_SGI_FLAG_NS) ? (1U << 15) : 0) |
                     (irq & 0xf);

  if (irq >= 16) {
    return ZX_ERR_INVALID_ARGS;
  }

  LTRACEF("GICD_SGIR: %x\n", val);

  arm_gicv2_write32(GICD_SGIR, val);

  return ZX_OK;
}

zx_status_t gic_mask_interrupt(interrupt_vector_t vector) {
  LTRACEF("vector %u\n", vector);
  if (vector >= max_irqs) {
    return ZX_ERR_INVALID_ARGS;
  }

  gic_set_enable(vector, false);

  return ZX_OK;
}

zx_status_t gic_unmask_interrupt(interrupt_vector_t vector) {
  LTRACEF("vector %u\n", vector);
  if (vector >= max_irqs) {
    return ZX_ERR_INVALID_ARGS;
  }

  gic_set_enable(vector, true);

  return ZX_OK;
}

zx_status_t gic_deactivate_interrupt(interrupt_vector_t vector) {
  if (vector >= max_irqs) {
    return ZX_ERR_INVALID_ARGS;
  }

  uint32_t reg = 1 << (vector % 32);
  arm_gicv2_write32(GICD_ICACTIVER(vector / 32), reg);

  return ZX_OK;
}

zx_status_t gic_configure_interrupt(interrupt_vector_t vector, interrupt_trigger_mode tm,
                                    interrupt_polarity pol) {
  // Only configurable for SPI interrupts
  if ((vector >= max_irqs) || (vector < GIC_BASE_SPI)) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (pol != interrupt_polarity::HIGH) {
    // TODO: polarity should actually be configure through a GPIO controller
    return ZX_ERR_NOT_SUPPORTED;
  }

  Guard<SpinLock, IrqSave> guard{gic_lock::Get()};

  // type is encoded with two bits, MSB of the two determine type
  // 16 irqs encoded per ICFGR register
  uint32_t reg_ndx = vector >> 4;
  uint32_t bit_shift = ((vector & 0xf) << 1) + 1;
  uint32_t reg_val = arm_gicv2_read32(GICD_ICFGR(reg_ndx));
  if (tm == interrupt_trigger_mode::EDGE) {
    reg_val |= (1U << bit_shift);
  } else {
    reg_val &= ~(1U << bit_shift);
  }
  arm_gicv2_write32(GICD_ICFGR(reg_ndx), reg_val);

  const uint32_t clear_reg = vector / 32;
  const uint32_t clear_mask = 1 << (vector % 32);
  arm_gicv2_write32(GICD_ICPENDR(clear_reg), clear_mask);

  return ZX_OK;
}

zx_status_t gic_get_interrupt_config(interrupt_vector_t vector, interrupt_trigger_mode* tm,
                                     interrupt_polarity* pol) {
  if (vector >= max_irqs) {
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

  if (vector >= max_irqs) {
    return ZX_ERR_INVALID_ARGS;
  }

  // make sure the cpu mask is within range
  if (highest_cpu_set(mask) > 7) {
    return ZX_ERR_INVALID_ARGS;
  }

  Guard<SpinLock, IrqSave> guard{gic_lock::Get()};

  // set the 4 bit field in the corresponding targetsr register with the
  // 8 bit cpu mask.
  uint32_t targetsr = arm_gicv2_read32(GICD_ITARGETSR(vector / 4));
  uint32_t old_targetsr = targetsr;
  targetsr &= ~(0xff << ((vector % 4) * 8));
  targetsr |= mask << ((vector % 4) * 8);
  arm_gicv2_write32(GICD_ITARGETSR(vector / 4), targetsr);

  guard.Release();

  LTRACEF("setting TARGETSR old %#x new %#x\n", old_targetsr, targetsr);

  return ZX_OK;
}

interrupt_vector_t gic_remap_interrupt(interrupt_vector_t vector) { return vector; }

void gic_handle_irq(iframe_t* frame) {
  // get the current vector
  uint32_t iar = arm_gicv2_read32(GICC_IAR);
  unsigned int vector = iar & 0x3ff;

  if (vector >= 0x3fe) {
    // spurious
    return;
  }

  // tracking external hardware irqs in this variable
  if (vector >= 32)
    CPU_STATS_INC(interrupts);

  ktrace::Scope trace = KTRACE_CPU_BEGIN_SCOPE("kernel:irq", "irq", ("irq #", vector));

  LTRACEF_LEVEL(2, "iar 0x%x cpu %u currthread %p vector %u pc %#" PRIxPTR "\n", iar,
                arch_curr_cpu_num(), Thread::Current::Get(), vector, (uintptr_t)IFRAME_PC(frame));

  // deliver the interrupt
  pdev_invoke_int_if_present(vector);
  arm_gicv2_write32(GICC_EOIR, iar);

  LTRACEF_LEVEL(2, "cpu %u exit\n", arch_curr_cpu_num());
}

zx_status_t gic_send_ipi(cpu_mask_t logical_target, mp_ipi ipi) {
  const cpu_mask_t target = mask_translator.LogicalMaskToGic(logical_target);

  uint gic_ipi_num = static_cast<uint32_t>(ipi) + ipi_base;

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

  const cpu_num_t logical_num = arch_curr_cpu_num();

  system_topology::Node* node = nullptr;
  auto status =
      system_topology::Graph::GetSystemTopology().ProcessorByLogicalId(logical_num, &node);
  if (status == ZX_OK) {
    cpu_num_t gic_id;
    DEBUG_ASSERT(node->entity.discriminant == ZBI_TOPOLOGY_ENTITY_PROCESSOR &&
                 node->entity.processor.architecture_info.discriminant ==
                     ZBI_TOPOLOGY_ARCHITECTURE_INFO_ARM64);
    gic_id = node->entity.processor.architecture_info.arm64.gic_id;
    mask_translator.SetGicIdForLogicalId(logical_num, gic_id);
  } else {
    printf("arm_gicv2: unable to get logical processor %u in topology, status: %d\n", logical_num,
           status);
  }

  // Lookup the gic_mask for this processor via GIC registers and compare it
  // to what we were told by the bootloader, warn if there is a mismatch.
  const cpu_mask_t gic_mask = gic_determine_local_mask(read_gicd_targetsr);
  const cpu_mask_t assigned_gic_mask = mask_translator.GetGicMask(logical_num);
  if (gic_mask != assigned_gic_mask) {
    printf(
        "arm_gicv2: WARNING assigned gic_id of %u does not match processor's gic_id of %u."
        "Successful operation is unlikely!",
        assigned_gic_mask, gic_mask);
  }
  TRACEF("logical_cpu_mask: %u programmatic_gic_mask: %u assigned_gic_mask: %u\n",
         cpu_num_to_mask(arch_curr_cpu_num()), gic_mask, assigned_gic_mask);
}

void gic_shutdown() {
  // Turn off all GIC0 interrupts at the distributor.
  arm_gicv2_write32(GICD_CTLR, 0);
}

// Returns true if any PPIs are enabled on the calling CPU.
[[maybe_unused]] bool is_ppi_enabled() {
  DEBUG_ASSERT(arch_ints_disabled());

  // PPIs are 16-31.
  uint32_t ppi_mask = 0xffff0000;

  // GICD_ISENABLER0 is banked so it corresponds to *this* CPU's interface.
  return (arm_gicv2_read32(GICD_ISENABLER(0)) & ppi_mask) != 0;
}

// Returns true if any SPIs are enabled on the calling CPU.
[[maybe_unused]] bool is_spi_enabled() {
  DEBUG_ASSERT(arch_ints_disabled());

  // We're going to check four interrupts at a time.  Build a repeated mask for the current CPU.
  // Each byte in the mask is a CPU bit mask corresponding to CPU0..CPU7 (lsb..msb).
  cpu_num_t cpu_num = arch_curr_cpu_num();
  DEBUG_ASSERT(cpu_num < 8);
  uint32_t mask = 0x01010101U << cpu_num;

  for (unsigned int vector = GIC_BASE_SPI; vector < max_irqs; vector += 4) {
    uint32_t reg = arm_gicv2_read32(GICD_ITARGETSR(vector / 4));
    if (reg & mask) {
      return true;
    }
  }

  return false;
}

void gic_shutdown_cpu() {
  DEBUG_ASSERT(arch_ints_disabled());

  // If we're running on a secondary CPU there's a good chance this CPU will be powered off shortly
  // (PSCI_CPU_OFF).  Sending an interrupt to a CPU that's been powered off may result in an
  // "erronerous state" (see Power State Coordination Interface (PSCI) System Software on ARM
  // specification, 5.5.2).  So before we shutdown the GIC, make sure we've migrated/disabled any
  // and all peripheral interrupts targeted at this CPU (PPIs and SPIs).
  //
  // Note, we don't perform these checks on the boot CPU because we don't call PSCI_CPU_OFF on the
  // boot CPU, and we likely still have PPIs and SPIs targeting the boot CPU.
  DEBUG_ASSERT(arch_curr_cpu_num() == BOOT_CPU_ID || !is_ppi_enabled());
  DEBUG_ASSERT(arch_curr_cpu_num() == BOOT_CPU_ID || !is_spi_enabled());

  // Turn off interrupts at the CPU interface.
  arm_gicv2_write32(GICC_CTLR, 0);
}

zx_status_t gic_suspend_cpu() { return ZX_ERR_NOT_SUPPORTED; }

zx_status_t gic_resume_cpu() { return ZX_ERR_NOT_SUPPORTED; }

zx_status_t gic_get_status(interrupt_vector_t vector, bool& out_pending, bool& out_enabled) {
  out_pending = out_enabled = false;
  if (vector >= MAX_INT) {
    return ZX_ERR_INVALID_ARGS;
  }

  const uint32_t reg = vector / 32;
  const uint32_t mask = static_cast<uint32_t>(1ULL << (vector % 32));
  out_enabled = (arm_gicv2_read32(GICD_ISENABLER(reg)) & mask) != 0;
  out_pending = (arm_gicv2_read32(GICD_ISPENDR(reg)) & mask) != 0;

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
    .msi_is_supported = arm_gicv2m_msi_is_supported,
    .msi_supports_masking = arm_gicv2m_msi_supports_masking,
    .msi_mask_unmask = arm_gicv2m_msi_mask_unmask,
    .msi_alloc_block = arm_gicv2m_msi_alloc_block,
    .msi_free_block = arm_gicv2m_msi_free_block,
    .msi_register_handler = arm_gicv2m_msi_register_handler,
    .get_status = gic_get_status,
};

}  // anonymous namespace

// Init hooks

void ArmGicInitEarly(const zbi_dcfg_arm_gic_v2_driver_t& config) {
  ASSERT(config.mmio_phys);

  mmio_phys = config.mmio_phys;
  msi_frame_phys = config.msi_frame_phys;
  arm_gicv2_gicd_offset = config.gicd_offset;
  arm_gicv2_gicc_offset = config.gicc_offset;
  arm_gicv2_gich_offset = config.gich_offset;
  arm_gicv2_gicv_offset = config.gicv_offset;
  ipi_base = config.ipi_base;
  use_msi = config.use_msi;
}

void ArmGicInitPostVm(const zbi_dcfg_arm_gic_v2_driver_t& config) {
  // Compute the total size of the mmio region by finding the farthest offset and adding its size,
  // assuming all of the space between is part of the same mmio interface.
  uint64_t max_offset = 0;
  size_t mmio_size = 0;

  struct {
    uint64_t offset;
    size_t len;
  } gicv2_apertures[] = {{arm_gicv2_gicd_offset, GICD_REG_SIZE},
                         {arm_gicv2_gicc_offset, GICC_REG_SIZE},
                         {arm_gicv2_gich_offset, GICH_REG_SIZE},
                         {arm_gicv2_gicv_offset, GICV_REG_SIZE}};
  for (auto [offset, len] : gicv2_apertures) {
    if (offset > max_offset) {
      max_offset = offset;
      mmio_size = offset + len;
    }
  }

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

  // Map the mmio region.
  auto map_result = map_region(mmio_phys, mmio_size, "gicv2");
  if (map_result.is_error()) {
    panic("Could not allocate GICv2 mmio region: %d\n", map_result.error_value());
  }
  arm_gicv2_gic_base = map_result.value();
  dprintf(SPEW, "GICv2: mmio region [%#lx, %#lx)\n", arm_gicv2_gic_base,
          arm_gicv2_gic_base + mmio_size);
  ASSERT(arm_gicv2_gic_base);

  if (arm_gic_init() != ZX_OK) {
    if (config.optional) {
      // failed to detect gic v2 but it's marked optional. continue
      return;
    }
    printf("GICv2: failed to detect GICv2, interrupts will be broken\n");
    return;
  }

  dprintf(SPEW,
          "GICv2 (ID %#x), IPI base %u, MMIO phys %#lx, GICD offset %#lx, GICC offset %#lx\n"
          "\tGICH offset %#lx, GICV offset %#lx\n",
          arm_gicv2_read32(GICC_IIDR), ipi_base, mmio_phys, arm_gicv2_gicd_offset,
          arm_gicv2_gicc_offset, arm_gicv2_gich_offset, arm_gicv2_gicv_offset);

  // pass the list of physical and virtual addresses for the GICv2m register apertures
  if (use_msi && msi_frame_phys) {
    // the following arrays must be static because arm_gicv2m_init stashes the pointer
    static paddr_t GICV2M_REG_FRAMES[] = {0};
    static vaddr_t GICV2M_REG_FRAMES_VIRT[] = {0};

    // map in the gicv2m aperture
    map_result = map_region(msi_frame_phys, GICV2M_FRAME_REG_SIZE, "gicv2m");
    if (map_result.is_error()) {
      panic("Could not allocate GICv2m mmio region: %d\n", map_result.error_value());
    }
    dprintf(SPEW, "GICv2m: mmio region [%#lx, %#lx)\n", map_result.value(),
            map_result.value() + GICV2M_FRAME_REG_SIZE);

    GICV2M_REG_FRAMES[0] = msi_frame_phys;
    GICV2M_REG_FRAMES_VIRT[0] = map_result.value();
    ASSERT(GICV2M_REG_FRAMES_VIRT[0]);
    arm_gicv2m_init(GICV2M_REG_FRAMES, GICV2M_REG_FRAMES_VIRT, ktl::size(GICV2M_REG_FRAMES));
  }
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

  gicv2_hw_interface_register();
}

void ArmGicInitLate(const zbi_dcfg_arm_gic_v2_driver_t& config) {
  ASSERT(mmio_phys);

  // Place the physical address of the GICv2 registers on the MMIO deny list.
  // Users will not be able to create MMIO resources which permit mapping of the
  // GIC registers, even if they have access to the root resource.
  root_resource_filter_add_deny_region(mmio_phys + arm_gicv2_gicc_offset, GICC_REG_SIZE,
                                       ZX_RSRC_KIND_MMIO);
  root_resource_filter_add_deny_region(mmio_phys + arm_gicv2_gicd_offset, GICD_REG_SIZE,
                                       ZX_RSRC_KIND_MMIO);
  if (arm_gicv2_gich_offset) {
    root_resource_filter_add_deny_region(mmio_phys + arm_gicv2_gich_offset, GICH_REG_SIZE,
                                         ZX_RSRC_KIND_MMIO);
  }
  if (arm_gicv2_gicv_offset) {
    root_resource_filter_add_deny_region(mmio_phys + arm_gicv2_gicv_offset, GICV_REG_SIZE,
                                         ZX_RSRC_KIND_MMIO);
  }
  if (msi_frame_phys) {
    root_resource_filter_add_deny_region(msi_frame_phys, GICV2M_FRAME_REG_SIZE, ZX_RSRC_KIND_MMIO);
  }
}
