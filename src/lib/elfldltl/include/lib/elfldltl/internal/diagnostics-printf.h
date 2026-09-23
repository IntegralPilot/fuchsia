// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_INTERNAL_DIAGNOSTICS_PRINTF_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_INTERNAL_DIAGNOSTICS_PRINTF_H_

#include <inttypes.h>

#include <string_view>
#include <tuple>
#include <type_traits>

#include "../constants.h"
#include "const-string.h"

namespace elfldltl {

using namespace std::literals;

template <typename T, bool Swap>
class UnsignedField;

template <typename T, bool Swap>
class SignedField;

template <typename T>
struct FileOffset;

template <typename T>
struct FileAddress;

class SymbolName;

namespace internal {

// This only exists to be specialized.  The interface is shown here.
template <typename T>
struct PrintfType {
  // The default template also handles other unsigned types that are the
  // same size as one of the uintNN_t types but a different type, which
  // just get widened to uint64_t.  Likewise for signed types to int64_t.
  static_assert(std::is_integral_v<T>, "missing specialization");

  using IntegerType = std::conditional_t<std::is_signed_v<T>, int64_t, uint64_t>;

  consteval PrintfType() = default;

  // The call operator returns a printf format string.
  static consteval std::string Format() {
    if constexpr (std::is_signed_v<T>) {
      return " %" PRId64;
    } else {
      return " %" PRIu64;
    }
  }

  // This is a function of T that returns a std::tuple<...> of the arguments to
  // pass to printf corresponding to that format string.
  static constexpr auto Arguments(IntegerType arg) { return std::make_tuple(arg); }
};

template <typename T>
struct PrintfType<T&> : public PrintfType<T> {};

template <typename T>
struct PrintfType<const T&> : public PrintfType<T> {};

template <typename T>
struct PrintfType<T&&> : public PrintfType<T> {};

template <>
struct PrintfType<uint8_t> {
  static consteval std::string Format() { return " %" PRIu8; }
  static constexpr auto Arguments(uint8_t arg) { return std::make_tuple(arg); }
};

template <>
struct PrintfType<uint16_t> {
  static consteval std::string Format() { return " %" PRIu16; }
  static constexpr auto Arguments(uint16_t arg) { return std::make_tuple(arg); }
};

template <>
struct PrintfType<uint32_t> {
  static consteval std::string Format() { return " %" PRIu32; }
  static constexpr auto Arguments(uint32_t arg) { return std::make_tuple(arg); }
};

template <>
struct PrintfType<int8_t> {
  static consteval std::string Format() { return " %" PRId8; }
  static constexpr auto Arguments(int8_t arg) { return std::make_tuple(arg); }
};

template <>
struct PrintfType<int16_t> {
  static consteval std::string Format() { return " %" PRId16; }
  static constexpr auto Arguments(int16_t arg) { return std::make_tuple(arg); }
};

template <>
struct PrintfType<int32_t> {
  static consteval std::string Format() { return " %" PRId32; }
  static constexpr auto Arguments(int32_t arg) { return std::make_tuple(arg); }
};

// A field.h type will implicitly convert to its underlying integer type, which
// is is why diagnostics-ostream.h doesn't need to define operator<< for them.
// But the generic definition (for uint64_t and equivalents) and the
// specializations for integer types intentionally don't work as template
// instantiations for just any type convertible to their respective integer
// types, because we don't want to use them for signed integer types.
template <typename T, bool Swap>
struct PrintfType<UnsignedField<T, Swap>> : public PrintfType<T> {};

template <typename T, bool Swap>
struct PrintfType<SignedField<T, Swap>> : public PrintfType<std::make_signed_t<T>> {};

template <typename T>
struct FormatFor {
  using Type = T;
  std::string string;
};

template <typename T, class First, class... Rest>
consteval std::string Pick(First first, Rest... rest) {
  if constexpr (std::is_same_v<T, typename First::Type>) {
    return first.string;
  } else {
    static_assert(sizeof...(Rest) > 0, "missing type?");
    return Pick<T>(rest...);
  }
}

template <typename T>
consteval std::string PrintfHexFormatStringForType() {
  return " %#"s + Pick<T>(                                  //
                      FormatFor<uint8_t>(PRIx8),            //
                      FormatFor<uint16_t>(PRIx16),          //
                      FormatFor<uint32_t>(PRIx32),          //
                      FormatFor<uint64_t>(PRIx64),          //
                      FormatFor<unsigned char>("hhx"),      //
                      FormatFor<unsigned short int>("hx"),  //
                      FormatFor<unsigned int>("x"),         //
                      FormatFor<unsigned long int>("lx"),   //
                      FormatFor<unsigned long long int>("llx"));
}

// This handles string literals.  It could fold them into the format
// string, but that would require doubling any '%' inside.
template <size_t N>
struct PrintfType<const char (&)[N]> {
  static consteval std::string Format() { return "%s"; }
  static constexpr auto Arguments(const char (&str)[N]) { return std::forward_as_tuple(str); }
};

template <>
struct PrintfType<const char*> {
  static consteval std::string Format() { return "%s"; }
  static constexpr auto Arguments(const char* str) { return std::make_tuple(str); }
};

// This is specialized on its const& type because it's not copyable.
template <typename CharT, class Traits>
struct PrintfType<const ConstString<CharT, Traits>&> {
  static_assert(std::is_same_v<CharT, char>,
                "PrintfDiagnosticsReport doesn't support non-char instantiations of ConstString");
  static_assert(std::is_same_v<Traits, std::string_view::traits_type>,
                "PrintfDiagnosticsReport doesn't support special instantiations of ConstString");
  static consteval std::string Format() { return "%s"; }
  static constexpr auto Arguments(const ConstString<CharT, Traits>& str) {
    return std::make_tuple(str.c_str());
  }
};

template <>
struct PrintfType<std::string_view> {
  static consteval std::string Format() { return "%.*s"; }
  static constexpr auto Arguments(std::string_view str) {
    return std::make_tuple(static_cast<int>(str.size()), str.data());
  }
};

template <>
struct PrintfType<SymbolName> : public PrintfType<std::string_view> {};

template <typename T>
struct PrintfType<FileOffset<T>> {
  static consteval std::string Format() {
    return " at file offset"s + PrintfHexFormatStringForType<T>();
  }
  static constexpr auto Arguments(FileOffset<T> arg) { return std::make_tuple(*arg); }
};

template <typename T>
struct PrintfType<FileAddress<T>> {
  static consteval std::string Format() {
    return " at relative address"s + PrintfHexFormatStringForType<T>();
  }
  static constexpr auto Arguments(FileAddress<T> arg) { return std::make_tuple(*arg); }
};

template <>
struct PrintfType<ElfMachine> {
  static consteval std::string Format() { return "%.*s (%#" PRIx16 ")"; }
  static constexpr auto Arguments(ElfMachine machine) {
    std::string_view name = ElfMachineName(machine);
    return std::make_tuple(static_cast<int>(name.size()), name.data(),
                           static_cast<uint16_t>(machine));
  }
};

// This concatenates them all together in a mandatory constexpr context so the
// whole format string becomes effectively a single string literal.
template <typename... T>
inline constexpr ConstString kPrintfFormat{[] {  //
  return (std::string{} + ... + PrintfType<T>::Format());
}};

// Calls printer("format string", ...) with arguments corresponding to
// prefix..., args... (each prefix argument and each later argument might
// produce multiple arguments to printer).
template <typename Printer, typename... Prefix, typename... Args>
constexpr void Printf(Printer&& printer, std::tuple<Prefix...> prefix, Args&&... args) {
  constexpr auto printer_args = [](auto&&... args) {
    constexpr const char* kFormat = kPrintfFormat<decltype(args)...>.c_str();
    constexpr auto arg_tuple = [](auto&& arg) {
      using T = decltype(arg);
      return PrintfType<T>::Arguments(std::forward<T>(arg));
    };
    return std::tuple_cat(std::make_tuple(kFormat),
                          arg_tuple(std::forward<decltype(args)>(args))...);
  };
  std::apply(
      std::forward<Printer>(printer),
      std::apply(printer_args, std::tuple_cat(std::move(prefix),
                                              std::forward_as_tuple(std::forward<Args>(args)...))));
}

}  // namespace internal
}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_INTERNAL_DIAGNOSTICS_PRINTF_H_
