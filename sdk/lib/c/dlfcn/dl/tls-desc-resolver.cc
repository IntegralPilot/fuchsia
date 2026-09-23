// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tls-desc-resolver.h"

#include <concepts>
#include <limits>
#include <memory>

#include <fbl/alloc_checker.h>

#include "diagnostics.h"

namespace dl {
namespace {

using Elf = elfldltl::Elf<>;
using Addr = Elf::Addr;
using size_type = Elf::size_type;

template <std::unsigned_integral T, int IndexBits = std::numeric_limits<T>::digits / 2>
struct SplitValue {
  static constexpr int kValueBits = std::numeric_limits<T>::digits;
  static constexpr int kIndexBits = IndexBits;
  static constexpr int kOffsetBits = kValueBits - kIndexBits;
  static constexpr size_type kIndexLimit = size_type{1} << kIndexBits;
  static constexpr size_type kOffsetLimit = size_type{1} << kOffsetBits;

  static constexpr bool CanPack(size_type index, size_type offset) {
    return index < kIndexLimit && offset < kOffsetLimit;
  }

  static constexpr T Pack(size_type index, size_type offset) {
    // The index goes in the high bits.
    return (index << kOffsetBits) | offset;
  }
};

// TlsDescGot::value is two equal bit fields.
struct Split : public SplitValue<Elf::GotEntry<>::value_type> {
  static inline const Addr kFunction =
      reinterpret_cast<uintptr_t>(_dl_tlsdesc_runtime_dynamic_split);
};

// TlsDescGot::value is const TlsdescIndirect*.
const Addr kIndirectFunction = reinterpret_cast<uintptr_t>(_dl_tlsdesc_runtime_dynamic_indirect);

}  // namespace

// The assembly code reads from this.  It's defined here since this has the
// references that link in the assembly code that requires it.  Separate code
// must ensure that it's been set in each thread before any TLSDESC callbacks.
constinit thread_local RawDynamicTlsArray _dl_tlsdesc_runtime_dynamic_blocks;

// The operator() code has determined this is a dynamic TLS case and reduced
// the module ID to an index into _dl_tlsdesc_runtime_dynamic_blocks.  Choose
// how to fill the GOT.
fit::result<bool, TlsDescResolver::TlsDescGot> TlsDescResolver::Dynamic(  //
    Diagnostics& diag, size_type index, size_type offset) const {
  if (Split::CanPack(index, offset)) [[likely]] {
    // Both values fit into their bit fields, so no extra storage is required.
    return fit::ok(TlsDescGot{
        .function = Split::kFunction,
        .value = Split::Pack(index, offset),
    });
  }

  // For larger values, allocate storage for two whole words.  If it came up
  // any less rarely, there could be additional split versions with more bits
  // for offset and fewer for index, for example, which would likely cover most
  // remaining cases.
  fbl::AllocChecker ac;
  std::unique_ptr<TlsdescIndirectStorage> indirect =
      fbl::make_unique_checked<TlsdescIndirectStorage>(
          &ac, TlsdescIndirect{.index = index, .offset = offset});
  if (!ac.check()) [[unlikely]] {
    return fit::error{diag.OutOfMemory("TLSDESC indirect GOT entry", sizeof(*indirect))};
  }

  // Materializing the GOT value is just a glorified cast of the pointer.  The
  // storage is address-stable, so this can be done before moving ownership.
  const auto value = indirect->got_value();

  // The list in the RuntimeModule being relocated will own the storage and
  // free it only when it's certain the module's GOT won't be used.
  indirect_list_.push_front(std::move(indirect));

  return fit::ok(TlsDescGot{.function = kIndirectFunction, .value = value});
}

}  // namespace dl
