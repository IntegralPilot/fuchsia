// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/gnu-hash.h>
#include <lib/elfldltl/soname.h>

#include <optional>
#include <string>
#include <string_view>
#include <vector>

#include <gtest/gtest.h>

namespace {

constexpr std::string_view kOther = "other";
constexpr uint32_t kOtherHash = elfldltl::GnuHashString(kOther);

TEST(ElfldltlSonameTests, Basic) {
  elfldltl::Soname name{"test"};
  EXPECT_EQ(name.str(), "test");

  elfldltl::Soname other{"other"};
  EXPECT_EQ(other.str(), "other");

  name = other;
  EXPECT_EQ(name.str(), "other");
  EXPECT_EQ(other, name);
  EXPECT_EQ(other.hash(), kOtherHash);

  // Test assignment to std::string_view.
  name = kOther;
  EXPECT_EQ(name.str(), kOther);
  EXPECT_EQ(other, name);
  EXPECT_EQ(other.hash(), kOtherHash);

  // Test converting assignment to std::string_view.
  name = "other";
  EXPECT_EQ(name.str(), kOther);
  EXPECT_EQ(other, name);
  EXPECT_EQ(other.hash(), kOtherHash);

  // Test str() and size() methods.
  EXPECT_EQ(name.str().size(), kOther.size());
  EXPECT_EQ(name.size(), kOther.size());

  // Test copy() when truncating.
  std::array<char, 3> short_buffer;
  size_t n = name.copy(short_buffer.data(), short_buffer.size(), 1);
  EXPECT_EQ(n, short_buffer.size());
  EXPECT_EQ(std::string_view(short_buffer.data(), n), name.str().substr(1, n));

  // Test copy() when not truncating.
  std::array<char, 10> long_buffer;
  ASSERT_GT(long_buffer.size(), name.str().size());
  n = name.copy(long_buffer.data(), long_buffer.size());
  EXPECT_EQ(n, name.str().size() + 1);
  EXPECT_EQ(long_buffer[name.str().size()], '\0');
  EXPECT_STREQ(long_buffer.data(), name.c_str());

  // Test using copy_size().
  std::vector<char> buffer(name.copy_size());
  n = name.copy(buffer.data(), buffer.size());
  EXPECT_EQ(n, name.str().size() + 1);
  EXPECT_EQ(buffer[name.str().size()], '\0');
  EXPECT_STREQ(buffer.data(), name.c_str());

  elfldltl::Soname a{"a"}, b{"b"};
  EXPECT_LT(a, b);
  EXPECT_LE(a, b);
  EXPECT_LE(a, a);
  EXPECT_GT(b, a);
  EXPECT_GE(b, a);
  EXPECT_GE(a, a);
  EXPECT_EQ(a, a);
  EXPECT_NE(a, b);
}

TEST(ElfldltlSonameTests, Remote) {
  using RemoteSoname = elfldltl::Soname<elfldltl::Elf<>, elfldltl::RemoteAbiTraits>;

  RemoteSoname name;
  name = RemoteSoname(name);
  EXPECT_EQ(name.hash(), 0u);
}

}  // anonymous namespace
