// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fxt/interned_category.h>

#include <set>

#include <gtest/gtest.h>

namespace {

TEST(Types, InternedCategory) {
  using fxt::operator""_category;

  const fxt::InternedCategory& foo = "foo"_category;
  const fxt::InternedCategory& bar = "bar"_category;
  const fxt::InternedCategory& foo2 = "foo"_category;

  EXPECT_EQ(&foo, &foo2);
  EXPECT_NE(&foo, &bar);

  EXPECT_EQ(&foo.label(), &foo2.label());
  EXPECT_NE(&foo.label(), &bar.label());

  EXPECT_STREQ(foo.string(), "foo");
  EXPECT_STREQ(bar.string(), "bar");

  EXPECT_EQ(foo.index(), fxt::InternedCategory::kInvalidIndex);
  EXPECT_EQ(bar.index(), fxt::InternedCategory::kInvalidIndex);

  // The category section is only populated on supported compilers.
  if (fxt::InternedCategory::section_begin() != fxt::InternedCategory::section_end()) {
    // Interned categories from other tests in the same test binary will show up here if any show up
    // at all. See if at least the target set is in the category section.
    std::set<const fxt::InternedCategory*> target_category_set{&foo, &bar};
    std::set<const fxt::InternedCategory*> section_category_set{};

    fxt::InternedCategory::SetRegisterCallback([&](const fxt::InternedCategory& interned_category) {
      static uint32_t next_index{1};

      if (interned_category.index() == fxt::InternedCategory::kInvalidIndex) {
        section_category_set.emplace(&interned_category);
        interned_category.SetIndex(next_index++);
      }
      return interned_category.index();
    });
    fxt::InternedCategory::RegisterCategories();

    EXPECT_FALSE(section_category_set.empty());
    for (const fxt::InternedCategory* string : target_category_set) {
      EXPECT_EQ(1u, section_category_set.count(string));
    }

    EXPECT_NE(foo.index(), fxt::InternedCategory::kInvalidIndex);
    EXPECT_NE(bar.index(), fxt::InternedCategory::kInvalidIndex);
    EXPECT_NE(bar.index(), foo.index());
  }
}

}  // anonymous namespace
