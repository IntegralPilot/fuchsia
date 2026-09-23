// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_UTILS_H_
#define TOOLS_FIDL_FIDLC_SRC_UTILS_H_

#include <errno.h>
#include <lib/stdcompat/span.h>
#include <zircon/assert.h>

#include <clocale>
#include <cstdlib>
#include <cstring>
#include <string>
#include <string_view>

#include "re2/re2.h"
#include "tools/fidl/fidlc/src/findings.h"

namespace fidlc {

// The std::visit helper from https://en.cppreference.com/w/cpp/utility/variant/visit.
template <class... Ts>
struct overloaded : Ts... {
  using Ts::operator()...;
};
template <class... Ts>
overloaded(Ts...) -> overloaded<Ts...>;

// Clones a vector of unique_ptr by calling Clone() on each element.
template <typename T>
std::vector<std::unique_ptr<T>> MapClone(const std::vector<std::unique_ptr<T>>& original) {
  std::vector<std::unique_ptr<T>> cloned;
  cloned.reserve(original.size());
  for (const auto& item : original) {
    cloned.push_back(item->Clone());
  }
  return cloned;
}

// Validators for parts of the FIDL grammar.
// See https://fuchsia.dev/fuchsia-src/reference/fidl/language/language#identifiers
bool IsValidLibraryComponent(std::string_view component);
bool IsValidIdentifierComponent(std::string_view component);
bool IsValidFullyQualifiedMethodIdentifier(std::string_view fq_identifier);

// Validates a @discoverable name. This is like a fully qualified identifier,
// but uses a dot instead of a slash so it can be used in a filesystem path.
bool IsValidDiscoverableName(std::string_view discoverable_name);

// Removes the double quotes from the start and end of a string literal.
std::string StripStringLiteralQuotes(std::string_view str);
// Removes the indentation and "///" from each line of a doc comment.
std::string StripDocCommentSlashes(std::string_view str);

bool IsLowerSnakeCase(std::string_view str);
bool IsUpperSnakeCase(std::string_view str);
bool IsLowerCamelCase(std::string_view str);
bool IsUpperCamelCase(std::string_view str);

std::string ToLowerSnakeCase(std::string_view str);
std::string ToUpperSnakeCase(std::string_view str);
std::string ToLowerCamelCase(std::string_view str);
std::string ToUpperCamelCase(std::string_view str);

// Splits an identifier into lowercase words. Works for any naming style.
std::vector<std::string> SplitIdentifierWords(std::string_view str);

// Returns the canonical form of an identifier, used to detect name collisions
// in FIDL libraries. For example, the identifiers "FooBar" and "FOO_BAR"
// collide because both canonicalize to "foo_bar".
std::string Canonicalize(std::string_view identifier);

// Decodes 1 to 6 hex digits like "a" or "123" or "FFFFFF".
uint32_t DecodeUnicodeHex(std::string_view str);

// Given the source code of a string literal (including the double quotes),
// returns the length of the UTF-8 string it represents in bytes.
uint32_t StringLiteralLength(std::string_view str);

// Returns the first component of a name with 0 or more dots, e.g. "foo.bar" -> "foo".
inline std::string_view FirstComponent(std::string_view name) {
  return name.substr(0, name.find('.'));
}
// IsValidImplementationLocation validates the name of a place that could
// implement a protocol endpoint, encode a type or decode a type. It must be
// one of "platform" or "external".
bool IsValidImplementationLocation(std::string_view location);

// IsValidImplementationLocations validates a comma separated list of
// location names, as constrained by IsValidImplementationLocation.
// Items must be separated by commas but may have additional whitespace.
bool IsValidImplementationLocations(std::string_view locations);

// ParseImplementationLocations parses a string containing a comma separated
// list of implementation locations, or std::nullopt if the list is malformed.
std::optional<std::vector<std::string_view>> ParseImplementationLocations(
    std::string_view locations);

// Removes all whitespace characters from a string.
inline std::string RemoveWhitespace(std::string str) {
  str.erase(std::remove_if(str.begin(), str.end(), isspace), str.end());
  return str;
}

// Used by fidl-lint FormatFindings, and for testing, this generates the linter
// error message string in the format required for the Reporter.
void PrintFinding(std::ostream& os, const Finding& finding);

// Used by fidl-lint main() and for testing, this generates the linter error
// messages for a list of findings.
std::vector<std::string> FormatFindings(const Findings& findings, bool enable_color);

enum class ParseNumericResult : uint8_t {
  kSuccess,
  kOutOfBounds,
  kMalformed,
};

template <typename NumericType>
ParseNumericResult ParseNumeric(std::string_view input, NumericType* out_value, int base = 0) {
  ZX_ASSERT(out_value != nullptr);

  // Set locale to "C" for numeric types, since all strtox() functions are locale-dependent
  setlocale(LC_NUMERIC, "C");

  const char* startptr = input.data();
  if (base == 0 && 2 < input.size() && input[0] == '0' && (input[1] == 'b' || input[1] == 'B')) {
    startptr += 2;
    base = 2;
  }
  char* endptr;
  if constexpr (std::is_integral_v<NumericType> && std::is_unsigned_v<NumericType>) {
    if (input[0] == '-')
      return ParseNumericResult::kOutOfBounds;
    errno = 0;
    auto value = strtoull(startptr, &endptr, base);
    if (errno != 0)
      return ParseNumericResult::kMalformed;
    if (value > std::numeric_limits<NumericType>::max())
      return ParseNumericResult::kOutOfBounds;
    *out_value = static_cast<NumericType>(value);
  } else if constexpr (std::is_integral_v<NumericType> && std::is_signed_v<NumericType>) {
    errno = 0;
    auto value = strtoll(startptr, &endptr, base);
    if (errno != 0)
      return ParseNumericResult::kMalformed;
    if (value > std::numeric_limits<NumericType>::max())
      return ParseNumericResult::kOutOfBounds;
    if (value < std::numeric_limits<NumericType>::lowest())
      return ParseNumericResult::kOutOfBounds;
    *out_value = static_cast<NumericType>(value);
  } else if constexpr (std::is_floating_point_v<NumericType>) {
    errno = 0;
    long double value = strtold(startptr, &endptr);
    if (errno != 0)
      return ParseNumericResult::kMalformed;
    if (value > std::numeric_limits<NumericType>::max())
      return ParseNumericResult::kOutOfBounds;
    if (value < std::numeric_limits<NumericType>::lowest())
      return ParseNumericResult::kOutOfBounds;
    *out_value = static_cast<NumericType>(value);
  } else {
    static_assert(false, "NumericType should be unsigned, signed, or float");
  }
  if (endptr != (input.data() + input.size()))
    return ParseNumericResult::kMalformed;
  return ParseNumericResult::kSuccess;
}

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_UTILS_H_
