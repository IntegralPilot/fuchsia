// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_FBL_INCLUDE_FBL_NAME_H_
#define ZIRCON_KERNEL_LIB_FBL_INCLUDE_FBL_NAME_H_

#include <lib/zircon-internal/thread_annotations.h>
#include <string.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <kernel/null_lock.h>
#include <kernel/spinlock.h>
#include <ktl/algorithm.h>

namespace fbl {

// As names are often used for diagnostic purposes they might be accessed under various other
// existing locks and to simplify usage they default to being thread safe by use of an internal
// spinlock. Users can override this if they are already providing external synchronization.
enum class ThreadSafe : bool { No, Yes };

// A class for managing names of kernel objects. Since we don't want
// unbounded lengths, the constructor and setter perform
// truncation. Names include the trailing NUL as part of their
// Size-sized buffer.
template <size_t Size, ThreadSafe IsThreadSafe = ThreadSafe::Yes>
class Name {
 public:
  using LockType = ktl::conditional_t<IsThreadSafe == ThreadSafe::Yes, SpinLock, NullLock>;
  // Need room for at least one character and a NUL to be useful.
  static_assert(Size >= 1u, "Names must have size > 1");

  // Create an empty (i.e., "" with exactly 1 byte: a nul) Name.
  Name() {}

  // Create a name from the given data. This will be guaranteed to
  // be nul terminated, so the given data may be truncated.
  Name(const char* name, size_t len) { set(name, len); }

  ~Name() = default;

  // Copy the Name's data out. The written data is guaranteed to be
  // nul terminated, except when out_len is 0, in which case no data
  // is written.
  void get(size_t out_len, char* out_name) const __NONNULL((3)) {
    memset(out_name, 0, out_len);
    if (out_len > 0u) {
      Guard<LockType, IrqSave> lock(&lock_);
      strlcpy(out_name, name_, ktl::min(out_len, Size));
    }
  }

  // Reset the Name to the given data. This will be guaranteed to
  // be nul terminated, so the given data may be truncated.
  zx_status_t set(const char* name, size_t len) __NONNULL((2)) {
    // ignore characters after the first NUL
    len = strnlen(name, len);

    if (len >= Size)
      len = Size - 1;

    Guard<LockType, IrqSave> lock(&lock_);
    memcpy(name_, name, len);
    memset(name_ + len, 0, Size - len);
    return ZX_OK;
  }

  Name& operator=(const Name<Size, IsThreadSafe>& other) {
    if (this != &other) {
      char buffer[Size];
      other.get(Size, buffer);
      set(buffer, Size);
    }
    return *this;
  }

 private:
  [[no_unique_address]] mutable LOCK_DEP_INSTRUMENT(Name, LockType) lock_;
  // This includes the trailing NUL.
  char name_[Size] TA_GUARDED(lock_) = {};
};

}  // namespace fbl

#endif  // ZIRCON_KERNEL_LIB_FBL_INCLUDE_FBL_NAME_H_
