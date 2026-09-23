// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ZBITL_INCLUDE_LIB_ZBITL_CHECKING_H_
#define SRC_LIB_ZBITL_INCLUDE_LIB_ZBITL_CHECKING_H_

#include <lib/fit/result.h>
#include <lib/zbi-format/zbi.h>
#include <zircon/assert.h>

#include <string_view>

namespace zbitl {

// Validates ZBI item and container headers, returning a description of the
// failure in that event. The check is agnostic of storage capacity; whether
// the encoded length is sensible is left to the caller.
fit::result<std::string_view> CheckItemHeader(const zbi_header_t& header);
fit::result<std::string_view> CheckContainerHeader(const zbi_header_t& header);

// Modify a header so that it passes checks.  This can be used to mint new
// items from a designated initializer that omits uninteresting bits.
constexpr zbi_header_t SanitizeHeader(zbi_header_t header) {
  header.magic = ZBI_ITEM_MAGIC;
  header.flags |= ZBI_FLAGS_VERSION;
  if (!(header.flags & ZBI_FLAGS_CRC32)) {
    header.crc32 = ZBI_ITEM_NO_CRC32;
  }
  return header;
}

/// Returns empty if and only if the ZBI is bootable, otherwise an error
/// string.  This takes any zbitl::View type or any type that acts like it.
/// Note this does not check for errors from zbi.take_error() so if Zbi is
/// zbitl::View then the caller must use zbi.take_error() afterwards.  This
/// function always scans every item so all errors Zbi::iterator detects will
/// be found.  But this function's return value only indicates if the items
/// that were scanned before any errors were encountered added up to a complete
/// ZBI (regardless of whether there were additional items with errors).
template <class Zbi>
constexpr fit::result<typename Zbi::Error> CheckBootable(  //
    Zbi zbi, uint32_t kernel_type
#ifdef __aarch64__
             = ZBI_TYPE_KERNEL_ARM64
#elif defined(__riscv)
             = ZBI_TYPE_KERNEL_RISCV64
#elif defined(__x86_64__) || defined(__i386__)
             = ZBI_TYPE_KERNEL_X64
#endif
) {
  using Error = Zbi::Error;

  const auto first = zbi.begin();
  auto it = first;
  while (it != zbi.end() && it->header->type != kernel_type) {
    ++it;
  }

  // Any error before seeing a kernel item is a problem.  Once the kernel
  // item is found, the rest of the ZBI isn't checked for iterability.
  if (auto result = zbi.take_error(); result.is_error()) {
    return result.take_error();
  }

  if (it == zbi.end()) {
    // No kernel item found.  Why?
    if (it == first) {
      return fit::error{Error{.zbi_error = "empty ZBI"}};
    }
    return fit::error{Error{.zbi_error = "no kernel item found"}};
  }

  if (it != first) {
    return fit::error{
        Error{.zbi_error = "kernel item out of order: must be first"},
    };
  }

  return fit::ok();
}

}  // namespace zbitl

#endif  // SRC_LIB_ZBITL_INCLUDE_LIB_ZBITL_CHECKING_H_
