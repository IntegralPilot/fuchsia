// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "src/connectivity/network/mdns/service/common/mdns_names.h"
#include "src/connectivity/network/mdns/service/encoding/dns_formatting.h"

namespace mdns {
namespace test {

// Tests |LocalHostFullName|.
TEST(MdnsNamesTest, LocalHostFullName) {
  EXPECT_EQ(DnsName("test.host.name.local."), MdnsNames::HostFullName(DnsName("test.host.name")));
  EXPECT_EQ(DnsName("test-host-name.local."), MdnsNames::HostFullName(DnsName("test-host-name")));
}

// Tests |LocalServiceFullName|.
TEST(MdnsNamesTest, LocalServiceFullName) {
  EXPECT_EQ(DnsName("_printer._tcp.local."), MdnsNames::ServiceFullName(DnsName("_printer._tcp.")));
  EXPECT_EQ(DnsName("_fuchsia._udp.local."), MdnsNames::ServiceFullName(DnsName("_fuchsia._udp.")));
}

// Tests |ServiceSubtypeFullName|.
TEST(MdnsNamesTest, ServiceSubtypeFullName) {
  EXPECT_EQ(DnsName("_color._sub._printer._tcp.local."),
            MdnsNames::ServiceSubtypeFullName(DnsName("_printer._tcp."), DnsLabel("_color")));
  EXPECT_EQ(DnsName("_nuc._sub._fuchsia._udp.local."),
            MdnsNames::ServiceSubtypeFullName(DnsName("_fuchsia._udp."), DnsLabel("_nuc")));
}

// Tests |InstanceFullName|.
TEST(MdnsNamesTest, InstanceFullName) {
  EXPECT_EQ(DnsName("Acme Splotchamatic._printer._tcp.local."),
            MdnsNames::InstanceFullName("Acme Splotchamatic", DnsName("_printer._tcp.")));
  EXPECT_EQ(DnsName("My Egg Timer._fuchsia._udp.local."),
            MdnsNames::InstanceFullName("My Egg Timer", DnsName("_fuchsia._udp.")));
  EXPECT_EQ(DnsName("My Egg Timer.com._fuchsia._udp.local.", 16),
            MdnsNames::InstanceFullName("My Egg Timer.com", DnsName("_fuchsia._udp.")));
}

// Tests |SplitInstanceFullName|.
TEST(MdnsNamesTest, SplitInstanceFullName) {
  DnsLabel instance_name;
  DnsName service_name;

  EXPECT_TRUE(MdnsNames::SplitInstanceFullName(DnsName("Acme Splotchamatic._printer._tcp.local."),
                                               &instance_name, &service_name));
  EXPECT_EQ(DnsLabel("Acme Splotchamatic"), instance_name);
  EXPECT_EQ(DnsName("_printer._tcp."), service_name);

  EXPECT_TRUE(MdnsNames::SplitInstanceFullName(DnsName("My Egg Timer._fuchsia._udp.local."),
                                               &instance_name, &service_name));
  EXPECT_EQ(DnsLabel("My Egg Timer"), instance_name);
  EXPECT_EQ(DnsName("_fuchsia._udp."), service_name);

  // No local suffix.
  EXPECT_FALSE(MdnsNames::SplitInstanceFullName(DnsName("Acme Splotchamatic._printer._tcp."),
                                                &instance_name, &service_name));

  // Just a service name.
  EXPECT_FALSE(MdnsNames::SplitInstanceFullName(DnsName("_printer._tcp.local."), &instance_name,
                                                &service_name));

  // Zero-length instance name.
  EXPECT_FALSE(MdnsNames::SplitInstanceFullName(DnsName("._printer._tcp.local."), &instance_name,
                                                &service_name));

  // Instance name almost too long.
  EXPECT_TRUE(MdnsNames::SplitInstanceFullName(
      DnsName("012345678901234567890123456789012345678901234567890123456789012._"
              "printer._tcp.local."),
      &instance_name, &service_name));
  EXPECT_EQ(DnsLabel("012345678901234567890123456789012345678901234567890123456789012"),
            instance_name);
  EXPECT_EQ(DnsName("_printer._tcp."), service_name);

  // Instance name too long.
  EXPECT_FALSE(MdnsNames::SplitInstanceFullName(
      DnsName("0123456789012345678901234567890123456789012345678901234567890123._"
              "printer._tcp.local."),
      &instance_name, &service_name));
}

// Tests |MatchServiceName|.
TEST(MdnsNamesTest, MatchServiceName) {
  DnsLabel subtype;

  EXPECT_TRUE(MdnsNames::MatchServiceName(DnsName("_printer._tcp.local."),
                                          DnsName("_printer._tcp."), &subtype));
  EXPECT_EQ(DnsLabel(""), subtype);

  EXPECT_TRUE(MdnsNames::MatchServiceName(DnsName("_fuchsia._udp.local."),
                                          DnsName("_fuchsia._udp."), &subtype));
  EXPECT_EQ(DnsLabel(""), subtype);

  EXPECT_TRUE(MdnsNames::MatchServiceName(DnsName("_color._sub._printer._tcp.local."),
                                          DnsName("_printer._tcp."), &subtype));
  EXPECT_EQ(DnsLabel("_color"), subtype);

  EXPECT_TRUE(MdnsNames::MatchServiceName(DnsName("_nuc._sub._fuchsia._udp.local."),
                                          DnsName("_fuchsia._udp."), &subtype));
  EXPECT_EQ(DnsLabel("_nuc"), subtype);

  // Wrong service type.
  EXPECT_FALSE(MdnsNames::MatchServiceName(DnsName("_printer._tcp.local."),
                                           DnsName("_fuchsia._udp."), &subtype));

  // Wrong service type with subtype.
  EXPECT_FALSE(MdnsNames::MatchServiceName(DnsName("_color._sub._printer._tcp.local."),
                                           DnsName("_fuchsia._udp."), &subtype));

  // No local suffix.
  EXPECT_FALSE(
      MdnsNames::MatchServiceName(DnsName("_printer._tcp."), DnsName("_printer._tcp."), &subtype));

  // No local suffix with subtype.
  EXPECT_FALSE(MdnsNames::MatchServiceName(DnsName("_color._sub._printer._tcp."),
                                           DnsName("_printer._tcp."), &subtype));

  // Zero-length subtype.
  EXPECT_FALSE(MdnsNames::MatchServiceName(DnsName("._sub._printer._tcp.local."),
                                           DnsName("_printer._tcp."), &subtype));

  // Missing _sub.
  EXPECT_FALSE(MdnsNames::MatchServiceName(DnsName("_color._printer._tcp.local."),
                                           DnsName("_printer._tcp."), &subtype));

  // Subtype almost too long.
  EXPECT_TRUE(MdnsNames::MatchServiceName(
      DnsName("012345678901234567890123456789012345678901234567890123456789012._sub._"
              "printer._tcp.local."),
      DnsName("_printer._tcp."), &subtype));
  EXPECT_EQ(DnsLabel("012345678901234567890123456789012345678901234567890123456789012"), subtype);

  // Subtype too long.
  EXPECT_FALSE(MdnsNames::MatchServiceName(
      DnsName("0123456789012345678901234567890123456789012345678901234567890123._sub._"
              "printer._tcp.local."),
      DnsName("_printer._tcp."), &subtype));
}

// Tests |IsValidHostName|.
TEST(MdnsNamesTest, IsValidHostName) {
  EXPECT_TRUE(MdnsNames::IsValidHostName(DnsName("gopher")));
  EXPECT_TRUE(MdnsNames::IsValidHostName(DnsName("gopher-cow-alpaca-racoon")));
  EXPECT_TRUE(MdnsNames::IsValidHostName(DnsName("gopher.cow.alpaca.racoon")));
  EXPECT_TRUE(MdnsNames::IsValidHostName(DnsName("g.c.a.r")));
  EXPECT_TRUE(MdnsNames::IsValidHostName(
      DnsName("012345678901234567890123456789012345678901234567890123456789012")));
  EXPECT_TRUE(MdnsNames::IsValidHostName(
      DnsName("012345678901234567890123456789012345678901234567890123456789012."
              "012345678901234567890123456789012345678901234567890123456789012."
              "012345678901234567890123456789012345678901234567890123456789012."
              "0123456789012345678901234567890123456789012345678901234")));

  // Empty.
  EXPECT_FALSE(MdnsNames::IsValidHostName(DnsName()));

  // Too long.
  EXPECT_FALSE(MdnsNames::IsValidHostName(
      DnsName("012345678901234567890123456789012345678901234567890123456789012."
              "012345678901234567890123456789012345678901234567890123456789012."
              "012345678901234567890123456789012345678901234567890123456789012."
              "01234567890123456789012345678901234567890123456789012345")));
}

// Tests |IsValidServiceName|.
TEST(MdnsNamesTest, IsValidServiceName) {
  EXPECT_TRUE(MdnsNames::IsValidServiceName(DnsName("_printer._tcp.")));
  EXPECT_TRUE(MdnsNames::IsValidServiceName(DnsName("_printer._udp.")));
  EXPECT_TRUE(MdnsNames::IsValidServiceName(DnsName("_._udp.")));
  EXPECT_TRUE(MdnsNames::IsValidServiceName(DnsName("_x._udp.")));
  EXPECT_TRUE(MdnsNames::IsValidServiceName(DnsName("_012345678901234._tcp.")));

  // Empty.
  EXPECT_FALSE(MdnsNames::IsValidServiceName(DnsName()));

  // Invalid transport.
  EXPECT_FALSE(MdnsNames::IsValidServiceName(DnsName("_printer._qfc.")));

  // Empty label.
  EXPECT_FALSE(MdnsNames::IsValidServiceName(DnsName(")._tcp.")));

  // Label too long.
  EXPECT_FALSE(MdnsNames::IsValidServiceName(DnsName("_0123456789012345._tcp.")));

  // No leading underscore.
  EXPECT_FALSE(MdnsNames::IsValidServiceName(DnsName("printer._tcp.")));

  // Too many labels
  EXPECT_FALSE(MdnsNames::IsValidServiceName(DnsName("pretty.printer._tcp.")));
}

// Tests |IsValidInstanceName|.
TEST(MdnsNamesTest, IsValidInstanceName) {
  EXPECT_TRUE(MdnsNames::IsValidInstanceName(DnsLabel("x")));
  EXPECT_TRUE(MdnsNames::IsValidInstanceName(DnsLabel("x-ray machine")));
  EXPECT_TRUE(MdnsNames::IsValidInstanceName(
      DnsLabel("012345678901234567890123456789012345678901234567890123456789012")));

  // Empty.
  EXPECT_FALSE(MdnsNames::IsValidInstanceName(DnsLabel()));

  // Embedded dot.
  EXPECT_TRUE(MdnsNames::IsValidInstanceName(DnsLabel("gopher.cow")));

  // Too long.
  EXPECT_FALSE(MdnsNames::IsValidInstanceName(
      DnsLabel("0123456789012345678901234567890123456789012345678901234567890123")));
}

// Tests |IsValidSubtypeName|.
TEST(MdnsNamesTest, IsValidSubtypeName) {
  EXPECT_TRUE(MdnsNames::IsValidSubtypeName(DnsLabel("x")));
  EXPECT_TRUE(MdnsNames::IsValidSubtypeName(DnsLabel("x-ray machine")));
  EXPECT_TRUE(MdnsNames::IsValidSubtypeName(
      DnsLabel("012345678901234567890123456789012345678901234567890123456789012")));

  // Empty.
  EXPECT_FALSE(MdnsNames::IsValidSubtypeName(DnsLabel("")));

  // Just a dot.
  EXPECT_FALSE(MdnsNames::IsValidSubtypeName(DnsLabel(".")));

  // More than one label.
  EXPECT_FALSE(MdnsNames::IsValidSubtypeName(DnsLabel("gopher.cow")));

  // Too long.
  EXPECT_FALSE(MdnsNames::IsValidSubtypeName(
      DnsLabel("0123456789012345678901234567890123456789012345678901234567890123")));
}

// Tests |IsValidTextString|.
TEST(MdnsNamesTest, IsValidTextString) {
  EXPECT_TRUE(MdnsNames::IsValidTextString(""));
  EXPECT_TRUE(MdnsNames::IsValidTextString("."));
  EXPECT_TRUE(MdnsNames::IsValidTextString("x.y"));
  EXPECT_TRUE(MdnsNames::IsValidTextString("x=y"));
  EXPECT_TRUE(MdnsNames::IsValidTextString("x"));
  EXPECT_TRUE(MdnsNames::IsValidTextString("x-ray machine"));
  EXPECT_TRUE(MdnsNames::IsValidTextString(
      "012345678901234567890123456789012345678901234567890123456789012345678901"
      "234567890123456789012345678901234567890123456789012345678901234567890123"
      "456789012345678901234567890123456789012345678901234567890123456789012345"
      "678901234567890123456789012345678901234"));

  // Too long.
  EXPECT_FALSE(MdnsNames::IsValidTextString(
      "012345678901234567890123456789012345678901234567890123456789012345678901"
      "234567890123456789012345678901234567890123456789012345678901234567890123"
      "456789012345678901234567890123456789012345678901234567890123456789012345"
      "6789012345678901234567890123456789012345"));
}

// Tests |AltHostName|.
TEST(MdnsNamesTest, AltHostName) {
  EXPECT_EQ(DnsName("123456789ABC"), MdnsNames::AltHostName(DnsName("fuchsia-1234-5678-9abc")));
  EXPECT_EQ(DnsName("ABCDEFABCDEF"), MdnsNames::AltHostName(DnsName("fuchsia-abcd-efab-cdef")));
  EXPECT_EQ(DnsName("000000000000"), MdnsNames::AltHostName(DnsName("fuchsia-0000-0000-0000")));
  EXPECT_EQ(DnsName("unexpected format"), MdnsNames::AltHostName(DnsName("unexpected format")));
  EXPECT_EQ(DnsName("longer unexpected format"),
            MdnsNames::AltHostName(DnsName("longer unexpected format")));
}

}  // namespace test
}  // namespace mdns
