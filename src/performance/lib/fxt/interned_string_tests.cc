// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fxt/interned_string.h>

#include <set>

#include <gtest/gtest.h>

namespace {

TEST(Types, InternedString) {
  using fxt::operator""_intern;

  const fxt::InternedString& foo = "foo"_intern;
  const fxt::InternedString& bar = "bar"_intern;
  const fxt::InternedString& foo2 = "foo"_intern;

  EXPECT_EQ(&foo, &foo2);
  EXPECT_NE(&bar, &foo);

  EXPECT_STREQ(foo.string(), "foo");
  EXPECT_STREQ(bar.string(), "bar");

  // The string section is only populated on supported compilers.
  if (fxt::InternedString::section_begin() != fxt::InternedString::section_end()) {
    // Interned strings from other tests in the same test binary will show up here if any show up at
    // all. See if at least the target set is in the string section.
    const std::set<const fxt::InternedString*> target_string_set{&foo, &bar};
    std::set<const fxt::InternedString*> section_string_set{};

    fxt::InternedString::SetRegisterCallback(
        [&section_string_set](const fxt::InternedString& string) {
          section_string_set.emplace(&string);
        });
    fxt::InternedString::RegisterStrings();

    EXPECT_FALSE(section_string_set.empty());
    for (const fxt::InternedString* string : target_string_set) {
      EXPECT_EQ(1u, section_string_set.count(string));
    }

    EXPECT_NE(foo.id(), fxt::InternedString::kInvalidId);
    EXPECT_NE(bar.id(), fxt::InternedString::kInvalidId);
    EXPECT_NE(bar.id(), foo.id());
  }
}

}  // anonymous namespace
