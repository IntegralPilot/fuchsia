// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_UTILS_REDACT_REPLACER_H_
#define SRC_DEVELOPER_FORENSICS_UTILS_REDACT_REPLACER_H_

#include <lib/fit/function.h>

#include <string>
#include <string_view>
#include <vector>

#include "src/developer/forensics/utils/redact/cache.h"

namespace forensics {

// A Replacer is an invocable object that replaces substrings in |text| and returns |text|.
using Replacer = ::fit::function<std::string&(RedactionIdCache&, std::string& text)>;

// Constructs a Replacer that substitutes all instances of |pattern| with |replacement|.
Replacer ReplaceWithText(std::string_view pattern, std::string_view replacement);

// Constructs a Replacer that substitutes all instances of |pattern| with |format| and the id for
// the matched pattern. However, text will not be replaced if any strings from |ignore_prefixes|
// occur just _before_ the matching pattern begins, regardless of whether the found prefix starts on
// a word boundary. See examples 1 and 2.
//
// If a match happens to start with a string X from |ignore_prefixes|, the match will still be
// redacted unless that matching text also has an instance of X _before_ the matching text begins.
// See examples 3 and 4.
//
// Some examples:
//
//   1)
//   pattern: "id:a+"
//   ignore_prefix: "elf:"
//   replace: "<REDACTED>"
//   content: "other_stuff:id:aaa"
//   result: "other_stuff:<REDACTED>"
//
//   2)
//   pattern: "id:a+"
//   ignore_prefix: "elf:"
//   replace: "<REDACTED>"
//   content: "elf:id:aaa"
//   result: "elf:id:aaa"
//
//   3)
//   pattern: "elf:a+"
//   ignore_prefix: "elf:"
//   replace: "<REDACTED>"
//   content: "other_stuff:elf:aaa"
//   result: "other_stuff:<REDACTED>"
//
//   4)
//   pattern: "elf:a+"
//   ignore_prefix: "elf:"
//   replace: "<REDACTED>"
//   content: "elf:elf:aaa"
//   result: "elf:elf:aaa"
//
// Note: |pattern| must extract at least 1 value.
// Note: |format| must contain exactly 1 integer format specifier, i.e. '%d'.
Replacer ReplaceWithIdFormatString(std::string_view pattern, std::string_view format,
                                   const std::vector<std::string>& ignore_prefixes);

// Constructs a Replacer that substitutes all instances of SENSITIVE{...} decorators with
// "<REDACTED-SENSITIVE>".
Replacer ReplaceSensitive();

// Constructs a Replacer that substitutes all instances IPv4 address with "<REDACTED-IPV4: %d>"
Replacer ReplaceIPv4();

// Constructs a Replacer that substitutes all instances of IPv4 addresses from FIDL debug output
// with "<REDACTED-IPV4: %d>"
Replacer ReplaceFidlIPv4();

// Constructs a Replacer that substitutes all instances IPv6 address with some variation of
// "<REDACTED-IPV6: %d>"
Replacer ReplaceIPv6();

// Constructs a Replacer that substitutes all instances of IPv6 addresses from FIDL debug output
// with "<REDACTED-IPV6: %d>"
Replacer ReplaceFidlIPv6();

// Constructs a Replacer that substitutes all instances of MAC address with a string like
// "<REDACTED-MAC: %d>"
Replacer ReplaceMac();

// Constructs a Replacer that substitutes all instances of MAC addresses from FIDL debug output
// with "<REDACTED-MAC: %d>"
Replacer ReplaceFidlMac();

// Constructs a Replacer that substitutes all instances of SSIDs with a string like
// "<REDACTED-SSID: %d>"
Replacer ReplaceSsid();

namespace mac_utils {

// Returns a sub-string view of the first three bytes of |mac|, including the delimiters
// that follow each byte.
//
// This function assumes |mac| is a proper representation of a MAC address. Undefined
// behavior will result if passed any other string.
std::string GetOuiPrefix(const std::string& mac);

// Constructs a MAC address equivalent to |mac| but in the canonical form with
// exactly two digits per bytes and colons as delimiters.
//
// This function assumes |mac| is a proper representation of a MAC address. Undefined
// behavior will result if passed any other string.
std::string CanonicalizeMac(const std::string& original_mac);

}  // namespace mac_utils

}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_UTILS_REDACT_REPLACER_H_
