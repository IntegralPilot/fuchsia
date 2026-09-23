// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/utils/redact/redactor.h"

#include <lib/inspect/cpp/vmo/types.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/json_parser/json_parser.h"

namespace forensics {
namespace {

class IdentityRedactorTest : public ::testing::Test {
 protected:
  std::string Redact(std::string text) { return redactor_.Redact(text); }

 private:
  IdentityRedactor redactor_{inspect::BoolProperty()};
};

TEST_F(IdentityRedactorTest, Check) {
  EXPECT_EQ(Redact("Email: alice@website.tld"), "Email: alice@website.tld");
  EXPECT_EQ(Redact("Serial number: '12345678'"), "Serial number: '12345678'");
}

class RedactorTest : public ::testing::Test {
 protected:
  std::string Redact(std::string text) { return redactor_.Redact(text); }
  std::string RedactJson(std::string text) { return redactor_.RedactJson(text); }

  const Redactor& redactor() const { return redactor_; }

 private:
  Redactor redactor_{0u, inspect::UintProperty(), inspect::BoolProperty()};
};

TEST_F(RedactorTest, RedactsEmail) {
  EXPECT_EQ(Redact("Email: alice@website.tld"), "Email: <REDACTED-EMAIL>");
}

TEST_F(RedactorTest, RedactsIpv4) {
  EXPECT_EQ(Redact("IPv4: 8.8.8.8"), "IPv4: <REDACTED-IPV4: 1>");
}

TEST_F(RedactorTest, RedactsIpv4InIpv6) {
  EXPECT_EQ(Redact("IPv46: ::ffff:12.34.56.78"), "IPv46: ::ffff:<REDACTED-IPV4: 1>");
}

TEST_F(RedactorTest, RedactsIpv4InIpv6Hex) {
  EXPECT_EQ(Redact("IPv46h: ::ffff:ab12:34cd"), "IPv46h: ::ffff:<REDACTED-IPV4: 1>");
}

TEST_F(RedactorTest, RedactsIpv6) {
  EXPECT_EQ(Redact("not_IPv46h: ::ffff:ab12:34cd:5"), "not_IPv46h: <REDACTED-IPV6: 1>");
  EXPECT_EQ(Redact("IPv6: 2001:503:eEa3:0:0:0:0:30"), "IPv6: <REDACTED-IPV6: 2>");
}

TEST_F(RedactorTest, RedactsIpv6Complex) {
  EXPECT_EQ(Redact("IPv6C: [::/0 via 2082::7d84:c1dc:ab34:656a nic 4]"),
            "IPv6C: [::/0 via <REDACTED-IPV6: 1> nic 4]");
}

TEST_F(RedactorTest, RedactsIpv6LinkLocal) {
  EXPECT_EQ(Redact("IPv6LL: fe80::7d84:c1dc:ab34:656a"), "IPv6LL: fe80:<REDACTED-IPV6-LL: 1>");
}

TEST_F(RedactorTest, RedactsUuid) {
  EXPECT_EQ(Redact("UUID: ddd0fA34-1016-11eb-adc1-0242ac120002"), "UUID: <REDACTED-UUID>");
}

TEST_F(RedactorTest, RedactsSerialNumber) {
  EXPECT_EQ(Redact("Serial number: '12345678'"), "<REDACTED-SERIAL>");
  EXPECT_EQ(Redact("       serial=3125592196"), "       <REDACTED-SERIAL>");
  EXPECT_EQ(Redact("serial_number: ABC-DEF_123"), "<REDACTED-SERIAL>");
  EXPECT_EQ(Redact("serial = 'foo'"), "<REDACTED-SERIAL>");
  EXPECT_EQ(Redact("SERIAL: 99999"), "<REDACTED-SERIAL>");
  EXPECT_EQ(Redact("deserialization error"), "deserialization error");
  EXPECT_EQ(Redact("serial port initialized"), "serial port initialized");
}

TEST_F(RedactorTest, RedactsMacAddress) {
  EXPECT_EQ(Redact("MAC address: 00:0a:95:9F:68:16 12-34-95-9F-68-16"),
            "MAC address: 00:0a:95:<REDACTED-MAC: 1> 12-34-95-<REDACTED-MAC: 2>");
}

TEST_F(RedactorTest, RedactsSsid) {
  EXPECT_EQ(Redact("SSID: <ssid-666F6F> <ssid-77696669>"),
            "SSID: <REDACTED-SSID: 1> <REDACTED-SSID: 2>");
}

TEST_F(RedactorTest, RedactsHttpUrl) {
  EXPECT_EQ(Redact("HTTP: http://fuchsia.dev/"), "HTTP: <REDACTED-URL>");
}

TEST_F(RedactorTest, RedactsHttpsUrl) {
  EXPECT_EQ(Redact("HTTPS: https://fuchsia.dev/"), "HTTPS: <REDACTED-URL>");
}

TEST_F(RedactorTest, RedactsUrlWithSemicolon) {
  EXPECT_EQ(Redact("URL with semicolon: https://fuchsia.dev?query=a;b"),
            "URL with semicolon: <REDACTED-URL>");
}

TEST_F(RedactorTest, RedactsUrlWithUuid) {
  EXPECT_EQ(
      Redact("URL with UUID: https://fuchsia.dev/ddd0fA34-1016-11eb-adc1-0242ac120002?query=a;b"),
      "URL with UUID: <REDACTED-URL>");
}

TEST_F(RedactorTest, RedactsUrlWithIpv4Host) {
  EXPECT_EQ(Redact("GET https://8.8.8.8/v1/fetch?token=SECRET"), "GET <REDACTED-URL>");
}

TEST_F(RedactorTest, RedactsUrlWithIpv6Host) {
  EXPECT_EQ(Redact("request https://[2001:db8::1]/v1?auth=SecretKey"), "request <REDACTED-URL>");
}

TEST_F(RedactorTest, RedactsUrlWithIpv4HostAndPort) {
  EXPECT_EQ(Redact("fetch https://203.0.113.42:8080/media?sig=QWxhZGRpbg"), "fetch <REDACTED-URL>");
}

TEST_F(RedactorTest, RedactsCombined) {
  EXPECT_EQ(Redact("Combined: Email alice@website.tld, IPv4 8.8.8.8"),
            "Combined: Email <REDACTED-EMAIL>, IPv4 <REDACTED-IPV4: 1>");
}

TEST_F(RedactorTest, DoesNotRedactFidlService) {
  EXPECT_EQ(Redact("service::fidl service:fidl"), "service::fidl service:fidl");
}

TEST_F(RedactorTest, RedactsHexAndIpv4) {
  EXPECT_EQ(Redact("456 1234567890abcdefABCDEF0123456789 1.2.3.4"),
            "456 <REDACTED-HEX: 2> <REDACTED-IPV4: 1>");
}

TEST_F(RedactorTest, DoesNotRedactPartialIpv4) {
  EXPECT_EQ(Redact("current: 0.8.8.8"), "current: 0.8.8.8");
}

TEST_F(RedactorTest, DoesNotRedactLoopbackIpv4) {
  EXPECT_EQ(Redact("loopback: 127.8.8.8"), "loopback: 127.8.8.8");
}

TEST_F(RedactorTest, DoesNotRedactLinkLocalIpv4) {
  EXPECT_EQ(Redact("link_local: 169.254.8.8"), "link_local: 169.254.8.8");
}

TEST_F(RedactorTest, DoesNotRedactLinkLocalMulticastIpv4) {
  EXPECT_EQ(Redact("link_local_multicast: 224.0.0.8"), "link_local_multicast: 224.0.0.8");
}

TEST_F(RedactorTest, DoesNotRedactBroadcastIpv4) {
  EXPECT_EQ(Redact("broadcast: 255.255.255.255"), "broadcast: 255.255.255.255");
}

TEST_F(RedactorTest, RedactsNonBroadcastIpv4) {
  EXPECT_EQ(Redact("not_broadcast: 255.255.255.254"), "not_broadcast: <REDACTED-IPV4: 1>");
}

TEST_F(RedactorTest, RedactsNonLinkLocalMulticastIpv4) {
  EXPECT_EQ(Redact("not_link_local_multicast: 224.0.1.8"),
            "not_link_local_multicast: <REDACTED-IPV4: 1>");
}

TEST_F(RedactorTest, DoesNotRedactLocalMulticastIpv6) {
  EXPECT_EQ(Redact("local_multicast_1: fF41::1234:5678:9aBc"),
            "local_multicast_1: fF41::1234:5678:9aBc");
  EXPECT_EQ(Redact("local_multicast_2: Ffe2:1:2:33:abcd:ef0:6789:456"),
            "local_multicast_2: Ffe2:1:2:33:abcd:ef0:6789:456");
}

TEST_F(RedactorTest, RedactsMulticastIpv6) {
  EXPECT_EQ(Redact("multicast: fF43:abcd::ef0:6789:456"),
            "multicast: fF43:<REDACTED-IPV6-MULTI: 1>");
}

TEST_F(RedactorTest, RedactsLinkLocalIpv6) {
  EXPECT_EQ(Redact("link_local_8: fe89:123::4567:8:90"),
            "link_local_8: fe89:<REDACTED-IPV6-LL: 1>");
  EXPECT_EQ(Redact("link_local_b: FEB2:123::4567:8:90"),
            "link_local_b: FEB2:<REDACTED-IPV6-LL: 2>");
}

TEST_F(RedactorTest, RedactsNonLinkLocalIpv6) {
  EXPECT_EQ(Redact("not_link_local: fec1:123::4567:8:90"), "not_link_local: <REDACTED-IPV6: 1>");
  EXPECT_EQ(Redact("not_link_local_2: fe71:123::4567:8:90"),
            "not_link_local_2: <REDACTED-IPV6: 2>");
}

TEST_F(RedactorTest, DoesNotRedactInvalidIpv6) {
  EXPECT_EQ(Redact("not_address_1: 12:34::"), "not_address_1: 12:34::");
  EXPECT_EQ(Redact("not_address_2: ::12:34"), "not_address_2: ::12:34");
}

TEST_F(RedactorTest, RedactsValidIpv6WithEdgeCaseColons) {
  EXPECT_EQ(Redact("v6_colons_3_fields: ::12:34:5"), "v6_colons_3_fields: <REDACTED-IPV6: 1>");
  EXPECT_EQ(Redact("v6_3_fields_colons: 12:34:5::"), "v6_3_fields_colons: <REDACTED-IPV6: 2>");
  EXPECT_EQ(Redact("v6_colons_7_fields: ::12:234:35:46:5:6:7"),
            "v6_colons_7_fields: <REDACTED-IPV6: 3>");
  EXPECT_EQ(Redact("v6_7_fields_colons: 12:234:35:46:5:6:7::"),
            "v6_7_fields_colons: <REDACTED-IPV6: 4>");
  EXPECT_EQ(Redact("v6_colons_8_fields: ::12:234:35:46:5:6:7:8"),
            "v6_colons_8_fields: <REDACTED-IPV6: 3>:8");
  EXPECT_EQ(Redact("v6_8_fields_colons: 12:234:35:46:5:6:7:8::"),
            "v6_8_fields_colons: <REDACTED-IPV6: 5>::");
}

TEST_F(RedactorTest, RedactsObfuscatedGaiaId) {
  EXPECT_EQ(Redact("obfuscated_gaia_id: 106986199446298680449"),
            "obfuscated_gaia_id: <REDACTED-OBFUSCATED-GAIA-ID: 1>");
}

TEST_F(RedactorTest, Redacts32ByteHex) {
  EXPECT_EQ(
      Redact("32_hex: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 33_hex: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
      "32_hex: <REDACTED-HEX: 1> 33_hex: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
}

TEST_F(RedactorTest, Redacts16ByteHex) {
  EXPECT_EQ(Redact("456 1234567890abcdef 789"), "456 <REDACTED-HEX: 1> 789");
}

TEST_F(RedactorTest, DoesNotRedactHexWithElfPrefix) {
  EXPECT_EQ(Redact("456 elf:1234567890abcdef 789"), "456 elf:1234567890abcdef 789");
  EXPECT_EQ(Redact("456 elf:1234567890abcdefABCDEF0123456789 789"),
            "456 elf:1234567890abcdefABCDEF0123456789 789");
}

TEST_F(RedactorTest, DoesNotRedactBuildId) {
  EXPECT_EQ(Redact("456 build_id: '5f2c0ede0fa479b9b997c4fce6d4cf24' 789"),
            "456 build_id: '5f2c0ede0fa479b9b997c4fce6d4cf24' 789");
}

TEST_F(RedactorTest, RedactsIpv4InFidl) {
  EXPECT_EQ(Redact("ipv4 fidl debug: Ipv4Address { addr: [1, 255, FF, FF] }"),
            "ipv4 fidl debug: Ipv4Address { <REDACTED-IPV4: 1> }");
}

TEST_F(RedactorTest, RedactsIpv6InFidl) {
  EXPECT_EQ(
      Redact(
          "ipv6 fidl debug: Ipv6Address { addr: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 255, FF, FF] }"),
      "ipv6 fidl debug: Ipv6Address { <REDACTED-IPV6: 1> }");
}

TEST_F(RedactorTest, RedactsMacInFidl) {
  EXPECT_EQ(Redact("mac fidl debug: MacAddress { octets: [1, 2, 3, 255, FF, FF] }"),
            "mac fidl debug: MacAddress { <REDACTED-MAC: 1> }");
}

TEST_F(RedactorTest, RedactsSensitive) {
  EXPECT_EQ(Redact("SENSITIVE{12345}"), "<REDACTED-SENSITIVE>");
  EXPECT_EQ(Redact("addr: SENSITIVE{[1, 2, 3, 4]}"), "addr: <REDACTED-SENSITIVE>");
  EXPECT_EQ(Redact("SENSITIVE{SensitiveStruct { field: \"secret\" }}"), "<REDACTED-SENSITIVE>");
  EXPECT_EQ(Redact("Combined: SENSITIVE{foo}, IPv4 8.8.8.8"),
            "Combined: <REDACTED-SENSITIVE>, IPv4 <REDACTED-IPV4: 1>");
}

TEST_F(RedactorTest, RedactsSensitiveWithUnmatchedOrNestedBraces) {
  // Unclosed curly brace at the end of a log fails closed by redacting to the end.
  EXPECT_EQ(Redact("unclosed: SENSITIVE{something"), "unclosed: <REDACTED-SENSITIVE>");

  // Unmatched closing brace outside the sensitive block is preserved.
  EXPECT_EQ(Redact("extra closing: SENSITIVE{foo}}"), "extra closing: <REDACTED-SENSITIVE>}");

  // Unmatched curly braces inside quoted strings (e.g., in error descriptions) do not affect
  // matching.
  EXPECT_EQ(Redact("error: SENSITIVE{RustError(\"unmatched { brace\")}"),
            "error: <REDACTED-SENSITIVE>");
  EXPECT_EQ(Redact("error: SENSITIVE{RustError(\"unmatched } brace\")}"),
            "error: <REDACTED-SENSITIVE>");
}

TEST_F(RedactorTest, JsonRedactsSensitive) {
  EXPECT_EQ(RedactJson("SENSITIVE{12345}"), "<REDACTED-SENSITIVE>");
  EXPECT_EQ(RedactJson("addr: SENSITIVE{[1, 2, 3, 4]}"), "addr: <REDACTED-SENSITIVE>");
  EXPECT_EQ(RedactJson("SENSITIVE{SensitiveStruct { field: \"secret\" }}"), "<REDACTED-SENSITIVE>");
}

TEST_F(RedactorTest, Canary) {
  EXPECT_EQ(Redact(redactor().UnredactedCanary()), redactor().RedactedCanary());
}

TEST_F(RedactorTest, JsonDoesNotRedactEmail) {
  EXPECT_EQ(RedactJson("Email: alice@website.tld"), "Email: alice@website.tld");
}

TEST_F(RedactorTest, JsonRedactsIpv4) {
  EXPECT_EQ(RedactJson("IPv4: 8.8.8.8"), "IPv4: <REDACTED-IPV4: 1>");
}

TEST_F(RedactorTest, JsonRedactsIpv4InIpv6) {
  EXPECT_EQ(RedactJson("IPv46: ::ffff:12.34.56.78"), "IPv46: ::ffff:<REDACTED-IPV4: 1>");
}

TEST_F(RedactorTest, JsonRedactsIpv4InIpv6Hex) {
  EXPECT_EQ(RedactJson("IPv46h: ::ffff:ab12:34cd"), "IPv46h: ::ffff:<REDACTED-IPV4: 1>");
}

TEST_F(RedactorTest, JsonRedactsIpv6) {
  EXPECT_EQ(RedactJson("not_IPv46h: ::ffff:ab12:34cd:5"), "not_IPv46h: <REDACTED-IPV6: 1>");
  EXPECT_EQ(RedactJson("IPv6: 2001:503:eEa3:0:0:0:0:30"), "IPv6: <REDACTED-IPV6: 2>");
}

TEST_F(RedactorTest, JsonRedactsIpv6Complex) {
  EXPECT_EQ(RedactJson("IPv6C: [::/0 via 2082::7d84:c1dc:ab34:656a nic 4]"),
            "IPv6C: [::/0 via <REDACTED-IPV6: 1> nic 4]");
}

TEST_F(RedactorTest, JsonRedactsIpv6LinkLocal) {
  EXPECT_EQ(RedactJson("IPv6LL: fe80::7d84:c1dc:ab34:656a"), "IPv6LL: fe80:<REDACTED-IPV6-LL: 1>");
}

TEST_F(RedactorTest, JsonDoesNotRedactUuid) {
  EXPECT_EQ(RedactJson("UUID: ddd0fA34-1016-11eb-adc1-0242ac120002"),
            "UUID: ddd0fA34-1016-11eb-adc1-0242ac120002");
}

TEST_F(RedactorTest, JsonDoesNotRedactHttp) {
  EXPECT_EQ(RedactJson("HTTP: http://fuchsia.dev/"), "HTTP: http://fuchsia.dev/");
}

TEST_F(RedactorTest, JsonDoesNotRedactHttps) {
  EXPECT_EQ(RedactJson("HTTPS: https://fuchsia.dev/"), "HTTPS: https://fuchsia.dev/");
}

TEST_F(RedactorTest, JsonDoesNotRedactUrlWithSemicolon) {
  EXPECT_EQ(RedactJson("URL with semicolon: https://fuchsia.dev?query=a;b"),
            "URL with semicolon: https://fuchsia.dev?query=a;b");
}

TEST_F(RedactorTest, JsonDoesNotRedactUrlWithUuid) {
  EXPECT_EQ(
      RedactJson(
          "URL with UUID: https://fuchsia.dev/ddd0fA34-1016-11eb-adc1-0242ac120002?query=a;b"),
      "URL with UUID: https://fuchsia.dev/ddd0fA34-1016-11eb-adc1-0242ac120002?query=a;b");
}

TEST_F(RedactorTest, JsonRedactsCombined) {
  EXPECT_EQ(RedactJson("Combined: Email alice@website.tld, IPv4 8.8.8.8"),
            "Combined: Email alice@website.tld, IPv4 <REDACTED-IPV4: 1>");
}

TEST_F(RedactorTest, JsonDoesNotRedactFidlService) {
  EXPECT_EQ(RedactJson("service::fidl service:fidl"), "service::fidl service:fidl");
}

TEST_F(RedactorTest, JsonRedactsIpv4ButNotHex) {
  EXPECT_EQ(RedactJson("456 1234567890abcdefABCDEF0123456789 1.2.3.4"),
            "456 1234567890abcdefABCDEF0123456789 <REDACTED-IPV4: 1>");
}

TEST_F(RedactorTest, JsonDoesNotRedactPartialIpv4) {
  EXPECT_EQ(RedactJson("current: 0.8.8.8"), "current: 0.8.8.8");
}

TEST_F(RedactorTest, JsonDoesNotRedactLoopbackIpv4) {
  EXPECT_EQ(RedactJson("loopback: 127.8.8.8"), "loopback: 127.8.8.8");
}

TEST_F(RedactorTest, JsonDoesNotRedactLinkLocalIpv4) {
  EXPECT_EQ(RedactJson("link_local: 169.254.8.8"), "link_local: 169.254.8.8");
}

TEST_F(RedactorTest, JsonDoesNotRedactLinkLocalMulticastIpv4) {
  EXPECT_EQ(RedactJson("link_local_multicast: 224.0.0.8"), "link_local_multicast: 224.0.0.8");
}

TEST_F(RedactorTest, JsonDoesNotRedactBroadcastIpv4) {
  EXPECT_EQ(RedactJson("broadcast: 255.255.255.255"), "broadcast: 255.255.255.255");
}

TEST_F(RedactorTest, JsonRedactsNonBroadcastIpv4) {
  EXPECT_EQ(RedactJson("not_broadcast: 255.255.255.254"), "not_broadcast: <REDACTED-IPV4: 1>");
}

TEST_F(RedactorTest, JsonRedactsNonLinkLocalMulticastIpv4) {
  EXPECT_EQ(RedactJson("not_link_local_multicast: 224.0.1.8"),
            "not_link_local_multicast: <REDACTED-IPV4: 1>");
}

TEST_F(RedactorTest, JsonDoesNotRedactLocalMulticastIpv6) {
  EXPECT_EQ(RedactJson("local_multicast_1: fF41::1234:5678:9aBc"),
            "local_multicast_1: fF41::1234:5678:9aBc");
  EXPECT_EQ(RedactJson("local_multicast_2: Ffe2:1:2:33:abcd:ef0:6789:456"),
            "local_multicast_2: Ffe2:1:2:33:abcd:ef0:6789:456");
}

TEST_F(RedactorTest, JsonRedactsMulticastIpv6) {
  EXPECT_EQ(RedactJson("multicast: fF43:abcd::ef0:6789:456"),
            "multicast: fF43:<REDACTED-IPV6-MULTI: 1>");
}

TEST_F(RedactorTest, JsonRedactsLinkLocalIpv6) {
  EXPECT_EQ(RedactJson("link_local_8: fe89:123::4567:8:90"),
            "link_local_8: fe89:<REDACTED-IPV6-LL: 1>");
  EXPECT_EQ(RedactJson("link_local_b: FEB2:123::4567:8:90"),
            "link_local_b: FEB2:<REDACTED-IPV6-LL: 2>");
}

TEST_F(RedactorTest, JsonRedactsNonLinkLocalIpv6) {
  EXPECT_EQ(RedactJson("not_link_local: fec1:123::4567:8:90"),
            "not_link_local: <REDACTED-IPV6: 1>");
  EXPECT_EQ(RedactJson("not_link_local_2: fe71:123::4567:8:90"),
            "not_link_local_2: <REDACTED-IPV6: 2>");
}

TEST_F(RedactorTest, JsonDoesNotRedactInvalidIpv6) {
  EXPECT_EQ(RedactJson("not_address_1: 12:34::"), "not_address_1: 12:34::");
  EXPECT_EQ(RedactJson("not_address_2: ::12:34"), "not_address_2: ::12:34");
}

TEST_F(RedactorTest, JsonRedactsValidIpv6WithEdgeCaseColons) {
  EXPECT_EQ(RedactJson("v6_colons_3_fields: ::12:34:5"), "v6_colons_3_fields: <REDACTED-IPV6: 1>");
  EXPECT_EQ(RedactJson("v6_3_fields_colons: 12:34:5::"), "v6_3_fields_colons: <REDACTED-IPV6: 2>");
  EXPECT_EQ(RedactJson("v6_colons_7_fields: ::12:234:35:46:5:6:7"),
            "v6_colons_7_fields: <REDACTED-IPV6: 3>");
  EXPECT_EQ(RedactJson("v6_7_fields_colons: 12:234:35:46:5:6:7::"),
            "v6_7_fields_colons: <REDACTED-IPV6: 4>");
  EXPECT_EQ(RedactJson("v6_colons_8_fields: ::12:234:35:46:5:6:7:8"),
            "v6_colons_8_fields: <REDACTED-IPV6: 3>:8");
  EXPECT_EQ(RedactJson("v6_8_fields_colons: 12:234:35:46:5:6:7:8::"),
            "v6_8_fields_colons: <REDACTED-IPV6: 5>::");
}

TEST_F(RedactorTest, JsonDoesNotRedactObfuscatedGaiaId) {
  EXPECT_EQ(RedactJson("obfuscated_gaia_id: 106986199446298680449"),
            "obfuscated_gaia_id: 106986199446298680449");
}

TEST_F(RedactorTest, JsonRedactsMacAddress) {
  EXPECT_EQ(RedactJson("MAC address: 00:0a:95:9F:68:16 12-34-95-9F-68-16"),
            "MAC address: 00:0a:95:<REDACTED-MAC: 1> 12-34-95-<REDACTED-MAC: 2>");
}

TEST_F(RedactorTest, JsonRedactsSsid) {
  EXPECT_EQ(RedactJson("SSID: <ssid-666F6F> <ssid-77696669>"),
            "SSID: <REDACTED-SSID: 1> <REDACTED-SSID: 2>");
}

TEST_F(RedactorTest, RedactedJsonStillValid) {
  std::string json = R"(
{
  "addresses" : {
    "ipv4_addrs" : [
      "1.2.3.4",
      "5.6.7.8"
    ],
    "ipv6_addrs" : [
      "2001::1",
      "2001::2"
    ],
    "mac_addrs" : [
      "AA:BB:CC:DD:EE:FF",
      "11-22-33-44-55-66"
    ],
    "ssids" : [
      "<ssid-0123abcdef>",
      "<ssid-4567fedcba>"
    ]
  },
  "hex_id" : "1234567890abcdefABCDEF0123456789",
  "gaia_id" : 106986199446298680449,
  "log_message" : "hex 1234567890abcdefABCDEF0123456789 associated with gaia 106986199446298680449"
}
  )";
  json_parser::JSONParser unredacted_parser;
  unredacted_parser.ParseFromString(json, "unredacted");
  ASSERT_EQ(unredacted_parser.HasError(), false) << unredacted_parser.error_str();

  json_parser::JSONParser redacted_parser;
  const std::string redacted_json = RedactJson(json);
  redacted_parser.ParseFromString(redacted_json, "redacted");
  ASSERT_EQ(redacted_parser.HasError(), false) << redacted_parser.error_str();

  ASSERT_EQ(redacted_json, R"(
{
  "addresses" : {
    "ipv4_addrs" : [
      "<REDACTED-IPV4: 1>",
      "<REDACTED-IPV4: 2>"
    ],
    "ipv6_addrs" : [
      "<REDACTED-IPV6: 3>",
      "<REDACTED-IPV6: 4>"
    ],
    "mac_addrs" : [
      "AA:BB:CC:<REDACTED-MAC: 5>",
      "11-22-33-<REDACTED-MAC: 6>"
    ],
    "ssids" : [
      "<REDACTED-SSID: 7>",
      "<REDACTED-SSID: 8>"
    ]
  },
  "hex_id" : "1234567890abcdefABCDEF0123456789",
  "gaia_id" : 106986199446298680449,
  "log_message" : "hex 1234567890abcdefABCDEF0123456789 associated with gaia 106986199446298680449"
}
  )");
}

TEST_F(RedactorTest, CachePersistsAcrossTextAndJson) {
  std::string text = R"(
IPv4: 1.2.3.4 5.6.7.8
IPv6: 2001::1 2001::2
MAC address: 00-0a-95-9F-68-16 12:34:95:9F:68:16
SSID: <ssid-0123abcdef> <ssid-4567fedcba>
)";

  std::string json = R"(
{
  "addresses" : {
    "ipv4_addrs" : [
      "5.6.7.8",
      "1.2.3.4"
    ],
    "ipv6_addrs" : [
      "2001::2",
      "2001::1"
    ],
    "mac_addrs" : [
      "12-34-95-9F-68-16",
      "00:0a:95:9F:68:16"
    ],
    "ssids" : [
      "<ssid-4567fedcba>",
      "<ssid-0123abcdef>"
    ]
  }
}
  )";

  EXPECT_EQ(Redact(text), R"(
IPv4: <REDACTED-IPV4: 1> <REDACTED-IPV4: 2>
IPv6: <REDACTED-IPV6: 3> <REDACTED-IPV6: 4>
MAC address: 00-0a-95-<REDACTED-MAC: 5> 12:34:95:<REDACTED-MAC: 6>
SSID: <REDACTED-SSID: 7> <REDACTED-SSID: 8>
)");
  EXPECT_EQ(Redact(json), R"(
{
  "addresses" : {
    "ipv4_addrs" : [
      "<REDACTED-IPV4: 2>",
      "<REDACTED-IPV4: 1>"
    ],
    "ipv6_addrs" : [
      "<REDACTED-IPV6: 4>",
      "<REDACTED-IPV6: 3>"
    ],
    "mac_addrs" : [
      "12-34-95-<REDACTED-MAC: 6>",
      "00:0a:95:<REDACTED-MAC: 5>"
    ],
    "ssids" : [
      "<REDACTED-SSID: 8>",
      "<REDACTED-SSID: 7>"
    ]
  }
}
  )");
}

}  // namespace
}  // namespace forensics
