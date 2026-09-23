// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_SPIN_TRACING_STORAGE_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_SPIN_TRACING_STORAGE_H_

#include <lib/fxt/interned_string.h>
#include <lib/fxt/string_ref.h>

#include <kernel/lock_trace_config.h>
#include <kernel/lockdep.h>
#include <kernel/spin_tracing_config.h>
#include <ktl/atomic.h>

namespace spin_tracing {

class LockIdGenerator {
 protected:
  LockIdGenerator() = default;
  ~LockIdGenerator() = default;
  static uint64_t CreateId() { return generator_.fetch_add(1, ktl::memory_order_relaxed); }

 private:
  static inline ktl::atomic<uint64_t> generator_{1};
};

template <LockType kLockType, bool Enabled>
class LockNameStorage;

template <LockType kLockType>
class LockNameStorage<kLockType, false> {
 public:
  constexpr void SetLockClassId(lockdep::LockClassId lcid) {}

 protected:
  constexpr LockNameStorage() = default;
  explicit constexpr LockNameStorage(uint16_t) {}
  constexpr EncodedLockId encoded_lock_id() const { return EncodedLockId::Invalid(); }
  fxt::StringRef<fxt::RefType::kId> class_name_ref() const {
    using fxt::operator""_intern;
    return "[disabled]"_intern;
  }
};

template <LockType kLockType>
class LockNameStorage<kLockType, true> : public LockIdGenerator {
 public:
  void SetLockClassId(lockdep::LockClassId lcid) {
    if ((lcid != lockdep::kInvalidLockClassId) &&
        (encoded_lock_id_.class_name() == fxt::InternedString::kInvalidId)) {
      lockdep::MetadataLockClassState* state = lockdep::MetadataLockClassState::Get(lcid);
      const fxt::InternedString* name = &state->interned_name();

      // Note: in a perfect world it should be absolutely impossible for the
      // reference returned by `interned_name()` to be invalid.  Unfortunately,
      // because of _technically_ possible global ctor races, it is possible to
      // be accessing a LockStateClass which has not fully been constructed yet,
      // meaning that its reference to its name might be null (something which
      // _should_ be impossible). Until we get to C++20 and can use
      // consteval/constinit, there is always a small chance that this could
      // happen in an instrumented build.
      //
      // If/when it does happen, it can be a pretty unpleasant experience as the
      // kernel will panic before there is even serial enabled, making things
      // extremely difficult to debug.  Adding this check (and hoping that the
      // compiler does not elide it) makes for a better experience in the worst
      // case scenario.  It is possible for a lock to go unnamed, but at least
      // we won't crash during early boot.
      if (name != nullptr) {
        encoded_lock_id_.SetLockClassId(name->id());
      }
    }
  }

 protected:
  constexpr LockNameStorage()
      : encoded_lock_id_{kLockType, CreateId(), fxt::InternedString::kInvalidId} {}

  explicit LockNameStorage(uint16_t class_name_id)
      : encoded_lock_id_{kLockType, CreateId(), class_name_id} {}

  EncodedLockId encoded_lock_id() const { return encoded_lock_id_; }

  fxt::StringRef<fxt::RefType::kId> class_name_ref() {
    const uint16_t id = encoded_lock_id().class_name();
    if (id == fxt::InternedString::kInvalidId) {
      using fxt::operator""_intern;
      return "[invalid]"_intern;
    }
    return fxt::StringRef{id};
  }

 private:
  EncodedLockId encoded_lock_id_;
};

}  // namespace spin_tracing

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_SPIN_TRACING_STORAGE_H_
