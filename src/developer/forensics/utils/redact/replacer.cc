// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/utils/redact/replacer.h"

#include <lib/syslog/cpp/macros.h>

#include <algorithm>
#include <memory>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

#include <re2/re2.h>

#include "absl/strings/string_view.h"
#include "src/developer/forensics/utils/regexp.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace forensics {

Replacer ReplaceWithText(const std::string_view pattern, const std::string_view replacement) {
  auto regexp = std::make_unique<re2::RE2>(pattern);
  if (!regexp->ok()) {
    FX_LOGS(ERROR) << "Failed to compile regexp: \"" << pattern << "\"";
    return nullptr;
  }

  return [regexp = std::move(regexp), replace = std::string(replacement)](
             RedactionIdCache& cache, std::string& text) -> std::string& {
    RE2::GlobalReplace(&text, *regexp, replace);
    return text;
  };
}

namespace {

struct Redaction {
  int64_t original_position;
  size_t match_size;
  int64_t offset;
  std::string replacement;
};

// Given |redactions|, replaces the text at position |original_position| with |replacement|. As
// replacements are made, this function accounts for the position of characters shifting due to the
// difference in size between the original text and the replacement text.
//
// |redactions| is assumed to be sorted in order of |original_position|. The matches are assumed to
// not overlap.
void ApplyRedactions(const std::vector<Redaction>& redactions, std::string& text) {
  size_t running_offset = 0;
  for (const Redaction& redaction : redactions) {
    text.replace(redaction.original_position + running_offset, redaction.match_size,
                 redaction.replacement);

    running_offset += redaction.offset;
  }
}

// Finds strings in |text| that match |regexp| and constructs their redacted replacements with
// |build_redacted|.
std::vector<Redaction> BuildRedactions(
    const std::string& text, const re2::RE2& regexp,
    const std::vector<std::string>& ignore_prefixes,
    ::fit::function<std::string(const std::string& match)> build_redacted) {
  std::vector<Redaction> redactions;
  absl::string_view text_view(text);
  absl::string_view match;

  while (RE2::FindAndConsume(&text_view, regexp, &match)) {
    const bool has_prefix =
        std::any_of(ignore_prefixes.begin(), ignore_prefixes.end(),
                    [&text, &match](const std::string_view ignore_prefix) {
                      const char* prefix_start = match.data() - ignore_prefix.size();

                      // Don't access memory before the buffer that |text| owns.
                      return prefix_start >= text.data() &&
                             ignore_prefix == std::string_view(prefix_start, ignore_prefix.size());
                    });

    if (!match.empty() && !has_prefix) {
      const std::string replacement = build_redacted(std::string(match));
      redactions.push_back(Redaction{
          // We're working with pointers, but want a relative position within |text| so we need to
          // subtract the original start pointer.
          .original_position = match.data() - text.data(),
          .match_size = match.size(),
          .offset = static_cast<int64_t>(replacement.size()) - static_cast<int64_t>(match.size()),
          .replacement = replacement,
      });
    }
  }

  return redactions;
}

// Builds a Replacer that redacts instances of |pattern| with strings constructed by
// |build_redacted|. Does NOT replace if any strings from |ignore_prefixes| occur just before the
// matching string begins.
//
// Returns nullptr if pattern produces a bad regexp.
Replacer FunctionBasedReplacer(
    const std::string_view pattern, const std::vector<std::string>& ignore_prefixes,
    ::fit::function<std::string(RedactionIdCache& cache, const std::string& match)>
        build_redacted) {
  auto regexp = std::make_unique<re2::RE2>(pattern);
  if (!regexp->ok()) {
    FX_LOGS(ERROR) << "Failed to compile regexp: \"" << pattern << "\"";
    return nullptr;
  }

  if (regexp->NumberOfCapturingGroups() < 1) {
    FX_LOGS(ERROR) << "Regexp \"" << pattern
                   << "\" expected to have at least 1 capturing group, has "
                   << regexp->NumberOfCapturingGroups();
    return nullptr;
  }

  return [regexp = std::move(regexp), build_redacted = std::move(build_redacted), ignore_prefixes](
             RedactionIdCache& cache, std::string& text) mutable -> std::string& {
    const std::vector<Redaction> redactions = BuildRedactions(
        text, *regexp, ignore_prefixes, [&cache, &build_redacted](const std::string& match) {
          return build_redacted(cache, match);
        });
    ApplyRedactions(redactions, text);
    return text;
  };
}

}  // namespace

Replacer ReplaceWithIdFormatString(const std::string_view pattern,
                                   const std::string_view format_str,
                                   const std::vector<std::string>& ignore_prefixes) {
  bool specificier_found{false};

  for (size_t pos{0}; (pos = format_str.find("%d", pos)) != std::string::npos; ++pos) {
    if (specificier_found) {
      FX_LOGS(ERROR) << "Format string \"" << format_str
                     << "\" expected to have 1 \"%d\" specifier";
      return nullptr;
    }
    specificier_found = true;
  }

  if (!specificier_found) {
    FX_LOGS(ERROR) << "Format string \"" << format_str << "\" expected to have 1 \"%d\" specifier";
    return nullptr;
  }

  return FunctionBasedReplacer(
      pattern, ignore_prefixes,
      [format = std::string(format_str)](RedactionIdCache& cache, const std::string& match) {
        return fxl::StringPrintf(format.c_str(), cache.GetId(match));
      });
}

namespace {

// Finds and redacts all instances of `SENSITIVE{...}`, ignoring braces inside string literals.
std::string RedactSensitive(const std::string_view text) {
  static constexpr std::string_view kPrefix = "SENSITIVE{";
  static constexpr std::string_view kReplacement = "<REDACTED-SENSITIVE>";

  std::string result;
  result.reserve(text.size());

  size_t curr_begin = 0;
  while (curr_begin < text.size()) {
    size_t pos = text.find(kPrefix, curr_begin);
    if (pos == std::string_view::npos) {
      result.append(text.substr(curr_begin));
      break;
    }

    result.append(text.substr(curr_begin, pos - curr_begin));

    size_t depth = 1;
    size_t curr_end = pos + kPrefix.size();
    bool in_string = false;
    bool escape_next = false;

    while (curr_end < text.size() && depth > 0) {
      char c = text[curr_end];
      if (in_string) {
        if (escape_next) {
          escape_next = false;
        } else if (c == '\\') {
          escape_next = true;
        } else if (c == '"') {
          in_string = false;
        }
      } else {
        if (c == '"') {
          in_string = true;
        } else if (c == '{') {
          depth++;
        } else if (c == '}') {
          depth--;
        }
      }
      curr_end++;
    }

    result.append(kReplacement);
    curr_begin = curr_end;
  }

  return result;
}

}  // namespace

Replacer ReplaceSensitive() {
  return [](RedactionIdCache& /*cache*/, std::string& text) -> std::string& {
    if (text.find("SENSITIVE{") == std::string::npos) {
      return text;
    }
    text = RedactSensitive(text);
    return text;
  };
}

namespace {

constexpr std::string_view kIPv4Pattern{R"(\b()"
                                        R"((?:(?:25[0-5]|(?:2[0-4]|1{0,1}[0-9]){0,1}[0-9])\.){3,3})"
                                        R"((?:25[0-5]|(?:2[0-4]|1{0,1}[0-9]){0,1}[0-9]|[a-zA-Z]+))"
                                        R"()\b)"};

// 0.*.*.* = current network (as source)
// 127.*.*.* = loopback
// 169.254.*.* = link-local addresses
// 224.0.0.* = link-local multicast
constexpr re2::LazyRE2 kCleartextIPv4 = MakeLazyRE2(R"(^0\..*)"
                                                    R"(|)"
                                                    R"(^127\..*)"
                                                    R"(|)"
                                                    R"(^169\.254\..*)"
                                                    R"(|)"
                                                    R"(^224\.0\.0\..*)"
                                                    R"(|)"
                                                    R"(^255.255.255.255$)");

std::string RedactIPv4(RedactionIdCache& cache, const std::string& match) {
  return re2::RE2::FullMatch(match, *kCleartextIPv4)
             ? match
             : fxl::StringPrintf("<REDACTED-IPV4: %d>", cache.GetId(match));
}

}  // namespace

Replacer ReplaceIPv4() {
  return FunctionBasedReplacer(kIPv4Pattern, /*ignore_prefixes=*/{}, RedactIPv4);
}

namespace {

// Matches a string like "Ipv4Address { addr: [1, 2, 3, 4] }". The two inner capture groups are used
// to replace just the address within the match.
constexpr re2::LazyRE2 kFidlIpv4 =
    MakeLazyRE2(R"(((Ipv4Address { )addr: \[(?:[0-9a-fA-F]{1,3}, ){3}[0-9a-fA-F]{1,3}]( })))");

std::string RedactFidlIPv4(RedactionIdCache& cache, const std::string& match) {
  std::string content = match;
  const std::string replacement =
      fxl::StringPrintf("\\2<REDACTED-IPV4: %d>\\3", cache.GetId(match));
  RE2::GlobalReplace(&content, *kFidlIpv4, replacement);
  return content;
}

}  // namespace

Replacer ReplaceFidlIPv4() {
  return FunctionBasedReplacer(kFidlIpv4->pattern(), /*ignore_prefixes=*/{}, RedactFidlIPv4);
}

namespace {

constexpr std::string_view kIPv6Pattern{
    // IPv6 without ::
    R"(()"
    R"(\b(?:(?:[[:xdigit:]]{1,4}:){7}[[:xdigit:]]{1,4})\b)"
    R"(|)"
    //  IPv6 with embedded ::
    R"(\b(?:(?:[[:xdigit:]]{1,4}:)+:(?:[[:xdigit:]]{1,4}:)*[[:xdigit:]]{1,4})\b)"
    R"(|)"
    // IPv6 starting with :: and 3-7 non-zero fields
    R"(::[[:xdigit:]]{1,4}(?::[[:xdigit:]]{1,4}){2,6}\b)"
    R"(|)"
    // IPv6 with 3-7 non-zero fields ending with ::
    R"(\b[[:xdigit:]]{1,4}(?::[[:xdigit:]]{1,4}){2,6}::)"
    R"())"};

// ff.1:** and ff.2:** = local multicast
constexpr re2::LazyRE2 kCleartextIPv6 = MakeLazyRE2(R"((?i)^ff[[:xdigit:]][12]:)");

// ff..:** = multicast - display first 2 bytes and redact
constexpr re2::LazyRE2 kMulticastIPv6 = MakeLazyRE2(R"((?i)^(ff[[:xdigit:]][[:xdigit:]]:))");

// fe80/10 = link-local - display first 2 bytes and redact
constexpr re2::LazyRE2 kLinkLocalIPv6 = MakeLazyRE2(R"((?i)^(fe[89ab][[:xdigit:]]:))");

// ::ffff:*:* = IPv4
constexpr re2::LazyRE2 kIPv4InIPv6 = MakeLazyRE2(R"((?i)^::f{4}(:[[:xdigit:]]{1,4}){2}$)");

std::string RedactIPv6(RedactionIdCache& cache, const std::string& match) {
  if (re2::RE2::PartialMatch(match, *kCleartextIPv6)) {
    return match;
  }

  const int id = cache.GetId(match);

  std::string submatch;
  if (re2::RE2::PartialMatch(match, *kMulticastIPv6, &submatch)) {
    return fxl::StringPrintf("%s<REDACTED-IPV6-MULTI: %d>", submatch.c_str(), id);
  }

  if (re2::RE2::PartialMatch(match, *kLinkLocalIPv6, &submatch)) {
    return fxl::StringPrintf("%s<REDACTED-IPV6-LL: %d>", submatch.c_str(), id);
  }

  if (re2::RE2::FullMatch(match, *kIPv4InIPv6)) {
    return fxl::StringPrintf("::ffff:<REDACTED-IPV4: %d>", id);
  }

  return fxl::StringPrintf("<REDACTED-IPV6: %d>", id);
}

}  // namespace

Replacer ReplaceIPv6() {
  return FunctionBasedReplacer(kIPv6Pattern, /*ignore_prefixes=*/{}, RedactIPv6);
}

namespace {

// Matches a string like
// "Ipv6Address { addr: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16] }". The two inner
// capture groups are used to replace just the address within the match.
constexpr re2::LazyRE2 kFidlIpv6 =
    MakeLazyRE2(R"(((Ipv6Address { )addr: \[(?:[0-9a-fA-F]{1,3}, ){15}[0-9a-fA-F]{1,3}]( })))");

std::string RedactFidlIPv6(RedactionIdCache& cache, const std::string& match) {
  std::string content = match;
  const std::string replacement =
      fxl::StringPrintf("\\2<REDACTED-IPV6: %d>\\3", cache.GetId(match));
  RE2::GlobalReplace(&content, *kFidlIpv6, replacement);
  return content;
}

}  // namespace

Replacer ReplaceFidlIPv6() {
  return FunctionBasedReplacer(kFidlIpv6->pattern(), /*ignore_prefixes=*/{}, RedactFidlIPv6);
}

namespace mac_utils {

const size_t NUM_MAC_BYTES = 6;

static constexpr std::string_view kMacPattern{
    R"(\b()"
    R"(\b((?:[[:xdigit:]]{1,2}(?:[\.:-])){3})(?:[[:xdigit:]]{1,2}(?:[\.:-])){2}[[:xdigit:]]{1,2}\b)"
    R"()\b)"};

std::string GetOuiPrefix(const std::string& mac) {
  static constexpr re2::LazyRE2 regexp = MakeLazyRE2(kMacPattern.data());
  std::string oui;
  re2::RE2::FullMatch(mac, *regexp, nullptr, &oui);
  return oui;
}

std::string CanonicalizeMac(const std::string& original_mac) {
  std::string lowercased_mac(original_mac);
  std::transform(lowercased_mac.begin(), lowercased_mac.end(), lowercased_mac.begin(),
                 [](char c) { return std::tolower(c); });

  std::string canonical_mac = "00:00:00:00:00:00";
  absl::string_view lowercased_mac_view(lowercased_mac);
  for (size_t i = 0; i < NUM_MAC_BYTES; ++i) {
    absl::string_view mac_byte;
    re2::RE2::FindAndConsume(&lowercased_mac_view, R"(([[:xdigit:]]{1,2}))", &mac_byte);

    if (mac_byte.length() == 2) {
      canonical_mac.replace(3 * i, 2, mac_byte.data(), 2);
    } else if (mac_byte.length() == 1) {
      canonical_mac.replace(3 * i + 1, 1, mac_byte.data(), 1);
    } else {
      // The regular expression used in |FindAndConsume()| above ensure |mac_byte|
      // will have either 1 or 2 characters.
      __builtin_unreachable();
    }
  }

  return canonical_mac;
}

}  // namespace mac_utils

namespace {

std::string RedactMac(RedactionIdCache& cache, const std::string& mac) {
  const std::string oui = mac_utils::GetOuiPrefix(mac);
  const int id = cache.GetId(mac_utils::CanonicalizeMac(mac));
  return fxl::StringPrintf("%s<REDACTED-MAC: %d>", oui.c_str(), id);
}

}  // namespace

Replacer ReplaceMac() {
  return FunctionBasedReplacer(mac_utils::kMacPattern, /*ignore_prefixes=*/{}, RedactMac);
}

namespace {

// Matches a string like "MacAddress { octets: [1, 2, 3, 4, 5, 6] }". The two inner capture groups
// are used to replace just the address within the match.
constexpr re2::LazyRE2 kFidlMac =
    MakeLazyRE2(R"(((MacAddress { )octets: \[(?:[0-9a-fA-F]{1,3}, ){5}[0-9a-fA-F]{1,3}]( })))");

std::string RedactFidlMac(RedactionIdCache& cache, const std::string& match) {
  std::string content = match;
  const std::string replacement = fxl::StringPrintf("\\2<REDACTED-MAC: %d>\\3", cache.GetId(match));
  RE2::GlobalReplace(&content, *kFidlMac, replacement);
  return content;
}

}  // namespace

Replacer ReplaceFidlMac() {
  return FunctionBasedReplacer(kFidlMac->pattern(), /*ignore_prefixes=*/{}, RedactFidlMac);
}

namespace {

// The SSID identifier contains at most 32 pairs of hexadecimal characters, but match any number so
// SSID identifiers with the wrong number of hexadecimal characters are also redacted.
constexpr std::string_view kSsidPattern = R"((<ssid-[[:xdigit:]]*>))";

std::string RedactSsid(RedactionIdCache& cache, const std::string& match) {
  const int id = cache.GetId(match);
  return fxl::StringPrintf("<REDACTED-SSID: %d>", id);
}

}  // namespace

Replacer ReplaceSsid() {
  return FunctionBasedReplacer(kSsidPattern, /*ignore_prefixes=*/{}, RedactSsid);
}

}  // namespace forensics
