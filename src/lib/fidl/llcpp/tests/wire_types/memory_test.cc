// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fidl/cpp/wire/array.h>
#include <lib/fidl/cpp/wire/object_view.h>
#include <lib/fidl/cpp/wire/string_view.h>
#include <lib/fidl/cpp/wire/vector_view.h>
#include <lib/zx/event.h>

#include <array>

#include <fbl/string.h>
#include <gtest/gtest.h>

TEST(Memory, TrackingPointerUnowned) {
  uint32_t obj;
  auto ptr = fidl::ObjectView<uint32_t>::FromExternal(&obj);
  EXPECT_EQ(ptr.get(), &obj);
}

TEST(Memory, VectorViewUnownedArray) {
  uint32_t obj[1] = {1};
  auto vv = fidl::VectorView<uint32_t>::FromExternal(obj);
  EXPECT_EQ(vv.size(), std::size(obj));
  EXPECT_EQ(vv.data(), std::data(obj));
}

TEST(Memory, VectorViewUnownedFidlArray) {
  std::array<uint32_t, 1> obj = {1};
  auto vv = fidl::VectorView<uint32_t>::FromExternal(obj);
  EXPECT_EQ(vv.size(), std::size(obj));
  EXPECT_EQ(vv.data(), std::data(obj));
}

TEST(Memory, VectorViewUnownedStdVector) {
  std::vector<uint32_t> obj;
  obj.push_back(1);
  auto vv = fidl::VectorView<uint32_t>::FromExternal(obj);
  EXPECT_EQ(vv.size(), std::size(obj));
  EXPECT_EQ(vv.data(), std::data(obj));
}

TEST(Memory, StringViewUnownedStdString) {
  std::string str = "abcd";
  auto sv = fidl::StringView::FromExternal(str);
  EXPECT_EQ(sv.size(), str.size());
  EXPECT_EQ(sv.data(), str.data());
}

TEST(Memory, StringViewUnownedFblString) {
  fbl::String str = "abcd";
  auto sv = fidl::StringView::FromExternal(str);
  EXPECT_EQ(sv.size(), str.size());
  EXPECT_EQ(sv.data(), str.data());
}

TEST(Memory, StringViewUnownedStdStringView) {
  std::string_view str = "abcd";
  auto sv = fidl::StringView::FromExternal(str);
  EXPECT_EQ(sv.size(), str.size());
  EXPECT_EQ(sv.data(), str.data());
}

TEST(Memory, StringViewUnownedCharPtrLength) {
  const char* str = "abcd";
  constexpr size_t len = 2;
  fidl::StringView sv = fidl::StringView::FromExternal(str, len);
  EXPECT_EQ(sv.size(), len);
  EXPECT_EQ(sv.data(), str);
}

TEST(Memory, StringViewUnownedStringArray) {
  const char str[] = "abcd";
  fidl::StringView sv(str);
  EXPECT_EQ(sv.size(), strlen(str));
  EXPECT_EQ(sv.data(), str);
}

static_assert(std::is_constructible_v<fidl::StringView, const char[5]>);
static_assert(!std::is_constructible_v<fidl::StringView, char[5]>);

TEST(Memory, ObjectViewFromDoubleOwned) {
  fidl::Arena arena;
  auto ov = fidl::ObjectView(arena, 42.0);
  EXPECT_EQ(*ov, 42.0);
}

TEST(Memory, ObjectViewFromVectorViewOwned) {
  uint32_t obj[1] = {1};
  auto vv = fidl::VectorView<uint32_t>::FromExternal(obj);

  fidl::Arena arena;
  auto ov = fidl::ObjectView(arena, vv);
  EXPECT_EQ(ov->size(), std::size(obj));
  EXPECT_EQ(ov->data(), std::data(obj));
}

TEST(Memory, ObjectViewFromVectorViewOfMoveOnlyOwned) {
  zx::event obj[1];
  auto vv = fidl::VectorView<zx::event>::FromExternal(obj);

  fidl::Arena arena;
  auto ov = fidl::ObjectView(arena, vv);
  EXPECT_EQ(ov->size(), std::size(obj));
  EXPECT_EQ(ov->data(), std::data(obj));
}

TEST(Memory, ObjectViewFromStringViewOwned) {
  std::string str = "abcd";
  auto sv = fidl::StringView::FromExternal(str);

  fidl::Arena arena;
  auto ov = fidl::ObjectView(arena, sv);
  EXPECT_EQ(ov->size(), str.size());
  EXPECT_EQ(ov->data(), str.data());
}

TEST(Memory, ObjectViewPassArenaToVectorView) {
  std::vector<int32_t> vec = {1, 2, 3, 4};
  fidl::Arena arena;

  // By default, the same arena is reused.
  {
    auto ov = fidl::ObjectView<fidl::VectorView<int32_t>>(arena, vec);
    // std::span doesn't have deep equality so we compare manually.
    std::vector<int32_t> vec2;
    vec2.assign(ov->get().begin(), ov->get().end());
    EXPECT_EQ(vec2, vec);
    EXPECT_NE(ov->data(), vec.data());
  }

  // It should still be possible to override arenas.
  fidl::Arena arena2;
  {
    auto ov = fidl::ObjectView<fidl::VectorView<int32_t>>(arena, arena2, vec);
    // std::span doesn't have deep equality so we compare manually.
    std::vector<int32_t> vec2;
    vec2.assign(ov->get().begin(), ov->get().end());
    EXPECT_EQ(vec2, vec);
    EXPECT_NE(ov->data(), vec.data());
  }
}

TEST(Memory, ObjectViewPassArenaToStringView) {
  std::string str = "abcd";
  fidl::Arena arena;
  auto ov = fidl::ObjectView<fidl::StringView>(arena, str);
  EXPECT_EQ(ov->get(), str);
  EXPECT_NE(ov->data(), str.data());
}
