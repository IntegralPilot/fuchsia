// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_DIAGNOSTICS_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_DIAGNOSTICS_H_

#include <stdio.h>

#include <string_view>
#include <tuple>
#include <type_traits>
#include <utility>

#include "field.h"
#include "internal/const-string.h"
#include "internal/diagnostics-printf.h"
#include "internal/no_unique_address.h"

namespace elfldltl {

using namespace std::literals;

// Various template APIs use a polymorphic "diagnostics object" argument.
//
// This object is responsible for reporting errors and for the policy on when
// to bail out of processing ELF data early.  All processing using this object
// is implicitly related to a single ELF file, so error details and locations
// always refer to that file.
//
// A diagnostics object must implement a few simple methods:
//
// * `bool FormatError(std::string_view error, ...)`
//
//   This is called to report a fatal error in the ELF data.  The return value
//   tells the caller whether to continue processing to the extent safely
//   possible after the error.
//
//   The first argument is a string constant with permanent extent which
//   describes the error and can be referred to indefinitely.  If there are
//   additional arguments they have one of three types:
//
//    * `size_type`, the address-sized unsigned integral type for the file.
//      This argument is a value from the file that the error complains about.
//      It's canonical to show these in decimal.
//
//    * `elfldltl::FileOffset<size_type>`
//      This argument is the offset in the ELF file, where the bad data is.
//      It's canonical to show these in hexadecimal.
//
//    * `elfldltl::FileAddress<size_type>`
//      This argument is the address relative to the load bias of the ELF file,
//      where the bad data is.  It's canonical to show these in hexadecimal.
//
//    * `std::string_view`, `const char*`, or string literals
//      These strings are just concatenated.  Note that while the integer-based
//      types are expected to be formatted with a leading space, strings are
//      expected to just be appended verbatim.  So a typical call might look
//      like `FormatError("bad value", 123, " in something indexed", 456)` to
//      yield a result like `"bad value 123 in something indexed 456"`.
//
//   The `elfldltl::FileOffset` and `elfldltl::FileAddress` types each provide
//   a `static constexpr std::string_view kDescription` with canonical text
//   to precede the integer value (usually shown in hexadecimal).
//
//   Essentially this is an input-dependent assertion failure.  FormatError is
//   called exclusively for anomalies that can be explained only by a corrupted
//   ELF file or memory image or by a linker bug.  Processing cannot succeed
//   and no code or data from this file should be used.  The diagnostics object
//   should return true only for the purpose of logging additional errors from
//   the same file before abandoning it.  The processor may attempt additional
//   work but will only do what it can do safely without assertion failures or
//   other risks of crashing.  The bad data it has already encountered could
//   lead to a cascade of additional errors with entirely bogus details, but it
//   might be possible to get coherent reports of multiple independent errors.
//
// * `bool FormatWarning(std::string_view error, ...)`
//
//   This is like FormatError, but for issues that are less problematic.  These
//   are anomalies that probably constitute bugs in the ELF file, but plausibly
//   could be the result of build-time errors or dubious practices by the
//   programmer rather than a bug in the tools or corrupted data per se.  It's
//   probably safe enough to ignore these issues and use the file regardless.
//
// * `<size_t MaxObjects> bool ResourceLimit(std::string_view error, size_t requested)`
// * bool ResourceLimit(size_t max, std::string_view error, size_t requested)`
//
//   The ResourceLimit methods are used to format errors related to imposed
//   resource limits, like with StaticVector. A ResourceLimit is not caused
//   by system pressure and is expected that the same call that yielded a
//   ResourceLimit error on an unchanged object will do so again. The templated
//   version is preferred and the non templated version should be used when
//   the limit of the resource are unknown at compile time like
//   PreallocatedVector with a dynamic extent.
//
// * `bool UndefinedSymbol(std::string_view sym_name)`
//
//    UndefinedSymbol is an error used when the current linking task cannot
//    be completed because of an undefined symbol.
//
// * `bool MissingDependency(std::string_view soname)`
//
//    MissingDependency is used when a DT_NEEDED dependency cannot be found.
//
// * `bool OutOfMemory(std::string_view error, size_t bytes)`
//
//    OutofMemory is used when a memory allocation failure occurs. In contrast
//    to a ResourceLimit error, an OutOfMemory error arises from memory pressure
//    on the system instead of a exceeding a predefined fixed limit capacity.
//
// * `bool SystemError(std::string_view error, ...)`
//
//    SystemError is used when the system cannot fulfill an otherwise valid
//    request likely unrelated to the contents of the ELF file. SystemError
//    can optionally take PosixError and ZirconError objects to give more
//    context to the error encountered. Those two types are found in posix.h
//    and zircon.h, and take either an errno value or zx_status_t respectively.
//
// * `bool extra_checking() const`
//
//   If this returns true, the processor may do some extra work that is not
//   necessary for its correct operation but just offers an opportunity to
//   notice anomalies in the ELF data and report errors or warnings that might
//   otherwise go unnoticed.  Extra checking can be avoided if the use case is
//   optimized for performance over maximal format strictness, or if the
//   diagnostics object is ignoring warnings, etc.
//

// This wraps an unsigned integral type to represent an offset in the ELF file.
template <typename size_type>
struct FileOffset {
  static_assert(std::is_integral_v<size_type>);
  static_assert(std::is_unsigned_v<size_type>);

  using value_type = size_type;

  constexpr value_type operator*() const { return offset; }

  constexpr bool operator==(const FileOffset& other) const { return offset == other.offset; }
  constexpr bool operator!=(const FileOffset& other) const { return offset != other.offset; }

  static constexpr std::string_view kDescription = "file offset";

  value_type offset;
};

// Deduction guides.

template <typename size_type>
FileOffset(size_type) -> FileOffset<size_type>;

template <typename size_type, bool kSwap>
FileOffset(UnsignedField<size_type, kSwap>) -> FileOffset<size_type>;

// Helper to discover if T is a FileOffset type.
template <typename T>
inline constexpr bool kIsFileOffset = false;

template <typename size_type>
inline constexpr bool kIsFileOffset<FileOffset<size_type>> = true;

// This wraps an unsigned integral type to represent an address in the ELF
// file's load image, i.e. such that the p_vaddr of the first PT_LOAD segment
// corresponds to that segment's p_offset in the file.
template <typename size_type>
struct FileAddress {
  static_assert(std::is_integral_v<size_type>);
  static_assert(std::is_unsigned_v<size_type>);

  using value_type = size_type;

  constexpr value_type operator*() const { return address; }

  constexpr bool operator==(const FileAddress& other) const { return address == other.address; }
  constexpr bool operator!=(const FileAddress& other) const { return address != other.address; }

  static constexpr std::string_view kDescription = "file-relative address";

  value_type address;
};

// Deduction guides.
template <typename size_type>
FileAddress(size_type) -> FileAddress<size_type>;

template <typename size_type, bool kSwap>
FileAddress(UnsignedField<size_type, kSwap>) -> FileAddress<size_type>;

// Helper to discover if T is a FileAddress type.
template <typename T>
inline constexpr bool kIsFileAddress = false;

template <typename size_type>
inline constexpr bool kIsFileAddress<FileAddress<size_type>> = true;

// These flags are used by the elfldltl::Diagnostics template implementation.
// This is the default for its template parameter.  Any other class can be used
// as long as it provides members that are contextually convertible to bool
// with these names.
struct DiagnosticsFlags {
  // If true, keep going after errors so more errors can be diagnosed.
  bool multiple_errors = false;

  // If true, then warnings are treated like errors and obey the multiple_errors
  // setting too.  If false, then always keep going after a warning.
  bool warnings_are_errors = true;

  // If true, do extra work to diagnose more errors that could be ignored.
  bool extra_checking = false;
};

// This is an empty subclass of std::true_type or std::false_type, but
// different Index values yield distinct subclass types.  This is necessary for
// [[no_unique_address]] semantics to achieve an empty struct if two adjacent
// fields have the same Value.
template <bool Value, size_t Index>
struct FixedBool : std::integral_constant<bool, Value> {};

// An alternative Flags type can be defined like this one to make one or more
// of the values fixed, or to change the default value of a mutable flag.
struct DiagnosticsPanicFlags {
  ELFLDLTL_NO_UNIQUE_ADDRESS FixedBool<false, 0> multiple_errors;
  ELFLDLTL_NO_UNIQUE_ADDRESS FixedBool<true, 1> warnings_are_errors;
  ELFLDLTL_NO_UNIQUE_ADDRESS FixedBool<false, 2> extra_checking;
};

// elfldltl::Diagnostics provides a canonical implementation of a diagnostics
// object.  It wraps any callable object that takes the std::string_view and
// other arguments passed to FormatError.
//
// The Flags type can be DiagnosticsFlags or any type with those three member
// names having types convertible to bool.  The Flags object passed to the
// constructor (or default-constructed) determines the behavior.  The flags()
// method returns the Flags copy in the diagnostics object, which can then be
// changed in place.  The diagnostics object tracks the numbers of errors and
// warnings reported, unless Flags::multiple_errors is std::false_type.
//
// Convenience functions below return some canonical specializations of this.
//
template <typename Report, class Flags = DiagnosticsFlags>
class Diagnostics {
 public:
  constexpr Diagnostics(const Diagnostics&) = default;
  constexpr Diagnostics(Diagnostics&&) noexcept = default;

  explicit constexpr Diagnostics(Report report) : report_(std::move(report)) {}

  constexpr Diagnostics(Report report, Flags flags)
      : report_(std::move(report)), flags_(std::move(flags)) {}

  constexpr Flags& flags() { return flags_; }
  constexpr const Flags& flags() const { return flags_; }

  constexpr Report& report() { return report_; }
  constexpr const Report& report() const { return report_; }

  constexpr unsigned int errors() const { return errors_; }

  constexpr unsigned int warnings() const { return warnings_; }

  // Reset the counters.
  // This doesn't do anything to the state of the Report object.
  constexpr void reset() {
    errors_ = {};
    warnings_ = {};
  }

  // The following methods are the actual "diagnostics" API as described above.

  template <typename... Args>
  constexpr bool FormatError(std::string_view error, Args&&... args) {
    ++errors_;
    return report_(error, std::forward<Args>(args)...) && flags_.multiple_errors;
  }

  template <typename... Args>
  constexpr bool FormatWarning(std::string_view error, Args&&... args) {
    ++warnings_;
    return report_(error, std::forward<Args>(args)...) &&
           (flags_.multiple_errors || !flags_.warnings_are_errors);
  }

  constexpr bool extra_checking() const { return flags_.extra_checking; }

  template <size_t MaxObjects>
  constexpr bool ResourceLimit(std::string_view error, size_t requested) {
    constexpr internal::ConstString msg{
        []() { return ": maximum "s + internal::ToString(MaxObjects) + " < requested "s; }};
    return FormatError(error, static_cast<const std::string_view&>(msg), requested);
  }

  constexpr bool ResourceLimit(size_t max, std::string_view error, size_t requested) {
    return FormatError(error, ": maximum"sv, max, " < requested"sv, requested);
  }

  template <typename... Args>
  constexpr bool SystemError(std::string_view error, Args&&... args) {
    return FormatError(error, args...);
  }

  constexpr bool UndefinedSymbol(std::string_view sym_name) {
    return FormatError("undefined symbol: ", sym_name);
  }

  constexpr bool MissingDependency(std::string_view soname) {
    return SystemError("cannot open dependency: ", soname);
  }

  constexpr bool OutOfMemory(std::string_view error, size_t bytes) {
    return SystemError("cannot allocate ", bytes, " bytes for ", error);
  }

 private:
  // This is either a wrapper around an integer, or is an empty object.
  // The tag is unused but makes the two Count types always distinct so
  // that adjacent empty members with [[no_unique_address]] can be elided.
  template <bool Counting, auto Tag>
  struct Count;

  // When counting, a trivial wrapper around an integer.
  template <auto Tag>
  struct Count<true, Tag> {
    constexpr Count& operator++() {
      ++value_;
      return *this;
    }

    constexpr operator unsigned int() const noexcept { return value_; }

    unsigned int value_ = 0;
  };

  // When not counting, increments are no-ops and the count is always one.
  template <auto Tag>
  struct Count<false, Tag> {
    constexpr Count& operator++() { return *this; }

    constexpr operator unsigned int() const noexcept { return 1; }
  };

  // If multiple_errors is actually std::false_type, then use the empty objects
  // so we don't bother to keep counts at all.
  static constexpr bool kCount =
      !std::is_base_of_v<std::false_type, decltype(std::declval<Flags>().multiple_errors)>;

  ELFLDLTL_NO_UNIQUE_ADDRESS Report report_;
  ELFLDLTL_NO_UNIQUE_ADDRESS Flags flags_;
  ELFLDLTL_NO_UNIQUE_ADDRESS Count<kCount, &Flags::multiple_errors> errors_;
  ELFLDLTL_NO_UNIQUE_ADDRESS Count<kCount, &Flags::warnings_are_errors> warnings_;
};

// This creates a callable object to use as the Report function in a
// Diagnostics object; it calls the printer function like calling printf.  The
// remaining prefix arguments are treated like initial arguments passed to
// every Diagnostics::FormatError call.  The printer is called with a format
// string literal, followed by various argument types corresponding to the '%'
// formats used therein.  That format and argument sequence is generated based
// on the argument types as passed to FormatError.
template <typename Printer, typename... Prefix>
constexpr auto PrintfDiagnosticsReport(Printer&& printer, Prefix&&... prefix) {
  return [printer = std::forward<Printer>(printer),
          prefix = std::make_tuple(std::forward<Prefix>(prefix)...)](auto&&... args) {
    internal::Printf(printer, prefix, std::forward<decltype(args)>(args)...);
    return true;
  };
}

// This is just PrintfDiagnosticsReport with a printer function that calls
// fprintf with the given FILE* argument.
template <typename... Prefix>
constexpr auto FprintfDiagnosticsReport(FILE* stream, Prefix&&... prefix) {
  auto printer = [stream](auto&&... args) {
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wformat-nonliteral"
    fprintf(stream, std::forward<decltype(args)>(args)...);
#pragma GCC diagnostic pop
  };
  return PrintfDiagnosticsReport(printer, std::forward<Prefix>(prefix)...);
}

// This returns a Diagnostics object that crashes immediately for any error or
// warning.  There are no library dependencies of any kind.  This behavior is
// appropriate only for self-relocation and bootstrapping cases where if there
// is anything wrong in the ELF data then something went wrong in building this
// program itself and it shouldn't be running at all.
constexpr auto TrapDiagnostics() {
  constexpr auto trap = [](auto&&... args) -> bool {
    __builtin_trap();
    return false;
  };
  return Diagnostics(trap, DiagnosticsPanicFlags());
}

// This returns a Diagnostics object that simply stores a single error or
// warning message string.  It always request early bail-out for errors on the
// expectation that only one error will be reported.  But if the same object is
// indeed called again for another failure, the new error message will replace
// the old one.
template <typename T, typename... Flags>
constexpr auto OneStringDiagnostics(T& holder, Flags&&... flags) {
  auto set_error = [&holder](std::string_view error, auto&&... args) {
    holder = error;
    return false;
  };
  return Diagnostics(set_error, std::forward<Flags>(flags)...);
}

// This returns a Diagnostics object that collects a container of messages.
template <typename T, typename... Flags>
constexpr auto CollectStringsDiagnostics(T& container, Flags&&... flags) {
  auto add_error = [&container](std::string_view error, auto&&... args) {
    container.emplace_back(error);
    return true;
  };
  return Diagnostics(add_error, std::forward<Flags>(flags)...);
}

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_DIAGNOSTICS_H_
