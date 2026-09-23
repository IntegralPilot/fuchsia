// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_EVENT_FFI_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_EVENT_FFI_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

#include <kernel/deadline.h>
#include <kernel/event.h>
#include <kernel/ffi.h>
#include <kernel/owned_wait_queue.h>
#include <kernel/thread.h>

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
__BEGIN_CDECLS

// Event FFI routines
FFI_ALWAYS_INLINE void cpp_event_init(Event* event, bool initial);
FFI_ALWAYS_INLINE void cpp_event_destroy(Event* event);
FFI_ALWAYS_INLINE void cpp_event_signal(Event* event, zx_status_t wait_result);
FFI_ALWAYS_INLINE void cpp_event_signal_etc(Event* event, zx_status_t wait_result,
                                            OwnedWaitQueue* queue_to_own);
FFI_ALWAYS_INLINE zx_status_t cpp_event_unsignal(Event* event);
FFI_ALWAYS_INLINE zx_status_t cpp_event_wait(Event* event, zx_instant_mono_t deadline);
FFI_ALWAYS_INLINE zx_status_t cpp_event_wait_deadline(Event* event, const Deadline* deadline);
FFI_ALWAYS_INLINE zx_status_t cpp_event_wait_mask(Event* event, const Deadline* deadline,
                                                  uint32_t signal_mask);
FFI_ALWAYS_INLINE zx_status_t cpp_event_wait_infinite(Event* event);
FFI_ALWAYS_INLINE zx_status_t cpp_event_wait_interruptible(Event* event, zx_instant_mono_t deadline,
                                                           Interruptible interruptible);
FFI_ALWAYS_INLINE bool cpp_event_is_signaled(const Event* event);

// AutounsignalEvent FFI routines
FFI_ALWAYS_INLINE void cpp_autounsignal_event_init(AutounsignalEvent* event, bool initial);
FFI_ALWAYS_INLINE void cpp_autounsignal_event_destroy(AutounsignalEvent* event);
FFI_ALWAYS_INLINE Event* cpp_autounsignal_event_as_event(AutounsignalEvent* event);
FFI_ALWAYS_INLINE const Event* cpp_autounsignal_event_as_event_const(
    const AutounsignalEvent* event);

__END_CDECLS

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_EVENT_FFI_H_
