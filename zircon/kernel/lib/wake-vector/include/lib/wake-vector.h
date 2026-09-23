// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#ifndef ZIRCON_KERNEL_LIB_WAKE_VECTOR_INCLUDE_LIB_WAKE_VECTOR_H_
#define ZIRCON_KERNEL_LIB_WAKE_VECTOR_INCLUDE_LIB_WAKE_VECTOR_H_

#include <lib/relaxed_atomic.h>
#include <lib/user_copy/user_ptr.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <zircon/syscalls/system.h>
#include <zircon/types.h>

#include <fbl/intrusive_double_list.h>
#include <kernel/auto_preempt_disabler.h>
#include <kernel/mutex.h>
#include <ktl/array.h>
#include <ktl/forward.h>
#include <ktl/type_traits.h>

namespace wake_vector {

namespace internal {
struct GlobalListTag {};
struct PendingListTag {};
}  // namespace internal

// Forward declaration.
class WakeEvent;

// WakeVector is an interface implemented by objects that will generate system wake events using the
// WakeEvent type. This interface provides diagnostic information about the wake vector to the
// suspend subsystem.
class WakeVector {
 public:
  // This constructor verifies that the derived class has a WakeEvent member at compile time to help
  // avoid misuse. A derived class must pass a pointer-to-member to its WakeEvent member. This
  // constructor does not touch the contents of the WakeEvent instance, which most likely is
  // uninitialized at this point.
  //
  // Example:
  //
  // MyWakeVector::MyWakeVector() : WakeVector{&MyWakeVector::wake_event_}, wake_event_{*this} {}
  //
  template <typename Class>
  explicit WakeVector(WakeEvent Class::* wake_event_member) {
    static_assert(ktl::is_base_of_v<WakeVector, Class>);
  }
  virtual ~WakeVector() = default;

  // Diagnostic information about the wake vector managed by the implementor of this interface.
  struct Diagnostics {
    // Indicates that the given wake vector is enabled and can generate wake events. Disabled wake
    // vectors are not listed in diagnostic logs.
    bool enabled = false;

    // The koid of the object implementing this interface, if any.
    zx_koid_t koid = ZX_KOID_INVALID;

    // Extra information specific to the wake vector that can aid in determining the source of the
    // wake event and potentially its state.
    ktl::array<char, ZX_MAX_NAME_LEN> extra{};

    // Utility to write into the extra field printf style.
    int PrintExtra(const char* format, ...) __PRINTFLIKE(2, 3) {
      va_list ap;
      va_start(ap, format);
      const int err = vsnprintf(extra.data(), extra.size(), format, ap);
      va_end(ap);
      return err;
    }
  };

  // Provides diagnostic information about the wake vector object implementing this interface.
  virtual void GetDiagnostics(Diagnostics& diagnostics_out) const = 0;
};

// The result of a request to wake up the system.
enum class WakeResult {
  // The system was not suspended at the time of the event.
  Active,

  // The system was suspended at the time of the wake event.
  Resumed,

  // The system was in the process of suspending at the time of the wake event.
  SuspendAborted,

  // An already pending wake event was triggered again.
  BadState,
};

// WakeEvent manages the lifecycle of wake events triggered by wake vectors.
//
// A system wake event may be triggered in response to an appropriately configured interrupt,
// exception, timer, or other future wake source that should resume the system from a suspended
// state. When a wake event is triggered, it enters the pending state and will prevent the system
// from entering suspend until it has been acknowledged. A pending wake event is automatically
// acknowledged when the WakeEvent instance is destroyed to prevent missing an acknowledgement that
// would render the system unable to suspend.
//
// WakeEvent maintains a global list of all instances for diagnostic purposes (i.e. logging wake
// events that pending before, during, and after suspend). WakeEvents are added to and removed from
// the global list using WakeEvent::Initialize and WakeEvent::Destroy, respectively. Because
// diagnostics access each WakeEvent, and its containing WakeVector, from the global list, care must
// be taken to avoid potential use-after-free hazards.
//
// Users of WakeEvents MUST adhere to the following rules:
// 1. A WakeEvent object MUST be instantiated as a member of a type that implements the WakeVector
//    interface, such that the lifetime of the containing type encoloses the lifetime of the
//    WakeEvent. DO NOT heap allocate WakeEvent instances separately from the referenced WakeVector.
// 2. The container of a WakeEvent instance SHOULD call WakeEvent::Initialize during construction /
//    initialization to register the WakeEvent on the global list. The container MAY skip the call
//    to WakeEvent::Initialize if initialization of the containing type fails.
// 3. The container of a WakeEvent MUST call WakeEvent::Destroy IFF WakeEvent::Initialize has been
//    called previously on the same instance of WakeEvent AND WakeEvent::Destroy MUST be called
//    BEFORE the containing type itself is destructed.
//
// WakeEvent::Destroy MAY be called in the destructor of the containing type, which will ensure that
// the WakeEvent is removed from the global list before any state in the containing type that
// diagnostics may access becomes invalid. However, extra care must be taken if a WakeEvent is a
// member of a base class and there are subclasses that override WakeVector::GetDiagnostics -- in
// these cases the subclass is responsible for calling WakeEvent::Destroy before its destructor
// completes INSTEAD of the base class to prevent use-after-free during races between diagnostics
// and the destruction of subclass state that diagnostics might access.
//
// AS A GENERAL RULE, it is safe to call WakeEvent::Initialize in a constructor and
// WakeEvent::Destroy in a destructor IF the destructor OR the implementation/override of
// WakeVector::GetDiagnostics can be marked final in the class making the calls.
//
// Calls to WakeEvent::Initialize and WakeEvent::Destroy must always be balanced. However, a
// WakeEvent MAY be initialized and destroyed more than once, as long as it is destroyed before its
// destructor is invoked.
//
class WakeEvent : public fbl::ContainableBaseClasses<
                      fbl::TaggedDoublyLinkedListable<WakeEvent*, internal::GlobalListTag>,
                      fbl::TaggedDoublyLinkedListable<WakeEvent*, internal::PendingListTag>> {
 public:
  enum class AckBehavior { ClearSignaled, RemainSignaled };

  static bool has_pending_wake_events() TA_EXCL(PendingListLock::Get()) {
    Guard<SpinLock, IrqSave> guard{PendingListLock::Get()};
    return !pending_list_.is_empty();
  }

  // Construct a WakeEvent referencing the given wake_vector.
  explicit WakeEvent(const WakeVector& wake_vector) : wake_vector_(wake_vector) {}

  ~WakeEvent() {
    // By the time that we destruct, we should have been Destroyed, meaning that
    // we are no longer in any lists.
    DEBUG_ASSERT(!in_global_list());
    DEBUG_ASSERT(!in_pending_list());
  }

  void Initialize() TA_EXCL(GlobalListLock::Get(), PendingListLock::Get());
  void Destroy() TA_EXCL(GlobalListLock::Get(), PendingListLock::Get());

  // Triggers a wakeup that resumes the system, or aborts an incomplete suspend sequence, and
  // prevents the system from starting a new suspend sequence.
  //
  // Must be called with interrupts and preempt disabled.
  //
  // Returns:
  //  - WakeResult::Active if this wake trigger occurred when the system was active.
  //  - WakeResult::Resumed if this or another wake trigger resumed the system.
  //  - WakeResult::SuspendAborted if this wake trigger occurred before suspend completed.
  //  - WakeResult::BadState if this wake event is already pending.
  //
  // Calls to |Trigger| and |Acknowledge| must be synchronized by the caller to guarantee that
  // updates are performed by a single actor at a time.
  //
  WakeResult Trigger(zx_instant_boot_t trigger_time) TA_EXCL(PendingListLock::Get()) {
    AnnotatedAutoPreemptDisabler preempt_disabler;
    Guard<SpinLock, IrqSave> pending_guard{PendingListLock::Get()};
    return TriggerLocked(trigger_time);
  }

  // Acknowledges a pending wake event, allowing the system to enter suspend when all other
  // suspend conditions are met.
  //
  // Calls to |Trigger| and |Acknowledge| must be synchronized by the caller to guarantee that
  // updates are performed by a single actor at a time.
  //
  void Acknowledge(AckBehavior ack_behavior) TA_EXCL(PendingListLock::Get()) {
    AnnotatedAutoPreemptDisabler preempt_disabler;
    Guard<SpinLock, IrqSave> pending_guard{PendingListLock::Get()};
    AcknowledgeLocked(current_boot_time(), ack_behavior);
  }

  // WARNING : This is not the method you are looking for <jedimindtrick/>
  //
  // Strobe is an operation used only in a very specific situation; when a suspend operation times
  // out and the ResumeTimerWakeVector becomes signaled as a result.  This object is (currently) the
  // only non-interrupt wake source/vector defined in the system, and it is not directly exposed to
  // user-mode as an object which becomes acknowledged by user-mode actions.  Instead, it is the
  // synthetic wake source used to report suspend-operation timeouts, and is (logically speaking)
  // _always_ immediately acked after being signaled.
  //
  // Strobe handles this operation, without needing to expose any locks to make it possible to
  // atomically Trigger/Acknowledge the object.  For all other wake source objects in the system,
  // explicit calls to Trigger and Acknowledge are what should be used.
  WakeResult Strobe(zx_instant_boot_t trigger_time = current_boot_time())
      TA_EXCL(PendingListLock::Get()) {
    AnnotatedAutoPreemptDisabler preempt_disabler;
    Guard<SpinLock, IrqSave> pending_guard{PendingListLock::Get()};
    const WakeResult result = TriggerLocked(trigger_time);
    AcknowledgeLocked(trigger_time, AckBehavior::ClearSignaled);
    return result;
  }

  // Walk the global list of all instances and dump diagnostic information to |f|. All events that
  // are currently pending OR that were triggered after the optional time value are logged.
  //
  // Safe to call concurrently with any and all methods, including ctors and dtors.
  static void Dump(FILE* f, zx_instant_boot_t log_triggered_after_boot_time = ZX_TIME_INFINITE)
      TA_EXCL(GlobalListLock::Get(), PendingListLock::Get());

  static zx_status_t GenerateWakeEventReport(
      zx_instant_boot_t suspend_start_time, user_out_ptr<zx_wake_source_report_header_t> out_header,
      user_out_ptr<zx_wake_source_report_entry_t> out_entries, uint32_t num_entries,
      user_out_ptr<uint32_t> actual_entries) TA_EXCL(GlobalListLock::Get(), PendingListLock::Get());

  static void DiscardWakeEventReport() TA_EXCL(GlobalListLock::Get(), PendingListLock::Get());

 private:
  using GlobalListTag = internal::GlobalListTag;
  using PendingListTag = internal::PendingListTag;
  using GlobalList = fbl::DoublyLinkedList<WakeEvent*, GlobalListTag, fbl::SizeOrder::Constant>;
  using PendingList = fbl::DoublyLinkedList<WakeEvent*, PendingListTag, fbl::SizeOrder::Constant>;

  bool in_global_list() const TA_REQ(GlobalListLock::Get());
  bool in_pending_list() const TA_REQ(PendingListLock::Get());

  bool is_signaled() const TA_REQ(PendingListLock::Get()) {
    return (report_info_.flags & ZX_SYSTEM_WAKE_REPORT_ENTRY_FLAG_SIGNALED) != 0;
  }

  bool has_been_reported() const TA_REQ(PendingListLock::Get()) {
    return (report_info_.flags & ZX_SYSTEM_WAKE_REPORT_ENTRY_FLAG_PREVIOUSLY_REPORTED) != 0;
  }

  WakeResult TriggerLocked(zx_instant_boot_t trigger_time)
      TA_REQ(PendingListLock::Get(), preempt_disabled_token);
  void AcknowledgeLocked(zx_instant_boot_t trigger_time, AckBehavior ack_behavior)
      TA_REQ(PendingListLock::Get(), preempt_disabled_token);

  void AssignFlag(bool value, uint32_t flag) TA_REQ(PendingListLock::Get()) {
    if (value) {
      report_info_.flags |= flag;
    } else {
      report_info_.flags &= ~flag;
    }
  }

  void AssignSignaled(bool value) TA_REQ(PendingListLock::Get()) {
    AssignFlag(value, ZX_SYSTEM_WAKE_REPORT_ENTRY_FLAG_SIGNALED);
  }

  void AssignHasBeenReported(bool value) TA_REQ(PendingListLock::Get()) {
    AssignFlag(value, ZX_SYSTEM_WAKE_REPORT_ENTRY_FLAG_PREVIOUSLY_REPORTED);
  }

  // -- Important --
  //
  // Notes on the pending list, locks, and concurrency.  You definitely want to
  // read this if you are reading the wake source reporting generation code.  It
  // will provide an explanation about how this all works, why it is structured
  // the way it is, and why it is all safe.
  //
  // It is a requirement that every wake source in the system which has become
  // signaled since last being reported be present in any wake source report
  // generated for a caller of `zx_system_suspend_enter`.  They will continue to
  // be reported to users until they have been *both* acknowledged, and reported
  // at least once.
  //
  // The `pending_list_` holds the current list of wake events waiting to be
  // reported.  Members of the list should remain on the list provided that:
  //
  // 1) They have been signaled at some point in the past, at least once.
  // 2) They are either not-yet-acknowledged, or not-yet-reported, or both.
  //
  // The PendingListLock is a spinlock used to protect the integrity of the
  // `pending_list_`, however due to another requirement, it alone is not
  // sufficient.  Specifically, it is a requirement that generating a report can
  // never hold off interrupt processing for O(n) time.  This requirement would
  // be violated if we had to hold the PendingListLock for the duration of a
  // report-generation operation. To avoid violating this requirement, we drop
  // the PendingListLock each time through the loop while iterating through the
  // pending list during report generation.
  //
  // The need to drop the PendingListLock during iteration while generating a
  // report leads to two other potential bad behaviors which we need to protect
  // against:
  //
  // 1) During report generation, we are holding an iterator to an element in
  //    the list.  This iterator cannot become invalidated during the period
  //    where we don't hold the lock.
  // 2) We must never "double report" a wake source in a single report.  IOW -
  //    if KOID X shows up once in the report generated for the user, it must
  //    not show up any more times _in that specific report_.
  //
  // There are a total of 4 operations which can affect report generation.  They
  // are:
  //
  // 1) Triggering.  This will update the bookkeeping for a wake event, and add
  //    that event to the pending list if it was not already on the list.  This
  //    operation is the only operation which takes place at hard IRQ time, and
  //    takes O(1) time.
  // 2) Ack'ing.  This will update the bookkeeping for a wake event, but will
  //    never remove the event from the list, even if it is now both reported
  //    and acknowledged. This operation always takes place in the context of a
  //    syscall made by user mode and is O(1).
  // 3) Construction/Registration.  Construction of a new wake event does
  //    not directly affect the pending_list_, but it does affect the total
  //    count of wake sources in the system which is a number which is also
  //    included in the wake source report.  This is an O(1) operation.
  // 4) Destruction/De-registration.  Destruction of a wake event always
  //    unconditionally removes the event from the pending list, if it is on the
  //    list at the time of destruction.  This is an O(1)) operation.
  //
  // -- Avoiding iterator invalidation --
  //
  // Report generation holds the GlobalListLock (a mutex) for the duration of a
  // report generation operation.  Construction/Destruction (#3-4) operations
  // must also hold the GlobalListLock meaning that they cannot invalidate a
  // report operation's iterator as they cannot run concurrently with the report
  // generation.  Trigger operations (#1) only add items, and therefore cannot
  // invalidate an intrusive list iterator.
  //
  // Ack operations (#2) could theoretically cause trouble if they were to
  // remove an element from the list as soon as it became both acked and
  // reported.  While it would not result in UAF, it could cause the report to
  // stop iteration early if the next element to report was acked and
  // immediately removed from the list.  To avoid this, ack operations will
  // never remove an element from the list.  Instead, they merely mark the
  // element as ack'ed and depend on report generation to handle the removal for
  // them, avoiding the invalidation issue in the process.
  //
  // Adopting this approach of having the report generation operation remove the
  // ack'ed wake source, instead of using the GlobalListLock to synchronize,
  // does two things for us.
  //
  // 1) It means that user mode ack operations will never need to obtain the
  //    GlobalListLock, and potentially block behind an O(n) report generation
  //    operation.
  // 2) It means that it is possible for kernel code it ack kernel-owned
  //    interrupts which also happen to be wake sources at hard IRQ time.
  //
  // -- Avoiding double reports --
  //
  // Construction/Destruction (#3-4) operations have no potential to produce a
  // double report in the first place, but they also cannot run concurrently
  // with report generation, so they have no potential to produce a double
  // report.  Likewise, ack'ing (#3) cannot produce a double report as it will
  // never remove an element from the list, only update the bookkeeping.  Even
  // if it did actually remove elements from the list, it couldn't produce a
  // double report.
  //
  // This means that only triggering (op #2) has the potential to produce a
  // double report.  A sequence which would produce this behavior would go like
  // this.
  //
  // 1) While a report is being generated, event X is encountered.  X has
  //    already been acknowledged, and now has certainly been reported, so X is
  //    removed from the pending list as the iterator is advanced to the next
  //    event, Y. The reporting thread then it drops the lock and starts to copy
  //    information into the user's buffer.
  // 2) X is now triggered again.  The IRQ handler grabs the pending lock, marks
  //    X as triggered, and adds it back to the end of the list, then drops the
  //    lock again.
  // 3) The reporting thread locks the list again, and processes Y.  It will
  //    advance down the list until it encounters X again, eventually adding
  //    it to the report a second time.
  //
  // Avoiding this situation is easy if we follow one simple rule. When an event
  // becomes triggered, it should be added to the *front* of the list instead of
  // the back.  This ensures that once report generation has started and the
  // initial iterator has been computed, no newly triggered events can show up
  // in this report.  They will have to wait to ride the next report-train.
  //
  DECLARE_SINGLETON_MUTEX(GlobalListLock);
  DECLARE_SINGLETON_SPINLOCK(PendingListLock);
  static GlobalList global_list_ TA_GUARDED(GlobalListLock::Get());
  static PendingList pending_list_ TA_GUARDED(PendingListLock::Get());

  // Our parent wake_vector reference.  This is only safe to access when the object has been
  // instantiated and init'ed, but not yet destroyed.  IOW - only when `active_` is true.
  const WakeVector& wake_vector_;
  TA_GUARDED(PendingListLock::Get()) zx_wake_source_report_entry_t report_info_ { 0 };
};

inline bool WakeEvent::in_global_list() const { return fbl::InContainer<GlobalListTag>(*this); }
inline bool WakeEvent::in_pending_list() const { return fbl::InContainer<PendingListTag>(*this); }

}  // namespace wake_vector

#endif  // ZIRCON_KERNEL_LIB_WAKE_VECTOR_INCLUDE_LIB_WAKE_VECTOR_H_
