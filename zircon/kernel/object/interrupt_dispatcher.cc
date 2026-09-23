// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/interrupt_dispatcher.h"

#include <lib/affine/ratio.h>
#include <platform.h>
#include <zircon/syscalls/object.h>
#include <zircon/syscalls/port.h>

#include <dev/interrupt.h>
#include <kernel/auto_preempt_disabler.h>
#include <kernel/idle_power_thread.h>
#include <object/port_dispatcher.h>
#include <object/process_dispatcher.h>

InterruptDispatcher::InterruptDispatcher(Flags flags, uint32_t options)
    : WakeVector(&InterruptDispatcher::wake_event_),
      timestamp_(0),
      flags_(flags),
      options_(options),
      state_(InterruptState::IDLE),
      wake_event_(*this) {
  DEBUG_ASSERT((flags & INTERRUPT_UNMASK_PREWAIT) == 0 ||
               (flags & INTERRUPT_UNMASK_PREWAIT_UNLOCKED) == 0);
}

zx_info_interrupt_t InterruptDispatcher::GetInfo() const { return {.options = options_}; }

zx_status_t InterruptDispatcher::WaitForInterrupt(zx_time_t* out_timestamp) {
  while (true) {
    const ktl::optional<zx_status_t> opt_status = BeginWaitForInterrupt(out_timestamp);
    if (opt_status.has_value()) {
      return opt_status.value();
    }

    const zx_status_t block_status = DoWaitForInterruptBlock();
    if (block_status != ZX_OK) {
      return block_status;
    }
  }
}

ktl::optional<zx_status_t> InterruptDispatcher::BeginWaitForInterrupt(zx_time_t* out_timestamp) {
  bool defer_unmask = false;

  {
    // Disable preemption to ensure we don't trigger a reschedule while holding this spinlock.
    AutoPreemptDisabler preempt_disable;
    Guard<SpinLock, IrqSave> guard{&spinlock_};
    if (port_dispatcher_) {
      return ZX_ERR_BAD_STATE;
    }
    switch (state_) {
      case InterruptState::DESTROYED:
        return ZX_ERR_CANCELED;

      case InterruptState::TRIGGERED:
        state_ = InterruptState::NEEDACK;
        *out_timestamp = timestamp_;
        timestamp_ = 0;
        return event_.Unsignal();

      case InterruptState::NEEDACK:
        // We are in the NEEDACK state and have been waiting for a thread to
        // block on this object to serve as our Ack signal.  _If_ we are an
        // edge triggered interrupt, it is possible that we were signaled (by
        // hardware) once again after unblocking the time before.  If that has
        // happened, we will have a non-zero value stored in timestamp_, meaning
        // that there is another IRQ pending.
        //
        // So, if there is a pending IRQ, consume the timestamp, clear the
        // signal, and do not block our thread (as if we were in the TRIGGERED
        // state).  Otherwise, unmask our interrupt at the proper point in the
        // sequence, change to the waiting state, and block our thread.
        if (timestamp_) {
          // if we are a wake vector and our consuming a pending interrupt, then
          // make sure that our wake event remains signaled as we record the
          // acknowledgement event.
          if (is_wake_vector()) {
            wake_event_.Acknowledge(wake_vector::WakeEvent::AckBehavior::RemainSignaled);
          }

          *out_timestamp = timestamp_;
          timestamp_ = 0;
          return event_.Unsignal();
        } else {
          // There is no pending interrupt.  If we are a wake vector, ack our wake event and clear
          // its signaled state.
          if (is_wake_vector()) {
            wake_event_.Acknowledge(wake_vector::WakeEvent::AckBehavior::ClearSignaled);
          }
        }

        if (flags_ & INTERRUPT_UNMASK_PREWAIT) {
          UnmaskInterrupt();
        } else if (flags_ & INTERRUPT_UNMASK_PREWAIT_UNLOCKED) {
          defer_unmask = true;
        }
        break;

      case InterruptState::IDLE:
        break;

      default:
        return ZX_ERR_BAD_STATE;
    }
    state_ = InterruptState::WAITING;
  }

  if (defer_unmask) {
    UnmaskInterrupt();
  }

  return ktl::nullopt;
}

zx_status_t InterruptDispatcher::DoWaitForInterruptBlock() {
  ThreadDispatcher::AutoBlocked by(ThreadDispatcher::Blocked::INTERRUPT);
  zx_status_t status = event_.Wait(Deadline::infinite());
  if (status != ZX_OK) {
    // The Event::Wait call was interrupted and we need to retry
    // but before we retry we will set the interrupt state
    // back to IDLE if we are still in the WAITING state.
    Guard<SpinLock, IrqSave> guard{&spinlock_};
    if (state_ == InterruptState::WAITING) {
      state_ = InterruptState::IDLE;
    }
  }
  return status;
}

bool InterruptDispatcher::SendPacketLocked(zx_time_t timestamp) {
  bool status = port_dispatcher_->QueueInterruptPacket(&port_packet_, timestamp);
  if (flags_ & INTERRUPT_MASK_POSTWAIT) {
    MaskInterrupt();
  }
  timestamp_ = 0;
  return status;
}

zx_status_t InterruptDispatcher::Trigger(zx_time_t timestamp) {
  if (!(flags_ & INTERRUPT_VIRTUAL)) {
    return ZX_ERR_BAD_STATE;
  }

  // Use preempt disable for correctness to prevent rescheduling when waking a
  // thread while holding the spinlock.
  AutoPreemptDisabler preempt_disable;
  Guard<SpinLock, IrqSave> guard{&spinlock_};

  // Nothing to do if this interrupt has been destroyed and is waiting to be
  // cleaned up.
  if (state_ == InterruptState::DESTROYED) {
    return ZX_ERR_CANCELED;
  }

  // only record timestamp if this is the first signal since we started waiting
  if (!timestamp_) {
    timestamp_ = timestamp;
  }

  if (is_wake_vector()) {
    // If this interrupt is configured to use Monotonic timestamps, then we need
    // to capture a new boot timestamp to use as the trigger time for the wake
    // vector.  There is no way (nor will there ever be a way) to convert (after
    // initial capture) from monotonic time to boot time, or vice versa.
    const zx_instant_boot_t boot_time_trigger =
        ((options_ & INTERRUPT_TIMESTAMP_MONO) == 0) ? timestamp : current_boot_time();
    wake_event_.Trigger(boot_time_trigger);
  }

  if (state_ == InterruptState::NEEDACK && port_dispatcher_) {
    // Cannot trigger a interrupt without ACK
    // only record timestamp if this is the first signal since we started waiting
    return ZX_OK;
  }

  if (port_dispatcher_) {
    // Only send a packet if we are not already in the NEEDACK state.  If we are
    // already in NEEDACK, the packet will be sent as soon as the user
    // explicitly acks the interrupt.
    if (state_ != InterruptState::NEEDACK) {
      SendPacketLocked(timestamp);
      state_ = InterruptState::NEEDACK;
    }
  } else {
    Signal();

    // Do not change state to TRIGGERED if we are in the NEEDACK state.  We
    // recorded a timestamp (above) which is the signal to a calling thread that
    // we are in the signaled state, and as soon as a thread blocks on the
    // object again, we will deliver the interrupt to them.
    if (state_ != InterruptState::NEEDACK) {
      state_ = InterruptState::TRIGGERED;
    }
  }
  return ZX_OK;
}

void InterruptDispatcher::InterruptHandler() {
  // Using preempt disable is not necessary for correctness, since we should
  // be in an interrupt context with preemption disabled, but we re-disable anyway
  // for clarity and robustness.
  AutoPreemptDisabler preempt_disable;
  Guard<SpinLock, IrqSave> guard{&spinlock_};

  if (state_ == InterruptState::DESTROYED) {
    return;
  }

  const CurrentTicksObservation trigger_time = timer_current_mono_and_boot_ticks();
  ktl::optional<zx_instant_boot_t> boot_trigger_time;

  // only record timestamp if this is the first IRQ since we started waiting
  if (!timestamp_) {
    if (flags_ & INTERRUPT_TIMESTAMP_MONO) {
      timestamp_ = timer_get_ticks_to_time_ratio().Scale(trigger_time.mono_now);
    } else {
      boot_trigger_time = timestamp_ = timer_get_ticks_to_time_ratio().Scale(trigger_time.boot_now);
    }
  }

  // If we are a wake vector, trigger the wake event which will wake the system
  // if suspended and prevent entering suspend until acknowledged.
  if (is_wake_vector()) {
    wake_event_.Trigger(boot_trigger_time.has_value()
                            ? boot_trigger_time.value()
                            : timer_get_ticks_to_time_ratio().Scale(trigger_time.boot_now));
  }

  if (port_dispatcher_) {
    // Only send a packet if we are not already in the NEEDACK state.  If we are
    // already in NEEDACK, the packet will be sent as soon as the user
    // explicitly acks the interrupt.
    if (state_ != InterruptState::NEEDACK) {
      SendPacketLocked(timestamp_);
      state_ = InterruptState::NEEDACK;
    }
  } else {
    if (flags_ & INTERRUPT_MASK_POSTWAIT) {
      MaskInterrupt();
    }
    Signal();

    // Do not change state to TRIGGERED if we are in the NEEDACK state.  We
    // recorded a timestamp (above) which is the signal to a calling thread that
    // we are in the signaled state, and as soon as a thread blocks on the
    // object again, we will deliver the interrupt to them.
    if (state_ != InterruptState::NEEDACK) {
      state_ = InterruptState::TRIGGERED;
    }
  }
}

zx_status_t InterruptDispatcher::Destroy() {
  const InterruptState old_state = [&]() {
    Guard<SpinLock, IrqSave> guard{&spinlock_};
    return ktl::exchange(state_, InterruptState::DESTROYED);
  }();

  if (old_state == InterruptState::DESTROYED) {
    return ZX_OK;
  }

  // The interrupt may presently have been fired and we could already be about to acquire the
  // spinlock_ in InterruptHandler. If we were to call UnregisterInterruptHandler whilst holding
  // the spinlock_ then we risk a deadlock scenario where the platform interrupt code may have
  // taken a lock to call InterruptHandler, and it might take the same lock when we call
  // UnregisterInterruptHandler.

  MaskInterrupt();
  DeactivateInterrupt();
  UnregisterInterruptHandler();

  // Use preempt disable for correctness to prevent rescheduling when waking a
  // thread while holding the spinlock.
  AutoPreemptDisabler preempt_disable;
  Guard<SpinLock, IrqSave> guard{&spinlock_};

  if (port_dispatcher_) {
    bool packet_was_in_queue = port_dispatcher_->RemoveInterruptPacket(&port_packet_);
    if ((old_state == InterruptState::NEEDACK) && !packet_was_in_queue) {
      return ZX_ERR_NOT_FOUND;
    }
    if ((old_state == InterruptState::IDLE) ||
        ((old_state == InterruptState::NEEDACK) && packet_was_in_queue)) {
      return ZX_OK;
    }
  } else {
    Signal();
  }
  return ZX_OK;
}

zx_status_t InterruptDispatcher::Bind(fbl::RefPtr<PortDispatcher> port_dispatcher, uint64_t key) {
  AutoPreemptDisabler preempt_disable;
  Guard<SpinLock, IrqSave> guard{&spinlock_};
  if (state_ == InterruptState::DESTROYED) {
    return ZX_ERR_CANCELED;
  }
  if (state_ == InterruptState::WAITING) {
    return ZX_ERR_BAD_STATE;
  }
  if (port_dispatcher_) {
    return ZX_ERR_ALREADY_BOUND;
  }

  // If an interrupt is bound to a port there is a conflict between UNMASK_PREWAIT_UNLOCKED
  // and MASK_POSTWAIT because the mask operation will by necessity happen before leaving the
  // dispatcher spinlock, leading to a mask operation immediately followed by the deferred
  // unmask operation.
  if ((flags_ & INTERRUPT_UNMASK_PREWAIT_UNLOCKED) && (flags_ & INTERRUPT_MASK_POSTWAIT)) {
    return ZX_ERR_INVALID_ARGS;
  }

  port_dispatcher_ = ktl::move(port_dispatcher);
  port_packet_.key = key;

  if (state_ == InterruptState::TRIGGERED) {
    SendPacketLocked(timestamp_);
    state_ = InterruptState::NEEDACK;
  }

  return ZX_OK;
}

zx_status_t InterruptDispatcher::Unbind(fbl::RefPtr<PortDispatcher> port_dispatcher) {
  // Moving port_dispatcher_ to the local variable ensures it will not be destroyed while
  // holding this spinlock.
  fbl::RefPtr<PortDispatcher> dispatcher;
  {
    Guard<SpinLock, IrqSave> guard{&spinlock_};

    if (port_dispatcher_ != port_dispatcher) {
      // This case also covers the HasVcpu() case.
      return ZX_ERR_NOT_FOUND;
    }
    if (state_ == InterruptState::DESTROYED) {
      return ZX_ERR_CANCELED;
    }
    // Remove the packet for this interrupt from this port on an unbind before actually
    // doing the unbind. This protects against the case where the interrupt dispatcher
    // goes away between an unbind and a port_wait.
    const bool removed_packet = port_dispatcher_->RemoveInterruptPacket(&port_packet_);
    port_packet_.key = 0;
    dispatcher.swap(port_dispatcher_);

    // If we were in the NEEDACK state, and we successfully removed the packet
    // from the port, then revert back to the TRIGGERED state.  No one has
    // managed to process this interrupt yet, and we don't want to lose it.
    // Someone can either wait on this interrupt, or re-bind it to a new port,
    // and they should still be able to process the interrupt which has not
    // _technically_ been delivered yet.
    //
    // If the packet has already been read, then just stay in the NEEDACK state.
    // Someone has already processed the packet.
    if ((state_ == InterruptState::NEEDACK) && removed_packet) {
      state_ = InterruptState::TRIGGERED;
      timestamp_ = port_packet_.timestamp;
      port_packet_.key = 0;
      port_packet_.timestamp = 0;
    }
  }
  return ZX_OK;
}

zx_status_t InterruptDispatcher::Ack() {
  zx::result<InterruptDispatcher::PostAckState> res = AckInternal();
  return res.is_error() ? res.error_value() : ZX_OK;
}

zx::result<InterruptDispatcher::PostAckState> InterruptDispatcher::AckInternal() {
  PostAckState post_ack_state = FullyAcked;
  bool defer_unmask = false;
  // Use preempt disable to reduce the likelihood of the woken thread running
  // while the spinlock is still held.
  AutoPreemptDisabler preempt_disable;
  {
    Guard<SpinLock, IrqSave> guard{&spinlock_};
    if (port_dispatcher_ == nullptr && !(flags_ & INTERRUPT_ALLOW_ACK_WITHOUT_PORT_FOR_TEST)) {
      return zx::error(ZX_ERR_BAD_STATE);
    }

    if (state_ == InterruptState::DESTROYED) {
      return zx::error(ZX_ERR_CANCELED);
    }

    if (state_ == InterruptState::NEEDACK) {
      if (flags_ & INTERRUPT_UNMASK_PREWAIT) {
        UnmaskInterrupt();
      } else if (flags_ & INTERRUPT_UNMASK_PREWAIT_UNLOCKED) {
        defer_unmask = true;
      }

      if (timestamp_) {
        if (!SendPacketLocked(timestamp_)) {
          // We cannot queue another packet here.
          // If we reach here it means that the
          // interrupt packet has not been processed,
          // another interrupt has occurred & then the
          // interrupt was ACK'd
          return zx::error(ZX_ERR_BAD_STATE);
        }

        // If we are a wake vector, record our last_ack timestamp, but do not
        // clear the signaled state.  We just sent a new port packet, so there
        // is another interrupt signal on its way to user mode.
        if (is_wake_vector()) {
          wake_event_.Acknowledge(wake_vector::WakeEvent::AckBehavior::RemainSignaled);
        }

        post_ack_state = PostAckState::Retriggered;
      } else {
        // There are no other interrupt pending right now.  If we are a wake
        // vector, ack our wake event clearing the signaled state in the
        // process, then return to the IDLE state.
        if (is_wake_vector()) {
          wake_event_.Acknowledge(wake_vector::WakeEvent::AckBehavior::ClearSignaled);
        }
        state_ = InterruptState::IDLE;
      }
    }
  }

  if (defer_unmask) {
    UnmaskInterrupt();
  }
  return zx::ok(post_ack_state);
}

void InterruptDispatcher::on_zero_handles() { Destroy(); }
