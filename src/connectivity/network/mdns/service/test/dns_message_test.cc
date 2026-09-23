// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/network/mdns/service/encoding/dns_message.h"

#include <gtest/gtest.h>

namespace mdns::test {

const DnsName kInstanceFullName("testinstance._testservice._tcp.local.");
const std::vector<std::string> kTextStrings{"test string 1", "test string 2", "etc"};

// Tests DnsName constructor with first label size.
TEST(DnsMessageTest, Constructor) {
  DnsName name("testinstance", 12);
  EXPECT_EQ("testinstance", name.first_label_view());
  EXPECT_EQ("testinstance.", name.to_string());
}

// Tests DnsName equality operators.
TEST(DnsMessageTest, EqualityOperator) {
  EXPECT_TRUE(DnsName("cruel.shoes.") == DnsName("cruel.shoes."));
  EXPECT_TRUE(DnsName("nice.shoes.") != DnsName("cruel.shoes."));
  EXPECT_FALSE(DnsName("cruel.shoes.") != DnsName("cruel.shoes."));
  EXPECT_FALSE(DnsName("nice.shoes.") == DnsName("cruel.shoes."));
  EXPECT_TRUE(DnsName("cruel.shoes.") == DnsName("cruel.shoes.", 5));
  EXPECT_TRUE(DnsName("cruel.shoes.") != DnsName("cruel.shoes.", 11));
  EXPECT_FALSE(DnsName("cruel.shoes.") != DnsName("cruel.shoes.", 5));
  EXPECT_FALSE(DnsName("cruel.shoes.") == DnsName("cruel.shoes.", 11));
}

// Tests DnsName hashing.
TEST(DnsMessageTest, Hash) {
  DnsName name1("Cruel.Shoes.");
  DnsName name2("cruel.shoes.");
  DnsName name3("nice.shoes.");

  EXPECT_TRUE(name1 == name2);
  EXPECT_EQ(name1.hash(), name2.hash());
  EXPECT_NE(name1.hash(), name3.hash());

  // Also check DnsName hash in std::hash
  std::hash<DnsName> hasher;
  EXPECT_EQ(hasher(name1), hasher(name2));
  EXPECT_NE(hasher(name1), hasher(name3));
}

// Tests DnsName::first_label_view and DnsName::next_label_view.
TEST(DnsMessageTest, LabelViews) {
  DnsName name("testinstance._testservice._tcp.local.");
  auto label = name.first_label_view();
  EXPECT_EQ("testinstance", label);
  label = name.next_label_view(label);
  EXPECT_EQ("_testservice", label);
  label = name.next_label_view(label);
  EXPECT_EQ("_tcp", label);
  label = name.next_label_view(label);
  EXPECT_EQ("local", label);
  label = name.next_label_view(label);
  EXPECT_TRUE(label.empty());

  // next_level_view should return an empty label given an empty label.
  label = name.next_label_view(label);
  EXPECT_TRUE(label.empty());

  // Try a name who's first label contains dots.
  DnsName name_with_dots("test...instance._testservice._tcp.local.", 15);
  label = name_with_dots.first_label_view();
  EXPECT_EQ("test...instance", label);
  label = name_with_dots.next_label_view(label);
  EXPECT_EQ("_testservice", label);
  label = name_with_dots.next_label_view(label);
  EXPECT_EQ("_tcp", label);
  label = name_with_dots.next_label_view(label);
  EXPECT_EQ("local", label);
  label = name_with_dots.next_label_view(label);
  EXPECT_TRUE(label.empty());
}

// Tests DnsName::push_back.
TEST(DnsMessageTest, Pushback) {
  DnsName name;
  EXPECT_TRUE(name.empty());

  name.push_back("chowder");
  EXPECT_EQ("chowder.", name.to_string());
  EXPECT_FALSE(name.empty());
  EXPECT_EQ("chowder", name.first_label_view());

  name.push_back("for");
  EXPECT_EQ("chowder.for.", name.to_string());
  EXPECT_FALSE(name.empty());
  EXPECT_EQ("chowder", name.first_label_view());

  name.push_back("lunch");
  EXPECT_EQ("chowder.for.lunch.", name.to_string());
  EXPECT_FALSE(name.empty());
  EXPECT_EQ("chowder", name.first_label_view());

  name = DnsName("not.nothing");
  EXPECT_EQ("not.nothing.", name.to_string());
  EXPECT_FALSE(name.empty());
  EXPECT_EQ("not", name.first_label_view());

  name.push_back("just");
  EXPECT_EQ("not.nothing.just.", name.to_string());
  EXPECT_FALSE(name.empty());
  EXPECT_EQ("not", name.first_label_view());

  name.push_back("something");
  EXPECT_EQ("not.nothing.just.something.", name.to_string());
  EXPECT_FALSE(name.empty());
  EXPECT_EQ("not", name.first_label_view());
}

// Tests DnsName::append.
TEST(DnsMessageTest, Append) {
  DnsName name;
  EXPECT_TRUE(name.empty());

  name = name.append("chowder");
  EXPECT_EQ("chowder.", name.to_string());
  EXPECT_FALSE(name.empty());
  EXPECT_EQ("chowder", name.first_label_view());

  name = name.append("for");
  EXPECT_EQ("chowder.for.", name.to_string());
  EXPECT_FALSE(name.empty());
  EXPECT_EQ("chowder", name.first_label_view());

  name = name.append("lunch");
  EXPECT_EQ("chowder.for.lunch.", name.to_string());
  EXPECT_FALSE(name.empty());
  EXPECT_EQ("chowder", name.first_label_view());

  name = name.append(DnsName("and.little.else"));
  EXPECT_EQ("chowder.for.lunch.and.little.else.", name.to_string());
  EXPECT_FALSE(name.empty());
  EXPECT_EQ("chowder", name.first_label_view());
}

}  // namespace mdns::test
