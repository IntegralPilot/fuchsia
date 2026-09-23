// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/utils.h"

#include <zircon/assert.h>

#include <algorithm>

#include "re2/re2.h"
#include "src/lib/fxl/strings/split_string.h"
#include "tools/fidl/fidlc/src/reporter.h"

namespace fidlc {

const std::string kLibraryComponentPattern = "[a-z][a-z0-9]*";
const std::string kIdentifierComponentPattern = "[A-Za-z]([A-Za-z0-9_]*[A-Za-z0-9])?";
const std::string kLibraryPattern =
    kLibraryComponentPattern + "(\\." + kLibraryComponentPattern + ")*";

bool IsValidLibraryComponent(std::string_view component) {
  static const re2::RE2 kPattern("^" + kLibraryComponentPattern + "$");
  return re2::RE2::FullMatch(component, kPattern);
}

bool IsValidIdentifierComponent(std::string_view component) {
  static const re2::RE2 kPattern("^" + kIdentifierComponentPattern + "$");
  return re2::RE2::FullMatch(component, kPattern);
}

bool IsValidFullyQualifiedMethodIdentifier(std::string_view fq_identifier) {
  static const re2::RE2 kPattern("^" + kLibraryPattern + "/" +
                                 /* protocol */ kIdentifierComponentPattern + "\\." +
                                 /* method */ kIdentifierComponentPattern + "$");
  return re2::RE2::FullMatch(fq_identifier, kPattern);
}

bool IsValidDiscoverableName(std::string_view discoverable_name) {
  static const re2::RE2 kPattern("^" + kLibraryPattern + "\\." + kIdentifierComponentPattern + "$");
  return re2::RE2::FullMatch(discoverable_name, kPattern);
}

bool IsValidImplementationLocation(std::string_view location) {
  return location == "platform" || location == "external";
}

bool IsValidImplementationLocations(std::string_view locations) {
  return ParseImplementationLocations(locations).has_value();
}

std::optional<std::vector<std::string_view>> ParseImplementationLocations(
    std::string_view locations) {
  auto parts = fxl::SplitString(locations, ",", fxl::kTrimWhitespace, fxl::kSplitWantAll);
  if (std::all_of(parts.begin(), parts.end(), IsValidImplementationLocation)) {
    return parts;
  }
  return std::nullopt;
}

std::string StripStringLiteralQuotes(std::string_view str) {
  ZX_ASSERT_MSG(str.size() >= 2 && str[0] == '"' && str[str.size() - 1] == '"',
                "string must start and end with '\"' style quotes");
  return std::string(str.data() + 1, str.size() - 2);
}

std::string StripDocCommentSlashes(std::string_view str) {
  std::string no_slashes(str);
  re2::RE2::GlobalReplace(&no_slashes, "([\\t ]*\\/\\/\\/)(.*)", "\\2");
  if (no_slashes[no_slashes.size() - 1] != '\n') {
    return no_slashes + '\n';
  }
  return no_slashes;
}

static bool HasConstantK(std::string_view str) {
  return str.size() >= 2 && str[0] == 'k' && isupper(str[1]);
}

static std::string StripConstantK(std::string_view str) {
  return std::string(HasConstantK(str) ? str.substr(1) : str);
}

bool IsLowerSnakeCase(std::string_view str) {
  static re2::RE2 re{"^[a-z][a-z0-9_]*$"};
  return !str.empty() && re2::RE2::FullMatch(str, re);
}

bool IsUpperSnakeCase(std::string_view str) {
  static re2::RE2 re{"^[A-Z][A-Z0-9_]*$"};
  return !str.empty() && re2::RE2::FullMatch(str, re);
}

bool IsLowerCamelCase(std::string_view str) {
  if (HasConstantK(str)) {
    return false;
  }
  static re2::RE2 re{"^[a-z][a-z0-9]*(([A-Z]{1,2}[a-z0-9]+)|(_[0-9]+))*([A-Z][a-z0-9]*)?$"};
  return !str.empty() && re2::RE2::FullMatch(str, re);
}

bool IsUpperCamelCase(std::string_view str) {
  static re2::RE2 re{
      "^(([A-Z]{1,2}[a-z0-9]+)(([A-Z]{1,2}[a-z0-9]+)|(_[0-9]+))*)?([A-Z][a-z0-9]*)?$"};
  return !str.empty() && re2::RE2::FullMatch(str, re);
}

std::vector<std::string> SplitIdentifierWords(std::string_view astr) {
  std::string str = StripConstantK(astr);
  std::vector<std::string> words;
  std::string word;
  bool last_char_was_upper_or_begin = true;
  for (size_t i = 0; i < str.size(); i++) {
    char ch = str[i];
    if (ch == '_' || ch == '-' || ch == '.') {
      if (!word.empty()) {
        words.push_back(word);
        word.clear();
      }
      last_char_was_upper_or_begin = true;
    } else {
      bool next_char_is_lower = ((i + 1) < str.size()) && islower(str[i + 1]);
      if (isupper(ch) && (!last_char_was_upper_or_begin || next_char_is_lower)) {
        if (!word.empty()) {
          words.push_back(word);
          word.clear();
        }
      }
      word.push_back(static_cast<char>(tolower(ch)));
      last_char_was_upper_or_begin = isupper(ch);
    }
  }
  if (!word.empty()) {
    words.push_back(word);
  }
  return words;
}

std::string ToLowerSnakeCase(std::string_view astr) {
  std::string str = StripConstantK(astr);
  std::string newid;
  for (const auto& word : SplitIdentifierWords(str)) {
    if (!newid.empty()) {
      newid.push_back('_');
    }
    newid.append(word);
  }
  return newid;
}

std::string ToUpperSnakeCase(std::string_view astr) {
  std::string str = StripConstantK(astr);
  auto newid = ToLowerSnakeCase(str);
  std::transform(newid.begin(), newid.end(), newid.begin(), ::toupper);
  return newid;
}

std::string ToLowerCamelCase(std::string_view astr) {
  std::string str = StripConstantK(astr);
  bool prev_char_was_digit = false;
  std::string newid;
  for (const auto& word : SplitIdentifierWords(str)) {
    if (newid.empty()) {
      newid.append(word);
    } else {
      if (prev_char_was_digit && isdigit(word[0])) {
        newid.push_back('_');
      }
      newid.push_back(static_cast<char>(toupper(word[0])));
      newid.append(word.substr(1));
    }
    prev_char_was_digit = isdigit(word.back());
  }
  return newid;
}

std::string ToUpperCamelCase(std::string_view astr) {
  std::string str = StripConstantK(astr);
  bool prev_char_was_digit = false;
  std::string newid;
  for (const auto& word : SplitIdentifierWords(str)) {
    if (prev_char_was_digit && isdigit(word[0])) {
      newid.push_back('_');
    }
    newid.push_back(static_cast<char>(toupper(word[0])));
    newid.append(word.substr(1));
    prev_char_was_digit = isdigit(word.back());
  }
  return newid;
}

std::string Canonicalize(std::string_view identifier) {
  const auto size = identifier.size();
  std::string canonical;
  char prev = '_';
  for (size_t i = 0; i < size; i++) {
    const char c = identifier[i];
    if (c == '_') {
      if (prev != '_') {
        canonical.push_back('_');
      }
    } else if (((islower(prev) || isdigit(prev)) && isupper(c)) ||
               (prev != '_' && isupper(c) && i + 1 < size && islower(identifier[i + 1]))) {
      canonical.push_back('_');
      canonical.push_back(static_cast<char>(tolower(c)));
    } else {
      canonical.push_back(static_cast<char>(tolower(c)));
    }
    prev = c;
  }
  return canonical;
}

uint32_t DecodeUnicodeHex(std::string_view str) {
  char* endptr;
  unsigned long codepoint = strtoul(str.data(), &endptr, 16);
  ZX_ASSERT(codepoint != ULONG_MAX);
  ZX_ASSERT(endptr == &(*str.end()));
  return codepoint;
}

static size_t Utf8SizeForCodepoint(uint32_t codepoint) {
  if (codepoint <= 0x7f) {
    return 1;
  }
  if (codepoint <= 0x7ff) {
    return 2;
  }
  if (codepoint <= 0x10000) {
    return 3;
  }
  ZX_ASSERT(codepoint <= 0x10ffff);
  return 4;
}

uint32_t StringLiteralLength(std::string_view str) {
  uint32_t count = 0;
  auto it = str.begin();
  ZX_ASSERT(*it == '"');
  ++it;
  const auto closing_quote = str.end() - 1;
  for (; it < closing_quote; ++it) {
    ++count;
    if (*it == '\\') {
      ++it;
      ZX_ASSERT(it < closing_quote);
      switch (*it) {
        case '\\':
        case '"':
        case 'n':
        case 'r':
        case 't':
          break;
        case 'u': {
          ++it;
          ZX_ASSERT(*it == '{');
          ++it;
          auto codepoint_begin = it;
          while (*it != '}') {
            ++it;
          }
          auto codepoint =
              DecodeUnicodeHex(std::string_view(&(*codepoint_begin), it - codepoint_begin));
          count += Utf8SizeForCodepoint(codepoint) - 1;
          break;
        }
        default:
          ZX_PANIC("invalid string literal");
      }
      ZX_ASSERT(it < closing_quote);
    }
  }
  ZX_ASSERT(*it == '"');
  return count;
}

void PrintFinding(std::ostream& os, const Finding& finding) {
  os << finding.message() << " [";
  os << finding.subcategory();
  os << ']';
  if (finding.suggestion().has_value()) {
    auto& suggestion = finding.suggestion();
    os << "; " << suggestion->description();
    if (suggestion->replacement().has_value()) {
      os << "\n    Proposed replacement:  '" << *suggestion->replacement() << "'";
    }
  }
}

std::vector<std::string> FormatFindings(const Findings& findings, bool enable_color) {
  std::vector<std::string> lint;
  for (auto& finding : findings) {
    std::stringstream ss;
    PrintFinding(ss, finding);
    auto warning = Reporter::Format("warning", finding.span(), ss.str(), enable_color);
    lint.push_back(warning);
  }
  return lint;
}

}  // namespace fidlc
