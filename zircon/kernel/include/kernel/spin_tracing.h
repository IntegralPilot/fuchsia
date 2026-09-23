// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_SPIN_TRACING_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_SPIN_TRACING_H_

#include <lib/ktrace.h>

#include <fbl/bits.h>
#include <kernel/spin_tracing_config.h>

namespace spin_tracing {

template <bool Enabled = kSchedulerLockSpinTracingEnabled>
class SpinTracingTimestamp;

template <>
class SpinTracingTimestamp<false> {
 public:
  constexpr SpinTracingTimestamp() = default;
  constexpr zx_instant_boot_ticks_t value() const { return 0; }
};

template <>
class SpinTracingTimestamp<true> {
 public:
  SpinTracingTimestamp() = default;
  zx_instant_boot_ticks_t value() const { return value_; }

 private:
  const zx_instant_boot_ticks_t value_{KTrace::Timestamp()};
};

template <bool TraceInstrumented = false>
class Tracer {
 public:
  constexpr Tracer() = default;
  constexpr explicit Tracer(zx_instant_mono_ticks_t) {}
  constexpr void Finish(FinishType, EncodedLockId,
                        SpinTracingTimestamp<false> = SpinTracingTimestamp<false>{}) const {}
};

template <>
class Tracer<true> {
 public:
  Tracer() = default;
  explicit constexpr Tracer(zx_instant_mono_ticks_t ticks) : start_{static_cast<uint64_t>(ticks)} {}

  void Finish(FinishType finish_type, EncodedLockId elid,
              SpinTracingTimestamp<true> end_time = SpinTracingTimestamp<true>{}) const {
    const uint16_t class_name = static_cast<uint16_t>(
        (elid.class_name() != fxt::InternedString::kInvalidId) ? elid.class_name()
                                                               : "<unknown>"_intern.id());

    if constexpr (!kSchedulerLockSpinTracingCompressed) {
      const uint64_t lock_id = elid.id();
      const auto& lock_type =
          elid.lock_type() == LockType::kSpinlock ? "Spinlock"_intern : "Mutex"_intern;
      const bool blocked_after = (finish_type == FinishType::kBlocked);

      FXT_EVENT_COMMON(true, KTrace::CategoryEnabled, KTrace::EmitComplete, "kernel:sched",
                       "lock_spin"_intern, start_, end_time.value(), KTrace::Context::Thread,
                       ("lock_id", lock_id),
                       ("lock_class", fxt::StringRef<fxt::RefType::kId>{class_name}),
                       ("lock_type", lock_type), ("blocked_after", blocked_after));
    } else {
      // TODO(johngro): We could compress this even more if we omitted the
      // dedicated lock_class field (we can extract the value from the elid
      // instead).  To do this, I need to update my external scripts to process
      // the native FXT format.  Currently, it depends on conversion to JSON
      // before processing, which strips the string table from the data
      // (embedding it directly into the JSON instead).
      FXT_EVENT_COMMON(true, KTrace::CategoryEnabled, KTrace::EmitComplete, "kernel:sched",
                       "lock_spin"_intern, start_, end_time.value(), KTrace::Context::Thread,
                       ("lock_class", fxt::StringRef<fxt::RefType::kId>{class_name}),
                       ("elid", elid.FinishedValue(finish_type)));
    }
  }

 private:
  uint64_t start_ = KTrace::Timestamp();
};

}  // namespace spin_tracing

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_SPIN_TRACING_H_
