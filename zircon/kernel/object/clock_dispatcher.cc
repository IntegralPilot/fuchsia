// Copyright 2019 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/affine/ratio.h>
#include <lib/affine/transform.h>
#include <lib/arch/intrin.h>
#include <lib/concurrent/seqlock.inc.h>
#include <lib/counters.h>
#include <lib/page/size.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/syscalls/clock.h>

#include <fbl/alloc_checker.h>
#include <ktl/memory.h>
#include <object/clock_dispatcher.h>
#include <object/vm_object_dispatcher.h>
#include <vm/physmap.h>

KCOUNTER(dispatcher_clock_create_count, "dispatcher.clock.create")
KCOUNTER(dispatcher_clock_destroy_count, "dispatcher.clock.destroy")

namespace {

// TODO(johngro): Find a better place for this, or figure out a better way to
// use the lockdep guards along with libfasttime
template <typename T>
class __TA_SCOPED_CAPABILITY SeqGuard {
 public:
  explicit SeqGuard(T& lock) __TA_ACQUIRE(lock) : irq_state_(arch_interrupt_save()), lock_(lock) {
    lock_.Acquire();
  }

  ~SeqGuard() __TA_RELEASE() {
    lock_.Release();
    arch_interrupt_restore(irq_state_);
  }

 private:
  interrupt_saved_state_t irq_state_;
  T& lock_;
};

// Helpers which normalize access to the two versions of the update args.
template <typename UpdateArgsType>
class UpdateArgsAccessor {
 public:
  static constexpr bool IsV1 = ktl::is_same_v<UpdateArgsType, zx_clock_update_args_v1_t>;
  static constexpr bool IsV2 = ktl::is_same_v<UpdateArgsType, zx_clock_update_args_v2_t>;

  UpdateArgsAccessor(const UpdateArgsType& args) : args_(args) {}
  int32_t rate_adjust() const { return args_.rate_adjust; }
  uint64_t error_bound() const { return args_.error_bound; }

  int64_t synthetic_value() const {
    if constexpr (IsV1) {
      return args_.value;
    } else {
      return args_.synthetic_value;
    }
  }

  // Reference value is an invalid field in the v1 struct.
  int64_t reference_value() const {
    static_assert(!IsV1, "v1 clock update structures have no reference value field");
    return args_.reference_value;
  }

 private:
  const UpdateArgsType& args_;
};

}  // namespace

zx_status_t ClockDispatcher::get_name(char (&out_name)[ZX_MAX_NAME_LEN]) const {
  name_.get(ZX_MAX_NAME_LEN, out_name);
  return ZX_OK;
}

zx_status_t ClockDispatcher::set_name(const char* name, size_t len) {
  KTRACE_KERNEL_OBJECT("kernel:meta", get_koid(), ZX_OBJ_TYPE_CLOCK, (fxt::StringRef{name, len}));
  return name_.set(name, len);
}

zx_status_t ClockDispatcher::Create(uint64_t options, const zx_clock_create_args_v1_t& create_args,
                                    KernelHandle<ClockDispatcher>* handle, zx_rights_t* rights) {
  // The syscall_ layer has already parsed our args version and extracted them
  // into our |create_args| argument as appropriate.  Go ahead and discard the
  // version information before sanity checking the rest of the options.
  options &= ~ZX_CLOCK_ARGS_VERSION_MASK;

  // Reject any request which includes an options flag we do not recognize.
  if (~ZX_CLOCK_OPTS_ALL & options) {
    return ZX_ERR_INVALID_ARGS;
  }

  // If the user asks for a continuous clock, it must also be monotonic
  if ((options & ZX_CLOCK_OPT_CONTINUOUS) && !(options & ZX_CLOCK_OPT_MONOTONIC)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Make sure that the backstop time is valid.  If this clock is being created
  // with the "auto start" flag, then it begins life as a clone of its reference
  // clock (either monotonic or boot) , and the backstop time has to be <= the
  // current reference clock value.  Otherwise, the clock starts in the stopped
  // state, and any specified backstop time must simply be non-negative.
  //
  const zx_time_t now = GetCurrentTime(options & ZX_CLOCK_OPT_BOOT);
  if (((options & ZX_CLOCK_OPT_AUTO_START) && (create_args.backstop_time > now)) ||
      (create_args.backstop_time < 0)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // If the user requested a map-able clock, create a single-page VMO which we
  // will use to share clock state with our user.
  fbl::RefPtr<VmObjectPaged> vmo_paged;
  if ((options & ZX_CLOCK_OPT_MAPPABLE) != 0) {
    // Make sure to allocate our VMO with the `kAlwaysPinned` flag, for two
    // reasons.
    //
    // 1) To save a bit of time and overhead, we use the physmap view of this
    //    page in the kernel in order to access the actual memory.  If the page
    //    backing this clock is not pinned, then this technique is no good.  _In
    //    theory_, the page could be re-claimed then restored (to a different
    //    physical location) invalidating our kernel-physmap view of the memory
    //    in the process.  This cannot be allowed to happen.
    // 2) Even if we make a kernel-specific PTE for the kernel view of the
    //    memory (instead of using the physmap view), it needs to be accessed
    //    from inside of a spinlock-equivalent (the exclusive form of the
    //    seq-lock) during an Update operation.  We are going to be touching the
    //    memory, but cannot allow a page fault during this operation, so it is
    //    important that it always remain pinned.
    static_assert(kMappedSize == kPageSize,
                  "Mapped clock size must be a single page to ensure continuity");
    zx_status_t res = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY | PMM_ALLOC_FLAG_CAN_WAIT,
                                            VmObjectPaged::kAlwaysPinned, kMappedSize, &vmo_paged);
    if (res != ZX_OK) {
      return res;
    }
  }

  fbl::AllocChecker ac;
  KernelHandle clock(fbl::AdoptRef(
      new (&ac) ClockDispatcher(options, create_args.backstop_time, ktl::move(vmo_paged))));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  // The new clock instance should have the default rights, plus the "map" right
  // if the clock was created as map-able.
  *rights = default_rights() | ((options & ZX_CLOCK_OPT_MAPPABLE) ? ZX_RIGHT_MAP : 0);
  *handle = ktl::move(clock);

  return ZX_OK;
}

ClockDispatcher::ClockDispatcher(uint64_t options, zx_time_t backstop_time,
                                 fbl::RefPtr<VmObjectPaged> vmo)
    : vmo_(ktl::move(vmo)) {
  // Find our storage for our clock transformation, either in our VMO if we are
  // mappable, or in our local storage if not.
  if (vmo_ != nullptr) {
    // Find the physical address of our VMO's (single) page, then use it to
    // locate the kernel view of that page in the kernel's flat map.  There
    // should be no possible way for this to fail, so unconditionally assert
    // that everything goes as we expect.
    static_assert(kMappedSize == kPageSize, "Mapped clock size must be exactly one page");

    paddr_t pa;
    const zx_status_t res = vmo_->GetPage(0, 0, nullptr, nullptr, &pa);
    ASSERT_MSG(res == ZX_OK, "Failed to get storage page for mappable clock (%d)", res);
    ASSERT_MSG(is_physmap_phys_addr(pa),
               "Mappable clock storage page is not in the physmap 0x%016lx", pa);
    DEBUG_ASSERT((options & ZX_CLOCK_OPT_MAPPABLE) != 0);
    clock_transformation_ = reinterpret_cast<ClockTransformationType*>(paddr_to_physmap(pa));

    // Set the user-id of our VMO to be the same as our KOID.  This way, when a
    // mapped clock is enumerated in a diagnostic info call, the KOID of this
    // clock will be what gets reported in the info record.
    vmo_->set_user_id(this->get_koid());

    // Clocks (as kernel objects) currently don't have names, so we cannot use a
    // similar trick to apply a name to how our mapped clock is reported.  For
    // now, just set the name of the underlying VMO to "kernel-clock", so that
    // it will be clear to someone looking at diagnostic info that the mapping
    // is for a clock.
    constexpr const char* default_name = "kernel-clock";
    vmo_->set_name(default_name, strlen(default_name));
  } else {
    DEBUG_ASSERT((options & ZX_CLOCK_OPT_MAPPABLE) == 0);
    clock_transformation_ = reinterpret_cast<ClockTransformationType*>(local_storage_);
  }

  // Explicitly placement new our transformation structure in our storage of choice.
  new (clock_transformation_) ClockTransformationType(options, backstop_time);

  // Initialize the internal transformation structure.
  ClockTransformationType& t = *clock_transformation_;
  ClockTransformationType::Params local_params;
  affine::Transform local_ticks_to_synthetic;

  // Compute the initial state
  if (t.options_ & ZX_CLOCK_OPT_AUTO_START) {
    ZX_DEBUG_ASSERT(const zx_time_t now = GetCurrentTime(t.is_boot()); t.backstop_time_ <= now);
    const affine::Ratio ticks_to_time_ratio = timer_get_ticks_to_time_ratio();
    const zx_ticks_t now_ticks = t.GetCurrentTicks();

    local_params.last_value_update_ticks = now_ticks;
    local_params.last_rate_adjust_update_ticks = now_ticks;
    local_ticks_to_synthetic = affine::Transform{
        0, 0, {ticks_to_time_ratio.numerator(), ticks_to_time_ratio.denominator()}};
    local_params.reference_to_synthetic = affine::Transform({0, 0, {1, 1}});
  } else {
    local_ticks_to_synthetic = affine::Transform{0, t.backstop_time_, {0, 1}};
    local_params.reference_to_synthetic = affine::Transform{0, t.backstop_time_, {0, 1}};
  }

  // Publish the state from within the SeqLock
  {
    SeqGuard guard(t.seq_lock_);
    t.reference_ticks_to_synthetic_.Update(local_ticks_to_synthetic);
    t.params_.Update(local_params);
  }

  // If we auto-started our clock, update our state.
  if (options & ZX_CLOCK_OPT_AUTO_START) {
    UpdateState(0, ZX_CLOCK_STARTED);
  }

  kcounter_add(dispatcher_clock_create_count, 1);
}

ClockDispatcher::~ClockDispatcher() {
  // Explicitly destruct our clock transformation instance before its underlying
  // storage goes away.
  ktl::destroy_at(clock_transformation_);
  kcounter_add(dispatcher_clock_destroy_count, 1);
}

zx_status_t ClockDispatcher::Read(zx_time_t* out_now) {
  return clock_transformation_->Read(out_now);
}

zx_status_t ClockDispatcher::GetDetails(zx_clock_details_v1_t* out_details) {
  return clock_transformation_->GetDetails(out_details);
}

template <typename UpdateArgsType>
zx_status_t ClockDispatcher::Update(uint64_t options, const UpdateArgsType& _args) {
  const bool do_set = options & ZX_CLOCK_UPDATE_OPTION_SYNTHETIC_VALUE_VALID;
  const bool do_rate = options & ZX_CLOCK_UPDATE_OPTION_RATE_ADJUST_VALID;
  const bool reference_valid = options & ZX_CLOCK_UPDATE_OPTION_REFERENCE_VALUE_VALID;
  const UpdateArgsAccessor args(_args);

  static_assert((args.IsV1 || args.IsV2) && (args.IsV1 != args.IsV2),
                "Clock update arguments must be either version 1, or version 2");

  // Perform the v1/v2 parameter sanity checks that we can perform without being
  // in the writer lock.
  if constexpr (args.IsV1) {
    // v1 clocks are not allowed to specify a reference value (the v1 struct
    // does not have a field for it)
    if (reference_valid) {
      return ZX_ERR_INVALID_ARGS;
    }
  } else {
    static_assert(args.IsV2, "Unrecognized clock update args version!");

    // A reference value may only be provided during a V2 update as part of
    // either a value set, or rate change operation (or both).
    if (reference_valid && !do_set && !do_rate) {
      return ZX_ERR_INVALID_ARGS;
    }
  }

  bool clock_was_started = false;
  {
    // Disable interrupts and enter the sequence lock exclusively, ensuring that
    // only one update can take place at a time.  We disable interrupts for this
    // because this operation should be very quick, and we may have observers
    // who are spinning attempting to read the clock. We cannot afford to become
    // preempted while we are performing an update operation.
    ClockTransformationType& t = *clock_transformation_;
    SeqGuard guard(t.seq_lock_);

    // If the clock has not yet been started, then we require the first update
    // to include a set operation.
    if (!do_set && !t.is_started()) {
      return ZX_ERR_BAD_STATE;
    }

    // Continue with the argument sanity checking.  Set operations are not
    // allowed on continuous clocks after the very first one (which is what
    // starts the clock).
    if (do_set && t.is_continuous() && t.is_started()) {
      return ZX_ERR_INVALID_ARGS;
    }

    // Checks specific to non-V1 update arguments.
    if constexpr (!args.IsV1) {
      // The following checks only apply if the clock is a monotonic clock which
      // has already been started.
      if (t.is_started() && t.is_monotonic()) {
        // Set operations for non-V1 update arguments made to a monotonic clock
        // must supply an explicit reference time.
        if (do_set && !reference_valid) {
          return ZX_ERR_INVALID_ARGS;
        }

        // non-v1 set operations on monotonic clocks may not be combined with rate
        // change operations.  Additionally, rate change operations may not specify
        // an explicit reference time when being applied to monotonic clocks.
        if (t.is_monotonic() && (do_set || reference_valid) && do_rate) {
          return ZX_ERR_INVALID_ARGS;
        }
      }
    }

    // Make local copies of the core state.  Note that we do not use either
    // acquire semantics on the loads during the copy, nor an acquire thread
    // fence.  We currently have exclusive write access, so no other threads may
    // be writing to these variable as we read them, meaning that no data races
    // should exist here.
    affine::Transform local_ticks_to_synthetic;
    ClockTransformationType::Params local_params;
    t.params_.Read(local_params);
    t.reference_ticks_to_synthetic_.Read(local_ticks_to_synthetic);

    // Aliases make some of the typing a bit shorter.
    affine::Transform& t2s = local_ticks_to_synthetic;
    affine::Transform& m2s = local_params.reference_to_synthetic;

    // Mark the time at which this update will take place.
    int64_t now_ticks = static_cast<int64_t>(t.GetCurrentTicks());

    // Don't bother updating the structures representing the transformation if:
    //
    // 1) We are not changing either the value or rate, or
    // 2a) This is a rate-only change (the value is not being set)
    // 2b) With no explicit reference time provided
    // 2c) Which specifies the same rate that we are already using
    const bool skip_update =
        !do_set &&
        (!do_rate || (!reference_valid && (args.rate_adjust() == local_params.cur_ppm_adj)));

    // Now compute the new transformations
    if (!skip_update) {
      // Figure out the reference times at which this change will take place at.
      affine::Ratio ticks_to_time_ratio = timer_get_ticks_to_time_ratio();
      int64_t now_mono = ticks_to_time_ratio.Scale(now_ticks);
      int64_t reference_ticks = now_ticks;
      int64_t reference_mono = now_mono;
      if constexpr (!args.IsV1) {
        if (reference_valid) {
          reference_mono = args.reference_value();
          reference_ticks = ticks_to_time_ratio.Inverse().Scale(reference_mono);
        }
      }

      // Next, figure out the synthetic value this clock will have after the
      // change.  If this is a set operation, it will be the explicit value
      // provided by the user, otherwise it will be the synthetic value computed
      // using the old transformation applied to the target reference time.
      //
      // In the case that we need to compute the target synthetic time from a
      // previous transformation, use the old mono->synthetic time
      // transformation if the user explicitly supplied a monotonic reference
      // time for the update operation.  Otherwise, use the old ticks->synthetic
      // time transformation along with reference ticks value which we observed
      // after entering the writer lock.
      //
      // In the case of a user supplied monotonic reference time, this avoids
      // rounding error ensures that the old and the new transformations both
      // pass through exactly the same [user_ref, synth] point (important during
      // testing).
      int64_t target_synthetic =
          do_set ? args.synthetic_value()
                 : (reference_valid ? m2s.Apply(reference_mono) : t2s.Apply(reference_ticks));

      // Compute the new rate ratios.
      affine::Ratio new_m2s_ratio;
      affine::Ratio new_t2s_ratio;
      if (do_rate) {
        new_m2s_ratio = {static_cast<uint32_t>(1'000'000 + args.rate_adjust()), 1'000'000};
        new_t2s_ratio =
            affine::Ratio::Product(ticks_to_time_ratio, new_m2s_ratio, affine::Ratio::Exact::No);
      } else if (t.is_started()) {
        new_m2s_ratio = m2s.ratio();
        new_t2s_ratio = t2s.ratio();
      } else {
        new_m2s_ratio = {1, 1};
        new_t2s_ratio = ticks_to_time_ratio;
      }

      // Update the local copies of the structures.
      affine::Transform old_t2s{t2s};
      m2s = {reference_mono, target_synthetic, new_m2s_ratio};
      t2s = {reference_ticks, target_synthetic, new_t2s_ratio};

      // Make certain that the new transformations follow all of the rules
      // before applying them. In specific, we need to make certain that:
      //
      // 1) Monotonic clocks do not move backwards.
      // 2) Backstop times are not violated.
      //
      int64_t new_synthetic_now = t2s.Apply(now_ticks);
      if (t.is_monotonic() && (new_synthetic_now < old_t2s.Apply(now_ticks))) {
        return ZX_ERR_INVALID_ARGS;
      }

      if (new_synthetic_now < t.backstop_time_) {
        return ZX_ERR_INVALID_ARGS;
      }
    }

    // Everything checks out, we can proceed with the update.
    // Record whether or not this is the initial start of the clock.
    clock_was_started = !t.is_started();

    // If this was a set operation, record the new last update time.
    if (do_set) {
      local_params.last_value_update_ticks = now_ticks;
    }

    // If this was a rate adjustment operation, or the clock was just started,
    // record the new last update time as well as the new current ppm
    // adjustment.
    if (do_rate || clock_was_started) {
      local_params.last_rate_adjust_update_ticks = now_ticks;
      local_params.cur_ppm_adj = do_rate ? args.rate_adjust() : 0;
    }

    // If this was an error bounds update operations, record the new last update
    // time as well as the new error bound.
    if (options & ZX_CLOCK_UPDATE_OPTION_ERROR_BOUND_VALID) {
      local_params.last_error_bounds_update_ticks = now_ticks;
      local_params.error_bound = args.error_bound();
    }

    // We are finished,  publish the results in the shared structures.
    t.reference_ticks_to_synthetic_.Update(local_ticks_to_synthetic);
    t.params_.Update(local_params);
  }

  // Now that we are out of the time critical section, if the clock was just
  // started, make sure to assert the ZX_CLOCK_STARTED signal to observers.
  const zx_signals_t set_mask = clock_was_started ? ZX_CLOCK_STARTED : 0;

  // Pulse ZX_CLOCK_UPDATED to announce that a clock update has occurred.
  UpdateState(0, set_mask, ZX_CLOCK_UPDATED);

  return ZX_OK;
}

// Explicit instantiation of the two types of update we might encounter.
template zx_status_t ClockDispatcher::Update(uint64_t options,
                                             const zx_clock_update_args_v1_t& args);
template zx_status_t ClockDispatcher::Update(uint64_t options,
                                             const zx_clock_update_args_v2_t& args);
