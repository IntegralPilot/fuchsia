// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_X86_INCLUDE_ARCH_HYPERVISOR_H_
#define ZIRCON_KERNEL_ARCH_X86_INCLUDE_ARCH_HYPERVISOR_H_

#include <lib/id_allocator.h>
#include <zircon/compiler.h>
#include <zircon/syscalls/hypervisor.h>
#include <zircon/types.h>

#include <arch/x86/apic.h>
#include <arch/x86/hypervisor/vmx_state.h>
#include <arch/x86/interrupts.h>
#include <fbl/ref_ptr.h>
#include <hypervisor/aspace.h>
#include <hypervisor/interrupt_tracker.h>
#include <hypervisor/page.h>
#include <hypervisor/trap_map.h>
#include <kernel/event.h>
#include <kernel/spinlock.h>
#include <kernel/timer.h>
#include <ktl/unique_ptr.h>

class AutoVmcs;
struct VmxInfo;

class VmxPage : public hypervisor::Page {
 public:
  zx::result<> Alloc(const VmxInfo& info, uint8_t fill);

 private:
  using hypervisor::Page::Alloc;
};

// Represents a guest within the hypervisor.
class Guest {
 public:
  // Maximum VCPUs per guest.
  static constexpr size_t kMaxGuestVcpus = 64;

  static zx::result<ktl::unique_ptr<Guest>> Create();

  Guest(const Guest&) = delete;
  Guest(Guest&&) = delete;
  Guest& operator=(const Guest&) = delete;
  Guest& operator=(Guest&&) = delete;

  virtual ~Guest();

  zx::result<> SetTrap(uint32_t kind, zx_vaddr_t addr, size_t len, fbl::RefPtr<PortDispatcher> port,
                       uint64_t key);

  hypervisor::GuestPhysicalAspace& PhysicalAspace() { return gpa_; }
  fbl::RefPtr<VmAddressRegion> RootVmar() const { return gpa_.RootVmar(); }
  hypervisor::TrapMap& Traps() { return traps_; }
  zx_paddr_t MsrBitmapsAddress() const { return msr_bitmaps_page_.PhysicalAddress(); }

  zx::result<uint16_t> TryAllocVpid() { return vpid_allocator_.TryAlloc(); }
  zx::result<> FreeVpid(uint16_t vpid) { return vpid_allocator_.Free(vpid); }

 private:
  explicit Guest(hypervisor::GuestPhysicalAspace gpa);

  hypervisor::GuestPhysicalAspace gpa_;
  hypervisor::TrapMap traps_;
  id_allocator::IdAllocator<uint16_t, kMaxGuestVcpus> vpid_allocator_;
  VmxPage msr_bitmaps_page_;
};

// Stores part of the MSR state for a virtual CPU.
struct MsrState {
  uint64_t star;
  uint64_t lstar;
  uint64_t fmask;
  uint64_t tsc_aux;
  uint64_t kernel_gs_base;
};

// Stores the local APIC state for a virtual CPU.
struct LocalApicState {
  // Timer for APIC timer.
  Timer timer;
  // Tracks pending interrupts.
  hypervisor::InterruptTracker<X86_INT_COUNT> interrupt_tracker;
  // LVT timer configuration
  uint32_t lvt_timer = LVT_MASKED;  // Initial state is masked (Vol 3 Section 10.12.5.1).
  uint32_t lvt_initial_count;
  uint32_t lvt_divide_config;
};

// Stores the para-virtualized clock for a virtual CPU.
//
// System time is time since boot time, and boot time is some fixed point in the
// past.
struct pv_clock_system_time;
struct PvClockState {
  bool is_stable = false;
  uint32_t version = 0;
  pv_clock_system_time* system_time = nullptr;
  hypervisor::GuestPtr guest_ptr;
};

// Represents a virtual CPU within a guest.
class Vcpu {
 public:
  static zx::result<ktl::unique_ptr<Vcpu>> Create(Guest& guest, zx_vaddr_t entry);

  Vcpu(const Vcpu&) = delete;
  Vcpu(Vcpu&&) = delete;
  Vcpu& operator=(const Vcpu&) = delete;
  Vcpu& operator=(Vcpu&&) = delete;

  virtual ~Vcpu();

  zx::result<> Enter(zx_port_packet_t& packet);
  void Kick();
  zx::result<> Interrupt(uint32_t vector);
  zx::result<> ReadState(zx_vcpu_state_t& vcpu_state);
  zx::result<> WriteState(const zx_vcpu_state_t& vcpu_state);
  zx::result<> WriteState(const zx_vcpu_io_t& io_state);

  zx_info_vcpu_t GetInfo() const;

 private:
  Vcpu(Guest& guest, uint16_t vpid, Thread* thread);

  void Migrate(Thread* thread, Thread::MigrateStage stage)
      TA_REQ(chainlock_transaction_token, thread->get_lock());
  void ContextSwitch(bool include_gs);
  void LoadExtendedRegisters(AutoVmcs& vmcs);
  void SaveExtendedRegisters(AutoVmcs& vmcs);

  // Some explanation is required for why we disable static lock analysis here.
  // `thread_` is initially set during our construction in a context where the
  // caller guarantees that `thread_`'s instance cannot exit.  After this, there
  // will only ever be one more assignment made to this variable, when the thread
  // exits and calls the registered migration function.  This happens with the
  // global Thread list_lock held, and is what is used to make sure that the
  // thread cannot exit out from under us during operations like Kick and
  // Interrupt.
  //
  // There are many other places in the code, however, where the code simply
  // wants to check to see the calling thread is the same as `thread_`.  There
  // are only two options here.
  //
  // 1) The calling thread (A) is not the same as thread as our thread.  If our
  //    initial thread was B, then thread_ can only ever have two values; either B
  //    or nullptr.  A will never match, regardless of whether or not B has
  //    exited or not yet.  We need the `thread_` member to be atomic in order
  //    to avoid a formal C++ data race, but nothing more.
  // 2) The calling thread (A) *is* the same as our thread.  This means that our
  //    thread is currently running, and cannot exit out from under us.  We
  //    don't even need the atomic load in this case, since it is A who will
  //    eventually perform the mutation setting thread_ to nullptr, so no
  //    concurrent write is possible.
  //
  // In either case, we don't actually need to hold the Thread::list_lock_ in
  // order to simply check to see if a calling thread is our thread.  All we
  // need is a relaxed atomic load to deal with case 1 (above).
  //
  bool ThreadIsOurThread(Thread* thread) const TA_NO_THREAD_SAFETY_ANALYSIS {
    // TODO(https://fxbug.dev/355287217): Remove workaround for const/volatile qualified atomic_ref.
    return thread ==
           ktl::atomic_ref{const_cast<Vcpu*>(this)->thread_}.load(ktl::memory_order_relaxed);
  }

  void InterruptCpu();

  Guest& guest_;
  const uint16_t vpid_;
  // |last_cpu_| contains the CPU dedicated to holding the guest's VMCS state,
  // or INVALID_CPU if there is no such VCPU. If this Vcpu is actively running,
  // then |last_cpu_| will point to that CPU.
  //
  // The VMCS state of this Vcpu must not be loaded prior to |last_cpu_| being
  // set, nor must |last_cpu_| be modified prior to the VMCS state being cleared.
  cpu_num_t last_cpu_ TA_GUARDED(thread_->get_lock());

  // |thread_| will be set to nullptr when the thread exits during the Exiting
  // stage of our migration function callback.
  Thread* thread_ TA_GUARDED(Thread::get_list_lock());
  ktl::atomic<bool> kicked_ = false;
  ktl::atomic<bool> entered_ = false;
  VmxPage vmcs_page_;
  VmxState vmx_state_;
  MsrState msr_state_;
  LocalApicState local_apic_state_;
  PvClockState pv_clock_state_;
  // The guest may enable any state, so the XSAVE area is the maximum size.
  alignas(64) uint8_t extended_register_state_[X86_MAX_EXTENDED_REGISTER_SIZE];
};

#endif  // ZIRCON_KERNEL_ARCH_X86_INCLUDE_ARCH_HYPERVISOR_H_
