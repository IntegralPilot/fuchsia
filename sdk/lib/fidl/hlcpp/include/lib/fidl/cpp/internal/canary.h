// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_CPP_INTERNAL_CANARY_H_
#define LIB_FIDL_CPP_INTERNAL_CANARY_H_

namespace fidl {
namespace internal {

// |Canary| is a stack-allocated object that observes when a boolean is set. It
// is used by |MessageReader| to observe when it is destroyed or unbound from
// the current channel, and by |ProxyController| to observe when it is
// destroyed.
//
// The |Canary| works by storing a pointer to its |should_stop_| field in the
// |MessageReader|.  Upon destruction or unbinding, the |MessageReader| writes
// |true| into |should_stop_|. When we unwind the stack, the |Canary| forwards
// that value to the next |Canary| on the stack.
//
// In |MessageReader|, because |WaitAndDispatchOneMessageUntil| can be called
// re-entrantly, we can be in a state where there are N nested calls to
// |ReadAndDispatchMessage| on the stack. While dispatching any of those
// messages, the client can destroy the |MessageReader| or unbind it from the
// current channel. When that happens we need to stop reading messages from the
// channel and unwind the stack safely.
//
// In |ProxyController|, a user-provided error callback may be called to handle
// errors encountered during message handling. That callback could destroy the
// controller itself, and so accessing member variables after executing the
// callback may cause undefined behavior. We use a canary to detect this
// situation and exit early if the user callback destroys the controller.
class Canary {
 public:
  explicit Canary(bool** should_stop_slot)
      : should_stop_slot_(should_stop_slot),
        previous_should_stop_(*should_stop_slot_),
        should_stop_(false) {
    *should_stop_slot_ = &should_stop_;
  }

  ~Canary() {
    if (should_stop_) {
      // If we should stop, we need to propagate that information to the
      // |Canary| higher up the stack, if any. We also cannot touch
      // |*should_stop_slot_| because the |MessageReader| might have been
      // destroyed (or bound to another channel).
      if (previous_should_stop_)
        *previous_should_stop_ = should_stop_;
    } else {
      // Otherwise, the |MessageReader| was not destroyed and is still bound to
      // the same channel. We need to restore the previous |should_stop_|
      // pointer so that a |Canary| further up the stack can still be informed
      // about whether to stop.
      *should_stop_slot_ = previous_should_stop_;
    }
  }

  // Whether the |ReadAndDispatchMessage| that created the |Canary| should stop
  // after dispatching the current message.
  bool should_stop() const { return should_stop_; }

 private:
  bool** should_stop_slot_;
  bool* previous_should_stop_;
  bool should_stop_;
};

}  // namespace internal
}  // namespace fidl

#endif  // LIB_FIDL_CPP_INTERNAL_CANARY_H_
