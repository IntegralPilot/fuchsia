// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_INTERRUPT_DISPATCHER_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_INTERRUPT_DISPATCHER_H_

#include <lib/wake-vector.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <sys/types.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <kernel/event.h>
#include <kernel/spinlock.h>
#include <object/dispatcher.h>
#include <object/port_dispatcher.h>

enum class InterruptState {
  WAITING = 0,
  DESTROYED = 1,
  TRIGGERED = 2,
  NEEDACK = 3,
  IDLE = 4,
};

// Note that unlike most Dispatcher subclasses, this one is further
// subclassed, and so cannot be final.
class InterruptDispatcher : public SoloDispatcher<InterruptDispatcher, ZX_DEFAULT_INTERRUPT_RIGHTS>,
                            public wake_vector::WakeVector {
 public:
  InterruptDispatcher& operator=(const InterruptDispatcher&) = delete;
  zx_obj_type_t get_type() const final { return ZX_OBJ_TYPE_INTERRUPT; }

  bool is_wake_vector() const { return flags_ & INTERRUPT_WAKE_VECTOR; }

  virtual zx_status_t WaitForInterrupt(zx_time_t* out_timestamp);
  virtual zx_status_t Trigger(zx_time_t timestamp);
  virtual zx_status_t Ack();
  virtual zx_status_t Destroy();

  void InterruptHandler();
  zx_status_t Bind(fbl::RefPtr<PortDispatcher> port_dispatcher, uint64_t key);
  zx_status_t Unbind(fbl::RefPtr<PortDispatcher> port_dispatcher);

  void on_zero_handles() final;

  // Default wake vector diagnostics for interrupt dispatchers.
  void GetDiagnostics(WakeVector::Diagnostics& diagnostics_out) const override {
    diagnostics_out.enabled = false;
    diagnostics_out.koid = get_koid();
  }

  // Returns information about this interrupt in a zx_object_get_info call.
  zx_info_interrupt_t GetInfo() const;

 protected:
  virtual void MaskInterrupt() = 0;
  virtual void UnmaskInterrupt() = 0;
  virtual void DeactivateInterrupt() = 0;
  virtual void UnregisterInterruptHandler() = 0;

  enum Flags : uint32_t {
    // The interrupt is virtual.
    INTERRUPT_VIRTUAL = (1u << 0),
    // The interrupt should be unmasked before waiting on the event.
    //
    // Mutually exclusive with INTERRUPT_UNMASK_PREWAIT_UNLOCKED.
    INTERRUPT_UNMASK_PREWAIT = (1u << 1),
    // The same as |INTERRUPT_UNMASK_PREWAIT| except release the dispatcher
    // spinlock before waiting.
    //
    // Mutually exclusive with INTERRUPT_UNMASK_PREWAIT.
    INTERRUPT_UNMASK_PREWAIT_UNLOCKED = (1u << 2),
    // The interrupt should be masked following waiting.
    INTERRUPT_MASK_POSTWAIT = (1u << 4),
    // The interrupt may wake the system from suspend.
    INTERRUPT_WAKE_VECTOR = (1u << 5),
    // Allow kernel tests to call Ack() without binding to a port.
    INTERRUPT_ALLOW_ACK_WITHOUT_PORT_FOR_TEST = (1u << 6),
    // The interrupt should use monotonic timestamps instead of the default,
    // which is boot timestamps.
    INTERRUPT_TIMESTAMP_MONO = (1u << 7),
  };

  enum PostAckState {
    FullyAcked,   // The interrupt was acked, and there was no other interrupt pending afterwards.
    Retriggered,  // The interrupt was acked, but immediately re-triggered due to a pending irq.
  };

  // It is an error to specify both INTERRUPT_UNMASK_PREWAIT and INTERRUPT_UNMASK_PREWAIT_UNLOCKED.
  explicit InterruptDispatcher(Flags flags) : InterruptDispatcher(flags, 0) {}
  explicit InterruptDispatcher(Flags flags, uint32_t options);
  void Signal() { event_.Signal(); }
  bool SendPacketLocked(zx_time_t timestamp) TA_REQ(spinlock_);
  zx::result<InterruptDispatcher::PostAckState> AckInternal() TA_EXCL(spinlock_);

  // Allow subclasses to add/remove the wake event instance from the global diagnostics list at the
  // appropriate times during initialization and teardown. These methods do not need to be called
  // unless the subclass needs to be able to wake the system from suspend. See lib/wake-vector.h for
  // the specific requirements regarding when to initialize and destroy the wake event.
  void InitializeWakeEvent() TA_REQ(spinlock_) { wake_event_.Initialize(); }
  void DestroyWakeEvent() TA_REQ(spinlock_) { wake_event_.Destroy(); }

  // The zx_interrupt_wait operation is broken down into two phases.  First, we
  // BeginWaitForInterrupt, which will handle internal bookkeeping and either return a status code
  // if the wait operation is finished (success or failure), or it will return nothing if we need to
  // block and try again later.
  //
  // Afterwards, if we need to block, we will call DoWaitForInterruptBlock and try again if the
  // block operation succeeds.
  ktl::optional<zx_status_t> BeginWaitForInterrupt(zx_time_t* out_timestamp) TA_EXCL(spinlock_);
  zx_status_t DoWaitForInterruptBlock() TA_EXCL(spinlock_);

 private:
  AutounsignalEvent event_;

  zx_time_t timestamp_ TA_GUARDED(spinlock_);
  const Flags flags_;
  const uint32_t options_;
  // Current state of the interrupt object
  InterruptState state_ TA_GUARDED(spinlock_);
  PortInterruptPacket port_packet_ TA_GUARDED(spinlock_) = {};
  fbl::RefPtr<PortDispatcher> port_dispatcher_ TA_GUARDED(spinlock_);
  wake_vector::WakeEvent wake_event_ TA_GUARDED(spinlock_);

  // Controls the access to Interrupt properties
  DECLARE_SPINLOCK(InterruptDispatcher) spinlock_;
};

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_INTERRUPT_DISPATCHER_H_
