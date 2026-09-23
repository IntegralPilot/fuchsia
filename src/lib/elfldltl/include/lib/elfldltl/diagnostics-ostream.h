// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_DIAGNOSTICS_OSTREAM_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_DIAGNOSTICS_OSTREAM_H_

#include <ios>

#include "constants.h"
#include "diagnostics.h"
#include "internal/const-string.h"

// This provides adapters for elfldltl::Diagnostics types to use std::ostream
// and compatible types for reporting messages.  This is in a separate header
// from <lib/elfldltl/diagnostics.h> because std::ostream is not available in
// all environments (such as kernels).

namespace elfldltl {

// These overloads let the object returned by OstreamDiagnostics format the
// special argument types.

template <typename S, typename CharT, class Traits>
constexpr decltype(auto) operator<<(S&& ostream,
                                    const internal::ConstString<CharT, Traits>& string) {
  using View = std::basic_string_view<CharT, Traits>;
  return std::forward<S>(ostream) << static_cast<View>(string);
}

template <typename S, typename T>
constexpr decltype(auto) operator<<(S&& ostream, FileOffset<T> offset) {
  auto flags = ostream.flags();
  ostream << std::showbase << std::hex << " at file offset " << *offset;
  ostream.flags(flags);
  return std::forward<S>(ostream);
}

template <typename S, typename T>
constexpr decltype(auto) operator<<(S&& ostream, FileAddress<T> address) {
  auto flags = ostream.flags();
  ostream << std::showbase << std::hex << " at relative address " << *address;
  ostream.flags(flags);
  return std::forward<S>(ostream);
}

template <typename S>
constexpr decltype(auto) operator<<(S&& ostream, ElfMachine machine) {
  auto flags = ostream.flags();
  ostream << ElfMachineName(machine) << " (" << std::showbase << std::hex
          << static_cast<uint16_t>(machine) << ")";
  ostream.flags(flags);
  return std::forward<S>(ostream);
}

template <typename S, typename T, typename = decltype(std::declval<T>().str())>
constexpr decltype(auto) operator<<(S&& ostream, T&& t) {
  return std::forward<S>(ostream) << t.str();
}

// This returns a Report callable that uses << on an ostream-style object.
// Any additional arguments are passed via << as a prefix on each message.  The
// ostream should probably be in << std::hex state for the output to look good.
template <typename Ostream, typename... Args>
constexpr auto OstreamDiagnosticsReport(Ostream& ostream, Args&&... prefix) {
  return [prefix = std::make_tuple(std::forward<Args>(prefix)...), &ostream](
             std::string_view error, auto&&... args) mutable -> bool {
    std::apply([&](auto&&... prefix) { ((ostream << prefix), ...); }, prefix);
    ostream << error;
    ((ostream << args), ...);
    ostream << "\n";
    return true;
  };
}

// This returns a Diagnostics object using OstreamDiagnosticsReport.
template <typename Ostream, class Flags = DiagnosticsFlags, typename... Args>
constexpr auto OstreamDiagnostics(Ostream& ostream, Flags&& flags = {}, Args&&... prefix) {
  return Diagnostics(OstreamDiagnosticsReport(ostream, std::forward<Args>(prefix)...), flags);
}

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_DIAGNOSTICS_OSTREAM_H_
