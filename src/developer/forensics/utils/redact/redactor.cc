// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/utils/redact/redactor.h"

#include <lib/inspect/cpp/hierarchy.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/syslog/cpp/macros.h>

#include <string>
#include <string_view>
#include <vector>

namespace forensics {
namespace {

// Email stub alice@website.tld
constexpr std::string_view kEmailPattern = R"([a-zA-Z0-9]*@[a-zA-Z0-9]*\.[a-zA-Z]*)";

// uuid
constexpr std::string_view kUuidPattern =
    R"([0-9a-fA-F]{8}\b-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-\b[0-9a-fA-F]{12})";

// http(s) urls
constexpr std::string_view kUrlPattern = R"(https?://[^"',!<> ]*)";

// Hex strings
constexpr std::string_view k16HexPattern = R"((\b[0-9a-fA-F]{16}\b))";
constexpr std::string_view k32HexPattern = R"((\b[0-9a-fA-F]{32}\b))";
const auto* kHexIgnorePrefixes = new std::vector<std::string>({"elf:", "build_id: '"});

// Obfuscated gaia ids
constexpr std::string_view kGaiaPattern = R"((\b1[0-9]{20}\b))";

// Serial numbers
constexpr std::string_view kSerialPattern =
    R"((?i)\bserial(?:[ _]number)?\s*[:=]\s*['"]?[0-9a-zA-Z_-]+['"]?)";

constexpr std::string_view kUnredactedCanary =
    R"(Email: alice@website.tld,
IPv4: 8.8.8.8,
IPv4_New: 8.9.10.42,
IPv4_Dup: 8.8.8.8,
IPv4_WithPort: 8.8.8.8:8080,
IPv4_Fidl: Ipv4Address { addr: [1, 255, FF, FF] }
IPv461: ::ffff:12.34.56.78,
IPv462: ::ffff:ab12:cd34,
IPv6: 2001:503:eEa3:0:0:0:0:30,
IPv6_WithPort: [2001:503:eEa3:0:0:0:0:30]:8080,
IPv6_Fidl: Ipv6Address { addr: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 255, FF, FF] }
IPv6C: fec8::7d84:c1dc:ab34:656a,
IPv6LL: fe80::7d84:c1dc:ab34:656a,
UUID: ddd0fA34-1016-11eb-adc1-0242ac120002,
MAC: de:ad:BE:EF:42:5a,
MAC_dashes: de-ad-BE-EF-42-5a,
MAC_dots: de.ad.BE.EF.42.5a,
MAC_mixed: de.ad-BE:EF.42-5a,
MAC_Fidl: MacAddress { octets: [1, 2, 3, 255, FF, FF] }
SSID: <ssid-666F6F>,
HTTP: http://fuchsia.dev/fuchsia/testing?q=Test,
HTTPS: https://fuchsia.dev/fuchsia/testing?q=Test,
URL_IPv4: https://8.8.8.8/fuchsia/testing?q=Test,
HEX: 1234567890abcdef,
HEX: 1234567890abcdefABCDEF0123456789,
v4Current: 0.1.2.3,
v4Loopback: 127.1.2.3,
v4LocalAddr: 169.254.12.34,
v4LocalMulti: 224.0.0.123,
v4Multi: 224.0.1.123,
broadcast: 255.255.255.255,
v6zeroes: :: ::1,
v6LeadingZeroes: ::abcd:dcba:bcde:f,
v6TrailingZeroes: f:e:d:c:abcd:dcba:bcde::,
v6LinkLocal: feB2:111:222:333:444:555:666:777,
v6LocalMulticast: ff72:111:222:333:444:555:666:777,
v6Multicast: ff77:111:222:333:444:555:666:777,
obfuscatedGaiaId: 106986199446298680449,
Sensitive: SENSITIVE{sensitive_data},
Serial: 12345ABC,
serial=3125592196)";

constexpr std::string_view kRedactedCanary =
    R"(Email: <REDACTED-EMAIL>,
IPv4: <REDACTED-IPV4: 1>,
IPv4_New: <REDACTED-IPV4: 2>,
IPv4_Dup: <REDACTED-IPV4: 1>,
IPv4_WithPort: <REDACTED-IPV4: 1>:8080,
IPv4_Fidl: Ipv4Address { <REDACTED-IPV4: 5> }
IPv461: ::ffff:<REDACTED-IPV4: 3>,
IPv462: ::ffff:<REDACTED-IPV4: 6>,
IPv6: <REDACTED-IPV6: 7>,
IPv6_WithPort: [<REDACTED-IPV6: 7>]:8080,
IPv6_Fidl: Ipv6Address { <REDACTED-IPV6: 14> }
IPv6C: <REDACTED-IPV6: 8>,
IPv6LL: fe80:<REDACTED-IPV6-LL: 9>,
UUID: <REDACTED-UUID>,
MAC: de:ad:BE:<REDACTED-MAC: 15>,
MAC_dashes: de-ad-BE-<REDACTED-MAC: 15>,
MAC_dots: de.ad.BE.<REDACTED-MAC: 15>,
MAC_mixed: de.ad-BE:<REDACTED-MAC: 15>,
MAC_Fidl: MacAddress { <REDACTED-MAC: 16> }
SSID: <REDACTED-SSID: 17>,
HTTP: <REDACTED-URL>,
HTTPS: <REDACTED-URL>,
URL_IPv4: <REDACTED-URL>,
HEX: <REDACTED-HEX: 18>,
HEX: <REDACTED-HEX: 19>,
v4Current: 0.1.2.3,
v4Loopback: 127.1.2.3,
v4LocalAddr: 169.254.12.34,
v4LocalMulti: 224.0.0.123,
v4Multi: <REDACTED-IPV4: 4>,
broadcast: 255.255.255.255,
v6zeroes: :: ::1,
v6LeadingZeroes: <REDACTED-IPV6: 10>,
v6TrailingZeroes: <REDACTED-IPV6: 11>,
v6LinkLocal: feB2:<REDACTED-IPV6-LL: 12>,
v6LocalMulticast: ff72:111:222:333:444:555:666:777,
v6Multicast: ff77:<REDACTED-IPV6-MULTI: 13>,
obfuscatedGaiaId: <REDACTED-OBFUSCATED-GAIA-ID: 20>,
Sensitive: <REDACTED-SENSITIVE>,
<REDACTED-SERIAL>,
<REDACTED-SERIAL>)";

}  // namespace

RedactorBase::RedactorBase(inspect::BoolProperty redaction_enabled)
    : redaction_enabled_(std::move(redaction_enabled)) {}

Redactor::Redactor(const int starting_id, inspect::UintProperty cache_size,
                   inspect::BoolProperty redaction_enabled)
    : RedactorBase(std::move(redaction_enabled)), cache_(std::move(cache_size), starting_id) {
  // The URL replacer must run before IP/MAC/SSID replacers. The IP replacers
  // inject '<' and space characters (e.g. "<REDACTED-IPV4: 1>") which fall in
  // the URL regex stop-character set [^"',!<> ]. If IP redaction runs first on
  // a URL with an IP-literal host, the URL regex terminates early and the
  // path/query (which may contain secrets) survives redaction.
  AddTextReplacer(kUrlPattern, "<REDACTED-URL>")
      .Add(ReplaceSensitive())
      .Add(ReplaceIPv4())
      .Add(ReplaceFidlIPv4())
      .Add(ReplaceIPv6())
      .Add(ReplaceFidlIPv6())
      .Add(ReplaceMac())
      .Add(ReplaceFidlMac())
      .Add(ReplaceSsid())
      .AddJsonReplacer(ReplaceSensitive())
      .AddJsonReplacer(ReplaceIPv4())
      .AddJsonReplacer(ReplaceIPv6())
      .AddJsonReplacer(ReplaceMac())
      .AddJsonReplacer(ReplaceSsid())
      .AddTextReplacer(kEmailPattern, "<REDACTED-EMAIL>")
      .AddTextReplacer(kUuidPattern, "<REDACTED-UUID>")
      .AddTextReplacer(kSerialPattern, "<REDACTED-SERIAL>")
      .AddIdReplacer(k16HexPattern, "<REDACTED-HEX: %d>", *kHexIgnorePrefixes)
      .AddIdReplacer(k32HexPattern, "<REDACTED-HEX: %d>", *kHexIgnorePrefixes)
      .AddIdReplacer(kGaiaPattern, "<REDACTED-OBFUSCATED-GAIA-ID: %d>", /*ignore_prefixes=*/{});
}

std::string& Redactor::Redact(std::string& text) {
  for (const Replacer& replacer : replacers_) {
    replacer(cache_, text);
  }
  return text;
}

std::string& Redactor::RedactJson(std::string& text) {
  for (const Replacer& replacer : json_replacers_) {
    replacer(cache_, text);
  }
  return text;
}

Redactor& Redactor::Add(Replacer replacer) {
  FX_CHECK(replacer != nullptr);
  replacers_.push_back(std::move(replacer));
  return *this;
}

Redactor& Redactor::AddTextReplacer(std::string_view pattern, std::string_view replacement) {
  Replacer replacer = ReplaceWithText(pattern, replacement);
  FX_CHECK(replacer != nullptr) << "Failed to build replacer for " << pattern << " " << replacement;

  return Add(std::move(replacer));
}

Redactor& Redactor::AddIdReplacer(std::string_view pattern, std::string_view format,
                                  const std::vector<std::string>& ignore_prefixes) {
  Replacer replacer = ReplaceWithIdFormatString(pattern, format, ignore_prefixes);
  FX_CHECK(replacer != nullptr) << "Failed to build replacer for " << pattern << " " << format;

  return Add(std::move(replacer));
}

Redactor& Redactor::AddJsonReplacer(Replacer replacer) {
  FX_CHECK(replacer != nullptr);
  json_replacers_.push_back(std::move(replacer));
  return *this;
}

std::string Redactor::UnredactedCanary() const { return std::string(kUnredactedCanary); }

std::string Redactor::RedactedCanary() const { return std::string(kRedactedCanary); }

IdentityRedactor::IdentityRedactor(inspect::BoolProperty redaction_enabled)
    : RedactorBase(std::move(redaction_enabled)) {}

std::string& IdentityRedactor::Redact(std::string& text) { return text; }

std::string& IdentityRedactor::RedactJson(std::string& text) { return text; }

std::string IdentityRedactor::UnredactedCanary() const { return std::string(kUnredactedCanary); }

std::string IdentityRedactor::RedactedCanary() const { return std::string(kUnredactedCanary); }

}  // namespace forensics
