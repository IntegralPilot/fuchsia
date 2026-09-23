// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <debug.h>
#include <lib/boot-options/boot-options.h>
#include <lib/fit/defer.h>
#include <lib/fxt/fields.h>
#include <lib/fxt/interned_category.h>
#include <lib/fxt/interned_string.h>
#include <lib/ktrace.h>
#include <lib/syscalls/zx-syscall-numbers.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <platform.h>
#include <string.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <arch/ops.h>
#include <arch/user_copy.h>
#include <fbl/alloc_checker.h>
#include <hypervisor/ktrace.h>
#include <kernel/ffi.h>
#include <kernel/koid.h>
#include <kernel/mp.h>
#include <ktl/atomic.h>
#include <ktl/iterator.h>
#include <lk/init.h>
#include <object/thread_dispatcher.h>
#include <vm/fault.h>
#include <vm/vm_aspace.h>

#include <ktl/enforce.h>

extern "C" {
zx_status_t rust_ktrace_init(uint32_t num_buffers, void* state_ptr);
zx_status_t rust_ktrace_init_cpu_buffer(uint32_t cpu_num, void* spsc_buffer_ptr,
                                        void* drop_stats_ptr, uint64_t process_koid,
                                        uint64_t thread_koid, uint16_t cpu_ref_header_entry);
}

namespace {

using fxt::operator""_category;

struct CategoryEntry {
  uint32_t index;
  const fxt::InternedCategory& category;
};

const CategoryEntry kCategories[] = {
    {KTRACE_GRP_META_BIT, "kernel:meta"_category},
    {KTRACE_GRP_MEMORY_BIT, "kernel:memory"_category},
    {KTRACE_GRP_SCHEDULER_BIT, "kernel:sched"_category},
    {KTRACE_GRP_CONTENTION_BIT, "kernel:contention"_category},
    {KTRACE_GRP_IPC_BIT, "kernel:ipc"_category},
    {KTRACE_GRP_IRQ_BIT, "kernel:irq"_category},
    {KTRACE_GRP_PROBE_BIT, "kernel:probe"_category},
    {KTRACE_GRP_ARCH_BIT, "kernel:arch"_category},
    {KTRACE_GRP_SYSCALL_BIT, "kernel:syscall"_category},
    {KTRACE_GRP_VM_BIT, "kernel:vm"_category},
    {KTRACE_GRP_RESTRICTED_BIT, "kernel:restricted"_category},
    {KTRACE_GRP_POWER_BIT, "kernel:power"_category},
    {KTRACE_GRP_OOM_BIT, "kernel:oom"_category},
};

void SetupCategoryBits() {
  for (const CategoryEntry& entry : kCategories) {
    if (entry.category.index() == fxt::InternedCategory::kInvalidIndex) {
      entry.category.SetIndex(entry.index);
    } else {
      dprintf(INFO, "Found category \"%s\" already initialized to 0x%04x!\n",
              entry.category.string(), (1u << entry.category.index()));
    }
  }
  // If debug assertions are enabled, validate that all interned categories have been initialized.
  if constexpr (DEBUG_ASSERT_IMPLEMENTED) {
    for (const fxt::InternedCategory& category : fxt::InternedCategory::Iterate()) {
      DEBUG_ASSERT_MSG(category.index() != fxt::InternedCategory::kInvalidIndex,
                       "Interned category %s was not initialized\n", category.string());
    }
  }
}

}  // namespace

KTrace KTrace::instance_;

zx::result<KTrace::Reservation> KTrace::Reserve(uint64_t header) {
  DEBUG_ASSERT(arch_ints_disabled());

  // If writes are disabled, then return an error. We return ZX_ERR_BAD_STATE, because this means
  // that tracing was disabled.
  //
  // It is valid for writes to be disabled immediately after this check. This is ok because Stop,
  // which disables writes, will follow up with an IPI to all cores and wait for those IPIs to
  // return. Because interrupts have been disabled prior to this check, that IPI will not return
  // until this write operation is complete.
  if (!WritesEnabled()) {
    return zx::error(ZX_ERR_BAD_STATE);
  }

  // Check which CPU we're running on and Reserve a slot in the appropriate SPSC buffer.
  const cpu_num_t cpu_num = arch_curr_cpu_num();
  DEBUG_ASSERT(percpu_buffers_ != nullptr);
  return percpu_buffers_[cpu_num].Reserve(header);
}

void KTrace::ReportMetadata() {
  // Emit the FXT metadata records. These must be emitted on the boot CPU to ensure that they
  // are read at the very beginning of the trace.
  auto emit_starting_records = [](void* arg) {
    DEBUG_ASSERT(arch_ints_disabled());

    // Emit the magic and initialization records.
    KTrace* ktrace = static_cast<KTrace*>(arg);
    zx_status_t status = fxt::WriteMagicNumberRecord(ktrace);
    DEBUG_ASSERT(status == ZX_OK);
    status = fxt::WriteInitializationRecord(ktrace, ticks_per_second());
    DEBUG_ASSERT(status == ZX_OK);

    // Emit strings needed to improve readability, such as syscall names, to the trace buffer.
    for (const fxt::InternedString& interned_string : fxt::InternedString::Iterate()) {
      fxt::WriteStringRecord(
          ktrace, interned_string.id(), interned_string.string(),
          strnlen(interned_string.string(), fxt::InternedString::kMaxStringLength));
    }

    // Emit the KOIDs of each CPU to the trace buffer.
    const uint32_t max_cpus = arch_max_num_cpus();
    char name[32];
    for (uint32_t i = 0; i < max_cpus; i++) {
      snprintf(name, sizeof(name), "cpu-%u", i);
      fxt::WriteKernelObjectRecord(ktrace, fxt::Koid(ktrace->cpu_context_map_.GetCpuKoid(i)),
                                   ZX_OBJ_TYPE_THREAD, fxt::StringRef{name},
                                   fxt::Argument{"process"_intern, kNoProcess},
                                   fxt::Argument{"job"_intern, kNoProcess});
    }
  };
  const cpu_mask_t target_mask = cpu_num_to_mask(BOOT_CPU_ID);
  mp_sync_exec(mp_ipi_target::MASK, target_mask, emit_starting_records, &GetInstance());

  // Emit the names of all live processes and threads to the trace buffer. Note that these records
  // will be inserted into the buffer associated with the CPU we're running on, which may not be
  // the boot CPU. Fortunately for us, these process and thread names, unlike the other metadata
  // records, do not need to exist before any records that reference them are emitted.
  ktrace_report_live_processes();
  ktrace_report_live_threads();
}

zx_status_t KTrace::Start(uint32_t action, uint32_t categories) {
  Guard<Mutex> guard{&lock_};

  if (!percpu_buffers_) {
    // Perform the allocations of the buffers here in the stack without any locks held as the
    // allocation process may need to block waiting for memory.
    uint32_t num_buffers = num_buffers_;
    uint32_t buffer_size = buffer_size_;
    zx_status_t status = ZX_OK;
    ktl::unique_ptr<percpu_writer::Buffer[]> buffers;
    guard.CallUnlocked([&]() {
      fbl::AllocChecker ac;
      buffers = ktl::make_unique<percpu_writer::Buffer[]>(&ac, num_buffers);
      if (!ac.check()) {
        status = ZX_ERR_NO_MEMORY;
        return;
      }

      // Initialize each per-CPU buffer by allocating the storage used to back it.
      for (uint32_t i = 0; i < num_buffers; i++) {
        const zx_status_t init_status =
            buffers[i].Init(buffer_size, "ktrace", cpu_context_map_.GetCpuRef(i));
        if (init_status != ZX_OK) {
          // Any allocated buffers will be destructed when we return.
          DiagsPrintf(INFO, "ktrace: cannot alloc buffer %u: %d\n", i, init_status);
          status = ZX_ERR_NO_MEMORY;
          return;
        }
      }
    });
    // Propagate any errors.
    if (status != ZX_OK) {
      return status;
    }
    // May have raced with another call to Start, in which case we'll just drop the buffers we
    // allocated and use the new ones we found.
    if (!percpu_buffers_) {
      percpu_buffers_ = ktl::move(buffers);
      for (uint32_t i = 0; i < num_buffers_; i++) {
        const auto& cpu_ref = cpu_context_map_.GetCpuRef(i);
        rust_ktrace_init_cpu_buffer(i, percpu_buffers_[i].spsc_buffer(),
                                    percpu_buffers_[i].drop_stats_ptr(), cpu_ref.process().koid,
                                    cpu_ref.thread().koid,
                                    static_cast<uint16_t>(cpu_ref.HeaderEntry()));
      }
    }
  }

  ASSERT(percpu_buffers_);

  // If writes are already enabled, then a trace session is already in progress and all we need to
  // do is set the categories bitmask and return.
  if (WritesEnabled()) {
    set_categories_bitmask(categories);
    return ZX_OK;
  }

  // Otherwise, enable writes.
  EnableWrites();

  // Report static metadata before setting the categories bitmask.
  // These metadata records must be emitted before we enable arbitrary categories, otherwise generic
  // trace records may fill up the buffer and cause these metadata records to be dropped, which
  // could make the trace unreadable.
  ReportMetadata();

  set_categories_bitmask(categories);
  DiagsPrintf(INFO, "Enabled category mask: 0x%03x\n", categories);
  DiagsPrintf(INFO, "Trace category states:\n");
  for (const fxt::InternedCategory& category : fxt::InternedCategory::Iterate()) {
    DiagsPrintf(INFO, "  %-20s : 0x%03x : %s\n", category.string(), (1u << category.index()),
                IsCategoryEnabled(category) ? "enabled" : "disabled");
  }

  return ZX_OK;
}

void KTrace::Init(uint32_t bufsize, uint32_t initial_grpmask) {
  {
    Guard<Mutex> guard{&lock_};

    ASSERT_MSG(buffer_size_ == 0, "KTrace::Init called twice");
    // Allocate the KOIDs used to annotate CPU trace records.
    cpu_context_map_.Init();

    // Compute the per-CPU buffer size, ensuring that the resulting value is a power of two.
    num_buffers_ = arch_max_num_cpus();
    const uint32_t raw_percpu_bufsize = bufsize / num_buffers_;
    DEBUG_ASSERT(raw_percpu_bufsize > 0);
    const int leading_zeros = __builtin_clz(raw_percpu_bufsize);
    buffer_size_ = 1u << (31 - leading_zeros);
    rust_ktrace_init(num_buffers_, &state_);
  }

  // If the initial_grpmask was zero, then we can delay allocation of the KTrace buffer.
  if (initial_grpmask == 0) {
    return;
  }
  // Otherwise, begin tracing immediately.
  Start(KTRACE_ACTION_START, initial_grpmask);
}

void KTrace::RestoreRustSingleton() {
  Guard<Mutex> guard{&lock_};
  if (num_buffers_ == 0) {
    return;
  }
  rust_ktrace_init(num_buffers_, &state_);
  if (percpu_buffers_) {
    for (uint32_t i = 0; i < num_buffers_; i++) {
      const auto& cpu_ref = cpu_context_map_.GetCpuRef(i);
      rust_ktrace_init_cpu_buffer(i, percpu_buffers_[i].spsc_buffer(),
                                  percpu_buffers_[i].drop_stats_ptr(), cpu_ref.process().koid,
                                  cpu_ref.thread().koid,
                                  static_cast<uint16_t>(cpu_ref.HeaderEntry()));
    }
  }
}

extern "C" void ktrace_restore_rust_singleton() { KTrace::GetInstance().RestoreRustSingleton(); }

zx_status_t KTrace::Stop() {
  Guard<Mutex> guard{&lock_};

  // Calling Stop on an uninitialized KTrace buffer is a no-op.
  if (!percpu_buffers_) {
    return ZX_OK;
  }

  // Clear the categories bitmask and disable writes. This prevents any new writes from starting.
  set_categories_bitmask(0u);
  DisableWrites();

  // Wait for any in-progress writes to complete and emit any dropped record statistics.
  // We accomplish this by sending an IPI to all cores that instructs them to EmitDropStats.
  // Since writes run with interrupts disabled, the mere fact that a core is able to process this
  // IPI means that it is not performing any other concurrent writes. Additionally, mp_sync_exec
  // issues a memory barrier that ensures that every other core will see that writes are disabled
  // after processing the IPI.
  auto emit_drop_stats = [](void* arg) {
    const cpu_num_t curr_cpu = arch_curr_cpu_num();
    percpu_writer::Buffer* percpu_buffers = static_cast<percpu_writer::Buffer*>(arg);
    percpu_writer::Buffer& curr_cpu_buffer = percpu_buffers[curr_cpu];

    // We do not require that this call succeeds. If the trace buffer still doesn't have enough
    // space to contain the dropped record statistics, this will fail, but there's not much we can
    // do about that.
    curr_cpu_buffer.EmitDropStats();
  };
  mp_sync_exec(mp_ipi_target::ALL, 0, emit_drop_stats, percpu_buffers_.get());
  return ZX_OK;
}

zx_status_t KTrace::Rewind() {
  Guard<Mutex> guard{&lock_};

  // Calling Rewind on an uninitialized KTrace buffer is a no-op.
  if (!percpu_buffers_) {
    return ZX_OK;
  }
  // Rewind calls Drain on each per-CPU buffer. As mentioned in the doc comments of that method,
  // it is invalid to call Drain concurrently with a Read, and the method is only guaranteed to
  // fully empty the buffer if there are no concurrent Write operations. We ensure that these
  // prerequisites are met by:
  // 1. Holding the lock_, ensuring that there can be no other readers.
  // 2. Ensuring writes are disabled to prevent any future writes from starting.
  // 3. Performing the Drain within an IPI on each core, ensuring that this operation does not
  //    race with any in-progress writes.
  // Rewind also resets the dropped record statistics on every buffer to prepare for the next
  // tracing session.
  DisableWrites();

  auto run_drain = [](void* arg) {
    const cpu_num_t curr_cpu = arch_curr_cpu_num();
    percpu_writer::Buffer* percpu_buffers = static_cast<percpu_writer::Buffer*>(arg);
    percpu_writer::Buffer& curr_cpu_buffer = percpu_buffers[curr_cpu];
    curr_cpu_buffer.Drain();
    curr_cpu_buffer.ResetDropStats();
  };
  mp_sync_exec(mp_ipi_target::ALL, 0, run_drain, percpu_buffers_.get());
  return ZX_OK;
}

zx::result<size_t> KTrace::ReadUser(user_out_ptr<void> ptr, uint32_t offset, size_t len) {
  // As we may need to perform fault resolution later, ensure our caller is not holding any locks.
  lockdep::AssertNoLocksHeld();

  // Reads must be serialized with respect to all other non-write operations.
  Guard<Mutex> guard{&lock_};

  // If the passed in ptr is nullptr, then return the buffer size needed to read all of the
  // per-CPU buffers' contents.
  if (!ptr) {
    return zx::ok(buffer_size_ * num_buffers_);
  }

  // If the per-CPU buffers have not been initialized, there's nothing to do, so return early.
  if (!percpu_buffers_) {
    return zx::ok(0);
  }

  // Eventually, this should support users passing in buffers smaller than the sum of the size of
  // all per-CPU buffers, but for now we do not allow this.
  if (len < (buffer_size_ * num_buffers_)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Iterate through each per-CPU buffer and read its contents.
  size_t bytes_read = 0;
  user_out_ptr<ktl::byte> byte_ptr = ptr.reinterpret<ktl::byte>();

  auto copy_fn = [&](uint32_t byte_offset, ktl::span<ktl::byte> src) {
    // This is safe to do while holding the lock_ because the KTrace lock is a leaf lock that is
    // not acquired during the course of a page fault.
    zx_status_t status = ZX_ERR_BAD_STATE;
    guard.CallUntracked([&]() {
      // Compute the destination address for this segment.
      user_out_ptr out_ptr = byte_ptr.byte_offset(bytes_read + byte_offset);

      // Prepare the destination range of this segment to improve efficiency by
      // coalescing potential page faults into a bulk operation.
      // TODO(eieio): This could be improved further by restructuring the copy
      // out operation so that the full destination range can be determined and
      // soft-faulted in a single operation.
      Thread::Current::SoftFaultInRange(reinterpret_cast<vaddr_t>(out_ptr.get()),
                                        VMM_PF_FLAG_USER | VMM_PF_FLAG_WRITE, src.size());

      // Copy the trace data to the user segment.
      status = out_ptr.copy_array_to_user(src.data(), src.size());
    });

    return status;
  };

  for (uint32_t i = 0; i < num_buffers_; i++) {
    const zx::result<size_t> result = percpu_buffers_[i].Read(copy_fn, static_cast<uint32_t>(len));
    if (result.is_error()) {
      DiagsPrintf(INFO, "failed to copy out ktrace data: %d\n", result.status_value());
      // If we copied some data from a previous buffer, we have to return the fact that we did so
      // here. Otherwise, that data will be lost.
      if (bytes_read != 0) {
        return zx::ok(bytes_read);
      }
      // Otherwise, return the error.
      return zx::error(result.status_value());
    }
    bytes_read += result.value();
  }
  return zx::ok(bytes_read);
}

void KTrace::InitHook(unsigned) {
  const uint32_t bufsize = BootOptions::Get()->ktrace_bufsize << 20;
  const uint32_t initial_grpmask = BootOptions::Get()->ktrace_grpmask;

  dprintf(INFO, "ktrace_init: bufsize=%u grpmask=%x\n", bufsize, initial_grpmask);

  if (!bufsize) {
    dprintf(INFO, "ktrace: disabled\n");
    return;
  }

  // Coerce the category ids to match the pre-defined bit mappings of aged ktrace interface.
  // TODO(eieio): Remove this when kernel migrates to IOB-based tracing with extensible categories.
  SetupCategoryBits();

  dprintf(INFO, "Trace categories: \n");
  for (const fxt::InternedCategory& category : fxt::InternedCategory::Iterate()) {
    dprintf(INFO, "  %-20s : 0x%03x\n", category.string(), (1u << category.index()));
  }

  if (!initial_grpmask) {
    dprintf(INFO, "ktrace: delaying buffer allocation\n");
  }

  // Initialize the singleton data structures.
  GetInstance().Init(bufsize, initial_grpmask);
}

// Finish initialization before starting userspace (i.e. before debug syscalls can occur).
LK_INIT_HOOK(ktrace, KTrace::InitHook, LK_INIT_LEVEL_USER - 1)
