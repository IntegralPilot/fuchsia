// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <new>

#include <kernel/event.h>
#include <kernel/event_ffi.h>
#include <kernel/ffi.h>

extern "C" {

FFI_ALWAYS_INLINE void cpp_event_init(Event* event, bool initial) {
  DEBUG_ASSERT(event != nullptr);
  new (event) Event(initial);
}

FFI_ALWAYS_INLINE void cpp_event_destroy(Event* event) {
  DEBUG_ASSERT(event != nullptr);
  event->~Event();
}

FFI_ALWAYS_INLINE void cpp_event_signal(Event* event, zx_status_t wait_result) {
  DEBUG_ASSERT(event != nullptr);
  event->Signal(wait_result);
}

FFI_ALWAYS_INLINE void cpp_event_signal_etc(Event* event, zx_status_t wait_result,
                                            OwnedWaitQueue* queue_to_own) {
  DEBUG_ASSERT(event != nullptr);
  event->Signal(wait_result, queue_to_own);
}

FFI_ALWAYS_INLINE zx_status_t cpp_event_unsignal(Event* event) {
  DEBUG_ASSERT(event != nullptr);
  return event->Unsignal();
}

FFI_ALWAYS_INLINE zx_status_t cpp_event_wait(Event* event, zx_instant_mono_t deadline) {
  DEBUG_ASSERT(event != nullptr);
  return event->Wait(Deadline::no_slack(deadline));
}

FFI_ALWAYS_INLINE zx_status_t cpp_event_wait_deadline(Event* event, const Deadline* deadline) {
  DEBUG_ASSERT(event != nullptr);
  DEBUG_ASSERT(deadline != nullptr);
  return event->Wait(*deadline);
}

FFI_ALWAYS_INLINE zx_status_t cpp_event_wait_mask(Event* event, const Deadline* deadline,
                                                  uint32_t signal_mask) {
  DEBUG_ASSERT(event != nullptr);
  DEBUG_ASSERT(deadline != nullptr);
  return event->Wait(*deadline, signal_mask);
}

FFI_ALWAYS_INLINE zx_status_t cpp_event_wait_infinite(Event* event) {
  DEBUG_ASSERT(event != nullptr);
  return event->Wait();
}

FFI_ALWAYS_INLINE zx_status_t cpp_event_wait_interruptible(Event* event, zx_instant_mono_t deadline,
                                                           Interruptible interruptible) {
  DEBUG_ASSERT(event != nullptr);
  return event->WaitDeadline(deadline, interruptible);
}

FFI_ALWAYS_INLINE bool cpp_event_is_signaled(const Event* event) {
  DEBUG_ASSERT(event != nullptr);
  return event->is_signaled();
}

FFI_ALWAYS_INLINE void cpp_autounsignal_event_init(AutounsignalEvent* event, bool initial) {
  DEBUG_ASSERT(event != nullptr);
  new (event) AutounsignalEvent(initial);
}

FFI_ALWAYS_INLINE void cpp_autounsignal_event_destroy(AutounsignalEvent* event) {
  DEBUG_ASSERT(event != nullptr);
  event->~AutounsignalEvent();
}

FFI_ALWAYS_INLINE Event* cpp_autounsignal_event_as_event(AutounsignalEvent* event) {
  DEBUG_ASSERT(event != nullptr);
  return static_cast<Event*>(event);
}

FFI_ALWAYS_INLINE const Event* cpp_autounsignal_event_as_event_const(
    const AutounsignalEvent* event) {
  DEBUG_ASSERT(event != nullptr);
  return static_cast<const Event*>(event);
}

}  // extern "C"
