// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_STARTUP_PROCESSARGS_H_
#define LIB_C_STARTUP_PROCESSARGS_H_

#include <lib/ld/processargs.h>
#include <lib/zx/handle.h>
#include <zircon/startup.h>
#include <zircon/types.h>

#include <cstdint>
#include <ranges>
#include <span>

#include "../asm-linkage.h"
#include "src/__support/macros/config.h"

namespace LIBC_NAMESPACE_DECL {

// This saves the data from the <zircon/processargs.h> bootstrap message.  It's
// allocated in its own GuardedPageBlock.  It's actually variable-sized, with a
// flexible array member at the end.
class Processargs {
 public:
  using Buffer = ld::ProcessargsBuffer<0, 0>;

  // This is the implementation function aliased to _zx_startup_get_handles.
  //
  // It's not really meant to be used as a weak symbol per se--it will always
  // be defined and won't ever be overridden by another definition.  However,
  // declaring it as weak prevents the compiler's language-level logic from
  // presuming that it cannot possibly compare equal to another function symbol
  // (as aliases are outside the C/C++ linkage model) without also hiding their
  // equality from LTO constant propagation and dead-code elimination (as
  // laundering either pointer through an empty asm would, for example).
  [[gnu::weak]] static zx_startup_handles_t GetHandles(zx_handle_t bootstrap_handle)
      LIBC_ASM_LINKAGE_DECLARE(ProcessargsGetHandles);

  explicit Processargs(const Buffer& msg, std::span<const zx_handle_t> msg_handles,
                       Buffer::Actual actual, uint32_t strtab_off);

  int argc() const { return static_cast<int>(argc_); }

  std::span<char*> argv() { return {&arrays_[0], argc_}; }

  std::span<char*> envp() {
    // There is a nullptr terminator after argv.last().
    return {&arrays_[argc_ + 1], envc_};
  }

  // The handles and their info must be kept alive for zx_take_startup_handle.
  std::span<uint32_t> handle_info() { return handle_arrays().subspan(0, handlec_); }
  std::span<zx_handle_t> handles() { return handle_arrays().subspan(handlec_); }

  // Returns a range of {uint32_t info, zx::handle take()} pairs such that
  // calling take() consumes the handle that matches info while calling
  // take(std::in_place) just borrows it.
  static auto HandleTakers(std::span<uint32_t> handle_info, std::span<zx_handle_t> handles) {
    auto taker = [handle_info, handles](size_t i) {
      struct Take {
        zx::handle operator()() {
          info = 0;
          return zx::handle{std::exchange(handle, ZX_HANDLE_INVALID)};
        }

        zx::unowned_handle operator()(std::in_place_t) const { return zx::unowned_handle{handle}; }

        uint32_t& info;
        zx_handle_t& handle;
      };
      Take take = {
          .info = handle_info[i],
          .handle = handles[i],
      };
      return std::make_pair(handle_info[i], take);
    };
    return std::views::transform(std::ranges::iota_view(size_t{0}, handle_info.size()), taker);
  }

  // The names table is only used by fdio startup (__libc_extensions_init),
  // which doesn't save pointers into it.  But it has to be allocated somewhere
  // without using the normal heap, and it's not likely to bump up the size of
  // the GuardedPageBlock so it's not worth the juggling to trim it when dead.
  std::span<char*> names() { return {&arrays_[argc_ + 1 + envc_ + 1 + 2], namec_}; }

 private:
  std::span<uint32_t> handle_arrays() {
    return {
        reinterpret_cast<uint32_t*>(&arrays_[argc_ + 1 + envc_ + 3 + namec_]),
        static_cast<size_t>(handlec_) * 2,
    };
  }

  uint32_t argc_ = 0, envc_ = 0, namec_ = 0, handlec_ = 0, strtab_size_ = 0;
  char* arrays_[];
};

}  // namespace LIBC_NAMESPACE_DECL

#endif  // LIB_C_STARTUP_PROCESSARGS_H_
