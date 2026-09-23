// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <arch.h>
#include <debug.h>
#include <lib/affine/ratio.h>
#include <lib/arch/intrin.h>
#include <lib/boot-options/boot-options.h>
#include <lib/boot-options/types.h>
#include <lib/console.h>
#include <lib/crashlog.h>
#include <lib/debuglog.h>
#include <lib/jtrace/jtrace.h>
#include <lib/memalloc/range.h>
#include <lib/persistent-debuglog.h>
#include <lib/system-topology.h>
#include <platform.h>
#include <string-file.h>
#include <trace.h>

#include <arch/arch_ops.h>
#include <arch/mp.h>
#include <arch/riscv64.h>
#include <arch/riscv64/sbi.h>
#include <dev/hw_rng.h>
#include <dev/interrupt.h>
#include <dev/power.h>
#include <explicit-memory/bytes.h>
#include <fbl/array.h>
#include <kernel/cpu_distance_map.h>
#include <kernel/dpc.h>
#include <kernel/jtrace_config.h>
#include <kernel/persistent_ram.h>
#include <kernel/spinlock.h>
#include <kernel/topology.h>
#include <ktl/algorithm.h>
#include <ktl/atomic.h>
#include <ktl/byte.h>
#include <lk/init.h>
#include <lk/main.h>
#include <object/resource_dispatcher.h>
#include <phys/handoff.h>
#include <platform/crashlog.h>
#include <platform/mapped_crashlog.h>
#include <platform/mexec.h>
#include <platform/timer.h>
#include <platform/uart.h>
#include <vm/kstack.h>
#include <vm/physmap.h>
#include <vm/vm.h>
#include <vm/vm_aspace.h>

#if WITH_PANIC_BACKTRACE
#include <kernel/thread.h>
#endif

#include <lib/zbi-format/zbi.h>
#include <lib/zbitl/image.h>
#include <lib/zbitl/memory.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/syscalls/smc.h>
#include <zircon/types.h>

#define LOCAL_TRACE 0

#include <ktl/enforce.h>

namespace {

// Enable feature to probe for parked cpu cores via SBI to build
// a fallback topology tree in case one was not passed in from
// the bootloader.
// TODO(https://fxbug.dev/42079665): Remove this hack once boot shim detects cpus via device tree.
constexpr bool ENABLE_SBI_TOPOLOGY_DETECT_FALLBACK = true;

void* ramdisk_base;
size_t ramdisk_size;

ktl::atomic<int> panic_started;
ktl::atomic<int> halted;

lazy_init::LazyInit<MappedCrashlog, lazy_init::CheckType::None, lazy_init::Destructor::Disabled>
    mapped_crashlog;

}  // anonymous namespace

bool IsEfiExpected() { return false; }

static void halt_other_cpus() {
  if (halted.exchange(1) == 0) {
    // stop the other cpus
    printf("stopping other cpus\n");
    arch_mp_send_ipi(mp_ipi_target::ALL_BUT_LOCAL, 0, mp_ipi::HALT);

    // spin for a while
    // TODO: find a better way to spin at this low level
    for (int i = 0; i < 100000000; i++) {
      arch::Yield();
    }
  }
}

// TODO(https://fxbug.dev/42180675): Refactor platform_panic_start.
void platform_panic_start(PanicStartHaltOtherCpus option) {
  arch_disable_ints();
  dlog_panic_start();

  if (option == PanicStartHaltOtherCpus::Yes) {
    halt_other_cpus();
  }

  if (panic_started.exchange(1) == 0) {
    dlog_bluescreen_init();
    // Attempt to dump the current debug trace buffer, if we have one.
    jtrace_dump(jtrace::TraceBufferType::Current);
  }
}

void* platform_get_ramdisk(size_t* size) {
  if (ramdisk_base) {
    *size = ramdisk_size;
    return ramdisk_base;
  } else {
    *size = 0;
    return nullptr;
  }
}

void platform_halt_cpu() {
  zx_status_t status = power_cpu_off();

  // Should not have returned
  panic("power_cpu_off returned %d\n", status);
}

bool platform_supports_suspend_cpu() { return false; }

zx_status_t platform_suspend_cpu(PlatformAllowDomainPowerDown allow_domain) {
  return ZX_ERR_NOT_SUPPORTED;
}

zx::result<power_cpu_state> platform_get_cpu_state(cpu_num_t cpu_id) {
  DEBUG_ASSERT(cpu_id < SMP_MAX_CPUS);
  return power_get_cpu_state(arch_cpu_num_to_hart_id(cpu_id));
}

static void topology_cpu_init() {
  DEBUG_ASSERT(arch_max_num_cpus() > 0);
  lk_init_secondary_cpus(arch_max_num_cpus() - 1);

  for (auto* node : system_topology::GetSystemTopology().processors()) {
    if (node->entity.discriminant != ZBI_TOPOLOGY_ENTITY_PROCESSOR ||
        node->entity.processor.architecture_info.discriminant !=
            ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64) {
      panic("Invalid processor node.");
    }

    const auto& processor = node->entity.processor;
    for (uint8_t i = 0; i < processor.logical_id_count; i++) {
      const uint64_t hart_id = processor.architecture_info.riscv64.hart_id;
      DEBUG_ASSERT(hart_id <= UINT32_MAX);

      // Skip the current (boot) hart, we are only starting secondary harts.
      if (processor.flags == ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY ||
          hart_id == riscv64_boot_hart_id()) {
        continue;
      }

      // Try to start the hart.
      riscv64_start_cpu(processor.logical_ids[i], static_cast<uint32_t>(hart_id));
    }
  }
}

// clang-format off
static constexpr zbi_topology_node_t kFallbackTopology = {
  .entity = {
    .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
    .processor = {
      .architecture_info = {
        .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
        .riscv64 = {
          .hart_id = 0,
        }
      },
      .flags = ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY,
      .logical_ids = {0},
      .logical_id_count = 1,
    }
  },
  .parent_index = ZBI_TOPOLOGY_NO_PARENT,
};
// clang-format on

static zx::result<fbl::Array<zbi_topology_node_t>> sbi_detect_topology(size_t max_cpus) {
  DEBUG_ASSERT(max_cpus > 0 && max_cpus <= SMP_MAX_CPUS);

  arch::HartId detected_harts[SMP_MAX_CPUS]{};

  // record the first known hart, that we're by definition running on
  detected_harts[0] = riscv64_curr_hart_id();
  size_t detected_hart_count = 1;

  DEBUG_ASSERT(arch_curr_cpu_num() == 0);

  dprintf(INFO, "RISCV: probing for stopped harts\n");

  // probe the first SMP_MAX_CPUS harts and see which ones are present according to SBI
  // NOTE: assumes that harts are basically 0 numbered, which will not be the case always.
  // This may also detect harts that we're not supposed to run on, such as machine mode only
  // harts intended for embedded use.
  for (arch::HartId i = 0; i < SMP_MAX_CPUS; i++) {
    // Stop if we've detected the clamped max cpus, including the boot cpu
    if (detected_hart_count == max_cpus) {
      break;
    }

    // skip the current cpu, it's known to be present
    if (i == riscv64_curr_hart_id()) {
      continue;
    }

    arch::RiscvSbiRet ret = arch::RiscvSbi::HartGetStatus(i);
    if (ret.error != arch::RiscvSbiError::kSuccess) {
      continue;
    }

    if (ret.value == static_cast<intptr_t>(arch::RiscvSbiHartState::kStopped)) {
      // this is a core that exists but is stopped, add it to the list
      detected_harts[detected_hart_count] = i;
      detected_hart_count++;
      dprintf(INFO, "RISCV: detected stopped hart %lu\n", i);
    }
  }

  // Construct a flat topology tree based on what was found
  fbl::AllocChecker ac;
  auto nodes = fbl::MakeArray<zbi_topology_node_t>(&ac, detected_hart_count);
  if (!ac.check()) {
    return zx::error_result(ZX_ERR_NO_MEMORY);
  }
  for (size_t i = 0; i < detected_hart_count; i++) {
    // clang-format off
    nodes[i] = {
      .entity = {
        .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
        .processor = {
          .architecture_info = {
            .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
            .riscv64 = {
              .hart_id = detected_harts[i],
            },
          },
          .flags = (i == 0) ? ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY : zbi_topology_processor_flags_t{0},
          .logical_ids = { static_cast<uint16_t>(i) },
          .logical_id_count = 1,
        }
      },
      .parent_index = ZBI_TOPOLOGY_NO_PARENT,
    };
    // clang-format on
  }

  return zx::ok(ktl::move(nodes));
}

static void init_topology(uint level) {
  ktl::span handoff = gPhysHandoff->cpu_topology.get();

  // Read the max cpu count from the command line and clamp it to reasonable values.
  uint32_t max_cpus = BootOptions::Get()->smp_max_cpus;
  if (max_cpus != SMP_MAX_CPUS) {
    dprintf(INFO, "SMP: command line setting maximum cpus to %u\n", max_cpus);
  }
  if (max_cpus > SMP_MAX_CPUS || max_cpus == 0) {
    printf("SMP: invalid kernel.smp.maxcpus value (%u), clamping to %d\n", max_cpus, SMP_MAX_CPUS);
    max_cpus = SMP_MAX_CPUS;
  }

  // TODO-rvbringup: clamp the topology tree passed from the bootloader to max_cpus.

  // Try to initialize the system topology from a tree passed from the bootloader.
  zx_status_t result =
      system_topology::Graph::InitializeSystemTopology(handoff.data(), handoff.size());
  if (result != ZX_OK) {
    // Only attempt to use the SBI fallback if our global allow define is set and we're
    // running on QEMU.
    if (ENABLE_SBI_TOPOLOGY_DETECT_FALLBACK && gPhysHandoff->platform_id &&
        strcmp(gPhysHandoff->platform_id.to_std()->board_name, "qemu-riscv64") == 0) {
      printf(
          "SMP: Failed to initialize system topolgy from handoff data, probing for secondary cpus via SBI\n");

      // Use SBI to try to detect secondary cpus.
      zx::result<fbl::Array<zbi_topology_node_t>> topo = sbi_detect_topology(max_cpus);
      if (topo.is_ok()) {
        // Assume the synthesized topology tree only contains processor nodes and thus
        // the size of the array is the total detected cpu count.
        const size_t detected_hart_count = topo->size();
        DEBUG_ASSERT(detected_hart_count > 0 && detected_hart_count <= max_cpus);

        // Set the detected topology.
        result =
            system_topology::Graph::InitializeSystemTopology(topo->data(), detected_hart_count);
        ASSERT(result == ZX_OK);
      } else {
        result = topo.error_value();
      }
    }
  }

  if (result != ZX_OK) {
    printf("SMP: Failed to initialize system topology, error: %d, using fallback topology\n",
           result);

    // Try to fallback to a topology of just this processor.
    result = system_topology::Graph::InitializeSystemTopology(&kFallbackTopology, 1);
    ASSERT(result == ZX_OK);
  }

  arch_set_num_cpus(static_cast<uint>(system_topology::GetSystemTopology().processor_count()));

  // Print the detected cpu topology.
  if (DPRINTF_ENABLED_FOR_LEVEL(INFO)) {
    size_t cpu_num = 0;
    for (auto* proc : system_topology::GetSystemTopology().processors()) {
      auto& info = proc->entity.processor.architecture_info.riscv64;
      dprintf(INFO, "System topology: CPU %zu Hart %lu%s\n", cpu_num++, info.hart_id,
              (info.hart_id == riscv64_curr_hart_id()) ? " boot" : "");
    }
  }
}

LK_INIT_HOOK(init_topology, init_topology, LK_INIT_LEVEL_VM)

static void allocate_persistent_ram(ktl::span<ktl::byte> range) {
  // Figure out how to divide up our persistent RAM.  Right now there are
  // three potential users:
  //
  // 1) The crashlog.
  // 2) Persistent debug logging.
  // 3) Persistent debug tracing.
  //
  // Persistent debug logging and tracing have target amounts of RAM they would
  // _like_ to have, and crash-logging has a minimum amount it is guaranteed to
  // get.  Additionally, all allocated are made in a chunks of the minimum
  // persistent RAM allocation granularity.
  //
  // Make sure that the crashlog gets as much of its minimum allocation as is
  // possible.  Then attempt to satisfy the target for persistent debug logging,
  // followed by persistent debug tracing.  Finally, give anything leftovers to
  // the crashlog.
  size_t crashlog_size = 0;
  size_t pdlog_size = 0;
  size_t jtrace_size = 0;
  {
    // start by figuring out how many chunks of RAM we have available to
    // us total.
    size_t persistent_chunks_available = range.size_bytes() / kPersistentRamAllocationGranularity;

    // If we have not already configured a non-trivial crashlog implementation
    // for the platform, make sure that crashlog gets its minimum allocation, or
    // all of the RAM if it cannot meet even its minimum allocation.
    size_t crashlog_chunks = !PlatformCrashlog::HasNonTrivialImpl()
                                 ? ktl::min(persistent_chunks_available,
                                            kMinCrashlogSize / kPersistentRamAllocationGranularity)
                                 : 0;
    persistent_chunks_available -= crashlog_chunks;

    // Next in line is persistent debug logging.
    size_t pdlog_chunks =
        ktl::min(persistent_chunks_available,
                 kTargetPersistentDebugLogSize / kPersistentRamAllocationGranularity);
    persistent_chunks_available -= pdlog_chunks;

    // Next up is persistent debug tracing.
    size_t jtrace_chunks =
        ktl::min(persistent_chunks_available,
                 kJTraceTargetPersistentBufferSize / kPersistentRamAllocationGranularity);
    persistent_chunks_available -= jtrace_chunks;

    // Finally, anything left over can go to the crashlog.
    crashlog_chunks += persistent_chunks_available;

    crashlog_size = crashlog_chunks * kPersistentRamAllocationGranularity;
    pdlog_size = pdlog_chunks * kPersistentRamAllocationGranularity;
    jtrace_size = jtrace_chunks * kPersistentRamAllocationGranularity;
  }

  // Configure up the crashlog RAM
  if (crashlog_size > 0) {
    dprintf(INFO, "Crashlog configured with %" PRIu64 " bytes\n", crashlog_size);
    mapped_crashlog.Initialize(ktl::span{range.data(), crashlog_size});
    PlatformCrashlog::Bind(mapped_crashlog.Get());
  }

  ktl::byte* leftover_base = range.data() + crashlog_size;

  // Configure the persistent debuglog RAM (if we have any)
  if (pdlog_size > 0) {
    dprintf(INFO, "Persistent debug logging enabled and configured with %" PRIu64 " bytes\n",
            pdlog_size);
    persistent_dlog_set_location(leftover_base, pdlog_size);
    leftover_base += pdlog_size;
  }

  // Do _not_ attempt to set the location of the debug trace buffer if this is
  // not a persistent debug trace buffer.  The location of a non-persistent
  // trace buffer would have been already set during (very) early init.
  if constexpr (kJTraceIsPersistent == jtrace::IsPersistent::Yes) {
    jtrace_set_location(reinterpret_cast<void*>(leftover_base), jtrace_size);
  }
}

void platform_early_init() {
  if (!gPhysHandoff->nvram.empty()) {
    allocate_persistent_ram(gPhysHandoff->nvram.get());
  }

  // Initialize the PmmChecker now that the cmdline has been parsed.
  pmm_checker_init_from_cmdline();

  ASSERT(pmm_init(gPhysHandoff->memory.get()) == ZX_OK);
}

void platform_prevm_init() {}

// Called after the heap is up but before the system is multithreaded.
void platform_init_pre_thread(uint) {}

LK_INIT_HOOK(platform_init_pre_thread, platform_init_pre_thread, LK_INIT_LEVEL_VM)

void platform_init() { topology_cpu_init(); }

// after the fact create a region to reserve the peripheral map(s)
static void platform_init_postvm(uint level) {}

LK_INIT_HOOK(platform_postvm, platform_init_postvm, LK_INIT_LEVEL_VM)

void platform_specific_halt(platform_halt_action suggested_action, zircon_crash_reason_t reason,
                            bool halt_on_panic) {
  TRACEF("suggested_action %u, reason %u, halt_on_panic %d\n", suggested_action,
         static_cast<unsigned int>(reason), halt_on_panic);
  if (suggested_action == HALT_ACTION_REBOOT) {
    power_reboot((reason == ZirconCrashReason::NoCrash) ? power_reboot_flags::REBOOT_NORMAL
                                                        : power_reboot_flags::REBOOT_PANIC);
    printf("reboot failed\n");
  } else if (suggested_action == HALT_ACTION_REBOOT_BOOTLOADER) {
    power_reboot(power_reboot_flags::REBOOT_BOOTLOADER);
    printf("reboot-bootloader failed\n");
  } else if (suggested_action == HALT_ACTION_REBOOT_RECOVERY) {
    power_reboot(power_reboot_flags::REBOOT_RECOVERY);
    printf("reboot-recovery failed\n");
  } else if (suggested_action == HALT_ACTION_SHUTDOWN) {
    power_shutdown();
    printf("shutdown failed\n");
  }

  if (reason == ZirconCrashReason::Panic) {
    Backtrace bt;
    Thread::Current::GetBacktrace(bt);
    bt.Print();
    if (!halt_on_panic) {
      power_reboot(power_reboot_flags::REBOOT_PANIC);
      printf("reboot failed\n");
    }
    dprintf(ALWAYS, "CRASH: starting debug shell... (reason = %d)\n", static_cast<int>(reason));
    arch_disable_ints();
    panic_shell_start();
  }

  dprintf(ALWAYS, "HALT: spinning forever... (reason = %d)\n", static_cast<int>(reason));

  // catch all fallthrough cases
  arch_disable_ints();

  for (;;) {
    arch::Yield();
  }
}

zx_status_t platform_mexec_patch_zbi(uint8_t* zbi, const size_t len) { PANIC_UNIMPLEMENTED; }

void platform_mexec_prep(uintptr_t new_bootimage_addr, size_t new_bootimage_len) {
  PANIC_UNIMPLEMENTED;
}

void platform_mexec(mexec_asm_func mexec_assembly, ktl::span<const memmov_ops_t> ops,
                    uintptr_t new_kernel_addr, size_t new_kernel_len, uintptr_t new_kernel_entry,
                    uintptr_t new_data_zbi_addr, size_t new_data_zbi_len) {
  PANIC_UNIMPLEMENTED;
}

// Initialize Resource system after the heap is initialized.
static void riscv64_resource_dispatcher_init_hook(unsigned int rl) {
  // 64 bit address space for MMIO on RISCV64
  zx_status_t status = ResourceDispatcher::InitializeAllocator(ZX_RSRC_KIND_MMIO, 0, UINT64_MAX);
  if (status != ZX_OK) {
    printf("Resources: Failed to initialize MMIO allocator: %d\n", status);
  }
  // Set up IRQs based on values from the PLIC
  const auto max_vector = interrupt_get_max_vector();
  // Normally there would be at least one interrupt vector.
  DEBUG_ASSERT(max_vector > 0);
  status = ResourceDispatcher::InitializeAllocator(ZX_RSRC_KIND_IRQ, interrupt_get_base_vector(),
                                                   max_vector);
  if (status != ZX_OK) {
    printf("Resources: Failed to initialize IRQ allocator: %d\n", status);
  }
  // Set up range of valid system resources.
  status = ResourceDispatcher::InitializeAllocator(ZX_RSRC_KIND_SYSTEM, 0, ZX_RSRC_SYSTEM_COUNT);
  if (status != ZX_OK) {
    printf("Resources: Failed to initialize system allocator: %d\n", status);
  }
}

LK_INIT_HOOK(riscv64_resource_init, riscv64_resource_dispatcher_init_hook, LK_INIT_LEVEL_INTC)

void topology_init() {
  // Setup the CPU distance map with the already initialized topology.
  const auto processor_count =
      static_cast<uint>(system_topology::GetSystemTopology().processor_count());
  CpuDistanceMap::Initialize(processor_count, [](cpu_num_t from_id, cpu_num_t to_id) { return 0; });

  const CpuDistanceMap::Distance kDistanceThreshold = 2u;
  CpuDistanceMap::Get().set_distance_threshold(kDistanceThreshold);

  CpuDistanceMap::Get().Dump();
}

zx_status_t platform_mp_prep_cpu_unplug(cpu_num_t cpu_id) {
  return arch_mp_prep_cpu_unplug(cpu_id);
}

zx_status_t platform_mp_cpu_unplug(cpu_num_t cpu_id) { return arch_mp_cpu_unplug(cpu_id); }

zx_status_t platform_append_mexec_data(ktl::span<ktl::byte> data_zbi) { return ZX_OK; }

ktl::optional<uint32_t> PlatformUartGetIrqNumber(uint32_t irq_num) { return irq_num; }
