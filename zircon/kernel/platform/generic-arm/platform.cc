// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2015 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <arch.h>
#include <debug.h>
#include <lib/arch/intrin.h>
#include <lib/boot-options/boot-options.h>
#include <lib/console.h>
#include <lib/control-region.h>
#include <lib/crashlog.h>
#include <lib/debuglog.h>
#include <lib/instrumentation/asan.h>
#include <lib/jtrace/jtrace.h>
#include <lib/memalloc/range.h>
#include <lib/persistent-debuglog.h>
#include <lib/system-topology.h>
#include <lib/zbi-format/kernel.h>
#include <platform.h>
#include <string-file.h>
#include <trace.h>
#include <zircon/time.h>

#include <cstdint>

#include <arch/arch_ops.h>
#include <arch/arm64.h>
#include <arch/arm64/mmu.h>
#include <arch/arm64/mp.h>
#include <arch/arm64/periphmap.h>
#include <arch/mp.h>
#include <dev/clocks_and_pmic.h>
#include <dev/hw_rng.h>
#include <dev/interrupt.h>
#include <dev/power.h>
#include <dev/psci.h>
#include <explicit-memory/bytes.h>
#include <fbl/ref_ptr.h>
#include <kernel/cpu.h>
#include <kernel/cpu_distance_map.h>
#include <kernel/dpc.h>
#include <kernel/persistent_ram.h>
#include <kernel/spinlock.h>
#include <kernel/topology.h>
#include <ktl/algorithm.h>
#include <ktl/atomic.h>
#include <ktl/byte.h>
#include <ktl/span.h>
#include <ktl/variant.h>
#include <lk/init.h>
#include <object/resource_dispatcher.h>
#include <platform/crashlog.h>
#include <platform/debug.h>
#include <platform/mexec.h>
#include <vm/handoff-end.h>
#include <vm/kstack.h>
#include <vm/physmap.h>
#include <vm/vm.h>
#include <vm/vm_aspace.h>

#include <ktl/enforce.h>

#if WITH_PANIC_BACKTRACE
#include <kernel/thread.h>
#endif

#include <lib/arch/intrin.h>
#include <lib/zbi-format/zbi.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/syscalls/smc.h>
#include <zircon/types.h>

#include <platform/mapped_crashlog.h>

#define LOCAL_TRACE 0

static ktl::atomic<int> panic_started;
static ktl::atomic<int> halted;

// Whether this platform implementation supports CPU suspend.
static bool cpu_suspend_supported = false;

namespace {

lazy_init::LazyInit<MappedCrashlog, lazy_init::CheckType::None, lazy_init::Destructor::Disabled>
    mapped_crashlog;

// Used for |platform_suspend_cpu| coordination.
ControlRegion gControlRegion;

}  // namespace

static void halt_other_cpus(void) {
  if (halted.exchange(1) == 0) {
    // stop the other cpus
    printf("stopping other cpus\n");
    arch_mp_send_ipi(mp_ipi_target::ALL_BUT_LOCAL, 0, mp_ipi::HALT);

    // spin for a while
    // TODO: find a better way to spin at this low level
    for (int i = 0; i < 100000000; i = i + 1) {
      arch::Yield();
    }
  }
}

// Difference on SMT systems is that the AFF0 (cpu_id) level is implicit and not stored in the info.
static uint64_t ToSmtMpid(const zbi_topology_processor_t& processor, uint8_t cpu_id) {
  DEBUG_ASSERT(processor.architecture_info.discriminant == ZBI_TOPOLOGY_ARCHITECTURE_INFO_ARM64);
  const auto& info = processor.architecture_info.arm64;
  return (uint64_t)info.cluster_3_id << 32 | info.cluster_2_id << 16 | info.cluster_1_id << 8 |
         cpu_id;
}

static uint64_t ToMpid(const zbi_topology_processor_t& processor) {
  DEBUG_ASSERT(processor.architecture_info.discriminant == ZBI_TOPOLOGY_ARCHITECTURE_INFO_ARM64);
  const auto& info = processor.architecture_info.arm64;
  return (uint64_t)info.cluster_3_id << 32 | info.cluster_2_id << 16 | info.cluster_1_id << 8 |
         info.cpu_id;
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

void platform_halt_cpu(void) {
  // TODO(https://fxbug.dev/450973098): Considering modeling offline CPUs as already in the control
  // region.
  gControlRegion.Unregister();

  uint32_t result = power_cpu_off();
  // should have never returned
  panic("power_cpu_off returned %u\n", result);
}

bool platform_supports_suspend_cpu() { return cpu_suspend_supported; }

// TODO(https://fxbug.dev/417558115): Expand to include a deadline parameter
// that's used to wake the CPU based on the boot time clock.
zx_status_t platform_suspend_cpu(PlatformAllowDomainPowerDown allow_domain) {
  LTRACEF("platform_suspend_cpu cpu-%u current_boot_time=%ld allow_domain=%d\n",
          arch_curr_cpu_num(), current_boot_time(), static_cast<int>(allow_domain));

  // TODO(https://fxbug.dev/458140022): Once we have figured out the cause of "negative time" we can
  // remove this assert.
  [[maybe_unused]] const zx_instant_boot_ticks_t boot_ticks_before = current_boot_ticks();
  DEBUG_ASSERT_MSG(boot_ticks_before >= 0, "boot_ticks_before=%" PRId64, boot_ticks_before);

  DEBUG_ASSERT(!Thread::Current::Get()->preemption_state().PreemptIsEnabled());
  DEBUG_ASSERT(arch_ints_disabled());
  // Make sure this thread is a kernel-only thread.
  // Otherwise, we might need to save and restore some vector/floating-point
  // state if we are going to power down.
  DEBUG_ASSERT(Thread::Current::Get()->user_thread() == nullptr);

  if (!cpu_suspend_supported) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  const bool cpu_might_power_down = psci_might_powerdown();

  const PsciCpuSuspendMaxScope max_scope = static_cast<bool>(allow_domain)
                                               ? PsciCpuSuspendMaxScope::CpuAndMore
                                               : PsciCpuSuspendMaxScope::CpuOnly;

  if (cpu_might_power_down) {
    lockup_percpu_shutdown();
    platform_suspend_timer_curr_cpu();
    suspend_interrupts_curr_cpu();
  }

  PsciCpuSuspendResult result;

  {
    const bool last_to_enter = !gControlRegion.TryEnter();
    if (last_to_enter) {
      if (allow_domain == PlatformAllowDomainPowerDown::Yes) {
        platform_serial_prepare_for_suspend();
        clocks_and_pmic_prepare_for_suspend();
      }
      gControlRegion.CloseEnter();
    }

    const uint32_t power_state = psci_select_cpu_suspend_power_state(max_scope, last_to_enter);

    LTRACEF("platform_suspend_cpu for cpu-%u, current_boot_time=%ld, suspending...\n",
            arch_curr_cpu_num(), current_boot_time());
    // The following call may not return for an arbitrartily long time.
    result = psci_cpu_suspend(power_state);
    LTRACEF("psci_cpu_suspend for cpu-%u, status %d\n", arch_curr_cpu_num(), result.status_value());

    DEBUG_ASSERT(arch_ints_disabled());

    const bool first_to_leave = !gControlRegion.TryLeave();
    if (first_to_leave) {
      // Serial wakeup is an idempotent operation so it's OK to call this even in the case where the
      // last to enter did not prepare serial for suspend (e.g. because it was called with
      // |PsciCpuSuspendMaxScope::CpuOnly|).
      clocks_and_pmic_wakeup_from_suspend();
      platform_serial_wakeup_from_suspend();

      // TODO(https://fxbug.dev/458140022): Once we have figured out the cause of "negative time" we
      // can remove this assert.
      [[maybe_unused]] const zx_instant_boot_ticks_t boot_ticks_after = current_boot_ticks();
      DEBUG_ASSERT_MSG(boot_ticks_after >= 0, "boot_ticks_after=%" PRId64, boot_ticks_after);
      DEBUG_ASSERT_MSG(boot_ticks_before <= boot_ticks_after,
                       "boot_ticks_before=%" PRId64 " boot_ticks_after=%" PRId64, boot_ticks_before,
                       boot_ticks_after);

      gControlRegion.OpenLeave();
    }
  }

  if (cpu_might_power_down) {
    zx_status_t status = resume_interrupts_curr_cpu();
    DEBUG_ASSERT_MSG(status == ZX_OK, "resume_interrupts_curr_cpu: %d", status);
    status = platform_resume_timer_curr_cpu();
    DEBUG_ASSERT_MSG(status == ZX_OK, "platform_resume_timer_curr_cpu: %d", status);
    lockup_percpu_init();
  } else {
    // If the requested power state isn't a "power down" power state, then make
    // sure we did not in fact power down.
    DEBUG_ASSERT(result.is_error() || result.value() == CpuPoweredDown::No);
  }

  LTRACEF("platform_suspend_cpu for cpu-%u current_boot_time=%ld, done\n", arch_curr_cpu_num(),
          current_boot_time());

  return result.status_value();
}

zx_status_t platform_start_cpu(cpu_num_t cpu_id, uint64_t mpid, uint64_t context) {
  // TODO(https://fxbug.dev/450973098): Considering modeling offline CPUs as already in the control
  // region.
  gControlRegion.Register();

  paddr_t kernel_secondary_entry_paddr = KernelPhysicalAddressOf<arm64_secondary_start>();
  uint32_t ret = power_cpu_on(mpid, kernel_secondary_entry_paddr, context);
  dprintf(INFO, "Trying to start cpu %u, mpid %#" PRIx64 " returned: %d\n", cpu_id, mpid, (int)ret);
  if (ret != 0) {
    return ZX_ERR_INTERNAL;
  }
  return ZX_OK;
}

zx::result<power_cpu_state> platform_get_cpu_state(cpu_num_t cpu_id) {
  DEBUG_ASSERT(cpu_id < SMP_MAX_CPUS);
  return power_get_cpu_state(arch_cpu_num_to_mpidr(cpu_id));
}

static void topology_cpu_init(void) {
  // We need booted secondary CPUs - *before* they enable their caches - to
  // have a view of the relevant memory that's coherent with the boot CPU. It
  // should suffice to ensure that (1) the code the secondary CPUs would touch
  // before enabling data caches and (2) the variables it loads are cleaned to
  // the point of coherency. While we could be surgical about that, it suffices
  // to simply clean the whole kernel load image, which surely includes (1) and
  // (2).
  arch_clean_cache_range(reinterpret_cast<vaddr_t>(__executable_start),
                         static_cast<size_t>(_end - __executable_start));

  for (auto* node : system_topology::GetSystemTopology().processors()) {
    if (node->entity.discriminant != ZBI_TOPOLOGY_ENTITY_PROCESSOR ||
        node->entity.processor.architecture_info.discriminant !=
            ZBI_TOPOLOGY_ARCHITECTURE_INFO_ARM64) {
      panic("Invalid processor node.");
    }

    zx_status_t status;
    const auto& processor = node->entity.processor;
    for (uint8_t i = 0; i < processor.logical_id_count; i++) {
      const uint64_t mpid =
          (processor.logical_id_count > 1) ? ToSmtMpid(processor, i) : ToMpid(processor);
      arch_register_mpid(processor.logical_ids[i], mpid);

      // Skip processor 0, we are only starting secondary processors.
      if (processor.logical_ids[i] == 0) {
        continue;
      }

      zx::result<uintptr_t> result = arm64_create_secondary_stack(processor.logical_ids[i]);
      DEBUG_ASSERT(result.is_ok());

      // start the cpu
      status = platform_start_cpu(processor.logical_ids[i], mpid, result.value());

      if (status != ZX_OK) {
        // TODO(maniscalco): Is continuing really the right thing to do here?

        // start failed, free the stack
        status = arm64_free_secondary_stack(processor.logical_ids[i]);
        DEBUG_ASSERT(status == ZX_OK);
        continue;
      }
    }
  }
}

static constexpr zbi_topology_node_t fallback_topology = {
    .entity = {.discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
               .processor =
                   {
                       .architecture_info =
                           {
                               .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_ARM64,
                               .arm64 =
                                   {
                                       .cluster_1_id = 0,
                                       .cluster_2_id = 0,
                                       .cluster_3_id = 0,
                                       .cpu_id = 0,
                                       .gic_id = 0,
                                   },
                           },
                       .flags = 0,
                       .logical_ids = {0},
                       .logical_id_count = 1,

                   }},
    .parent_index = ZBI_TOPOLOGY_NO_PARENT,
};

static void init_topology(uint level) {
  ktl::span handoff = gPhysHandoff->cpu_topology.get();

  auto result = system_topology::Graph::InitializeSystemTopology(handoff.data(), handoff.size());
  if (result != ZX_OK) {
    printf("Failed to initialize system topology! error: %d\n", result);

    // Try to fallback to a topology of just this processor.
    result = system_topology::Graph::InitializeSystemTopology(&fallback_topology, 1);
    ASSERT(result == ZX_OK);
  }

  arch_set_num_cpus(static_cast<uint>(system_topology::GetSystemTopology().processor_count()));

  if (DPRINTF_ENABLED_FOR_LEVEL(INFO)) {
    for (auto* proc : system_topology::GetSystemTopology().processors()) {
      auto& info = proc->entity.processor.architecture_info.arm64;
      dprintf(INFO, "System topology: CPU %u:%u:%u:%u\n", info.cluster_3_id, info.cluster_2_id,
              info.cluster_1_id, info.cpu_id);
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

void platform_early_init(void) {
  if (!gPhysHandoff->nvram.empty()) {
    allocate_persistent_ram(gPhysHandoff->nvram.get());
  }

  // Initialize the PmmChecker now that the cmdline has been parsed.
  pmm_checker_init_from_cmdline();

  ASSERT(pmm_init(gPhysHandoff->memory.get()) == ZX_OK);

  // give the mmu code a chance to do some bookkeeping
  arm64_mmu_early_init();
}

void platform_prevm_init() {}

void platform_init(void) {
  if (psci_is_cpu_suspend_supported()) {
    // If this PSCI implementation supports OSI mode, use it.
    if (psci_is_set_suspend_mode_supported()) {
      zx_status_t status = psci_set_suspend_mode(psci_suspend_mode::os_initiated);
      if (status == ZX_OK) {
        dprintf(INFO, "PSCI: using OS initiated suspend mode\n");
      } else if (status == ZX_ERR_NOT_SUPPORTED) {
        dprintf(INFO, "PSCI: OS initiated suspend mode not supported\n");
      } else {
        panic("psci_set_suspend_mode failed with unexpected value %d", status);
      }
    }

    cpu_suspend_supported = true;

    // Normally, |power_cpu_on| is responsible for registering CPUs with the |ControlRegion|.
    // Register this one manually since it's the boot CPU and was not started via |power_cpu_on|.
    DEBUG_ASSERT(arch_curr_cpu_num() == BOOT_CPU_ID);
    gControlRegion.Register();
  }
  dprintf(INFO, "platform_suspend_cpu support %s\n",
          cpu_suspend_supported ? "enabled" : "disabled");

  topology_cpu_init();
}

zx_status_t platform_mp_prep_cpu_unplug(cpu_num_t cpu_id) {
  return arch_mp_prep_cpu_unplug(cpu_id);
}

zx_status_t platform_mp_cpu_unplug(cpu_num_t cpu_id) { return arch_mp_cpu_unplug(cpu_id); }

void platform_specific_halt(platform_halt_action suggested_action, zircon_crash_reason_t reason,
                            bool halt_on_panic) {
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
    __wfi();
  }
}

void platform_mexec_prep(uintptr_t new_bootimage_addr, size_t new_bootimage_len) {
  DEBUG_ASSERT(!arch_ints_disabled());
  DEBUG_ASSERT(mp_get_online_mask() == cpu_num_to_mask(BOOT_CPU_ID));
}

// This function requires NO_ASAN because it accesses ops, which is memory
// that lives outside of the kernel address space (comes from IdAllocator).
NO_ASAN void platform_mexec(mexec_asm_func mexec_assembly, ktl::span<const memmov_ops_t> ops,
                            uintptr_t new_kernel_addr, size_t new_kernel_len,
                            uintptr_t new_kernel_entry, uintptr_t new_data_zbi_addr,
                            size_t new_data_zbi_len) {
  DEBUG_ASSERT(arch_ints_disabled());
  DEBUG_ASSERT(mp_get_online_mask() == cpu_num_to_mask(BOOT_CPU_ID));

  // Clean the destination range for the new kernel, so that nothing overwrites
  // it when caches are re-enabled (in the cases where they are disabled at
  // boot).
  arch_clean_cache_range(reinterpret_cast<vaddr_t>(paddr_to_physmap(new_kernel_addr)),
                         new_kernel_len);

  // Clean the op source ranges, excluding the terminal null entry, ensuring
  // that we do indeed end up copying the intended data when we access it
  // directly from main memory.
  for (const memmov_ops_t& memmov_ops : ops.subspan(0, ops.size() - 1)) {
    paddr_t src = reinterpret_cast<paddr_t>(memmov_ops.src);
    arch_clean_cache_range(reinterpret_cast<vaddr_t>(paddr_to_physmap(src)), memmov_ops.len);
  }

  mexec_assembly(new_data_zbi_addr, 0, 0, arm64_get_boot_el(), ops.data(), new_kernel_entry);
}

// Initialize Resource system after the heap is initialized.
static void arm_resource_dispatcher_init_hook(unsigned int rl) {
  // 64 bit address space for MMIO on ARM64
  zx_status_t status = ResourceDispatcher::InitializeAllocator(ZX_RSRC_KIND_MMIO, 0, UINT64_MAX);
  if (status != ZX_OK) {
    printf("Resources: Failed to initialize MMIO allocator: %d\n", status);
  }
  // Set up IRQs based on values from the GIC
  status = ResourceDispatcher::InitializeAllocator(ZX_RSRC_KIND_IRQ, interrupt_get_base_vector(),
                                                   interrupt_get_max_vector());
  if (status != ZX_OK) {
    printf("Resources: Failed to initialize IRQ allocator: %d\n", status);
  }
  // Set up SMC valid service call range
  status = ResourceDispatcher::InitializeAllocator(ZX_RSRC_KIND_SMC, 0,
                                                   ARM_SMC_SERVICE_CALL_NUM_MAX + 1);
  if (status != ZX_OK) {
    printf("Resources: Failed to initialize SMC allocator: %d\n", status);
  }
  // Set up range of valid system resources.
  status = ResourceDispatcher::InitializeAllocator(ZX_RSRC_KIND_SYSTEM, 0, ZX_RSRC_SYSTEM_COUNT);
  if (status != ZX_OK) {
    printf("Resources: Failed to initialize system allocator: %d\n", status);
  }
}
LK_INIT_HOOK(arm_resource_init, arm_resource_dispatcher_init_hook, LK_INIT_LEVEL_INTC)

void topology_init() {
  // Check MPIDR_EL1.MT to determine how to interpret AFF0 (i.e. cpu_id). For
  // now, assume that MT is set consistently across all PEs in the system. When
  // MT is set, use the next affinity level for the first cache depth element.
  // This approach should be adjusted if we find examples of systems that do not
  // set MT uniformly, and may require delaying cache-aware load balancing until
  // all PEs are initialized.
  const bool cpu_id_is_thread_id = __arm_rsr64("mpidr_el1") & (1 << 24);
  printf("topology_init: MPIDR_EL1.MT=%d\n", cpu_id_is_thread_id);

  // This platform initializes the topology earlier than this standard hook.
  // Setup the CPU distance map with the already initialized topology.
  const auto processor_count =
      static_cast<uint>(system_topology::GetSystemTopology().processor_count());
  CpuDistanceMap::Initialize(processor_count, [cpu_id_is_thread_id](cpu_num_t from_id,
                                                                    cpu_num_t to_id) {
    using system_topology::Node;
    using system_topology::Graph;

    const Graph& topology = system_topology::GetSystemTopology();

    Node* from_node = nullptr;
    if (topology.ProcessorByLogicalId(from_id, &from_node) != ZX_OK) {
      printf("Failed to get processor node for CPU %u\n", from_id);
      return -1;
    }
    DEBUG_ASSERT(from_node != nullptr);

    Node* to_node = nullptr;
    if (topology.ProcessorByLogicalId(to_id, &to_node) != ZX_OK) {
      printf("Failed to get processor node for CPU %u\n", to_id);
      return -1;
    }
    DEBUG_ASSERT(to_node != nullptr);

    const zbi_topology_arm64_info_t& from_info =
        from_node->entity.processor.architecture_info.arm64;
    const zbi_topology_arm64_info_t& to_info = to_node->entity.processor.architecture_info.arm64;

    // Return the maximum cache depth not shared when multithreaded.
    if (cpu_id_is_thread_id) {
      return ktl::max({1 * int{from_info.cluster_1_id != to_info.cluster_1_id},
                       2 * int{from_info.cluster_2_id != to_info.cluster_2_id},
                       3 * int{from_info.cluster_3_id != to_info.cluster_3_id}});
    }

    // Return the maximum cache depth not shared when single threaded.
    return ktl::max({1 * int{from_info.cpu_id != to_info.cpu_id},
                     2 * int{from_info.cluster_1_id != to_info.cluster_1_id},
                     3 * int{from_info.cluster_2_id != to_info.cluster_2_id},
                     4 * int{from_info.cluster_3_id != to_info.cluster_3_id}});
  });

  // TODO(eieio): Determine automatically or provide a way to specify in the
  // ZBI. The current value matches the depth of the first significant cache
  // above.
  const CpuDistanceMap::Distance kDistanceThreshold = 2u;
  CpuDistanceMap::Get().set_distance_threshold(kDistanceThreshold);

  CpuDistanceMap::Get().Dump();
}
