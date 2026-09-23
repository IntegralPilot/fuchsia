// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fidl/cpp/wire/vector_view.h>

#include <array>
#include <vector>

#include <gtest/gtest.h>

namespace {

TEST(VectorView, DefaultConstructor) {
  fidl::VectorView<int32_t> vv;
  EXPECT_EQ(vv.size(), 0u);
  EXPECT_TRUE(vv.empty());
  EXPECT_EQ(vv.data(), nullptr);
}

struct DestructionState {
  bool destructor_called = false;
};
struct DestructableObject {
  DestructableObject() : ds(nullptr) {}
  DestructableObject(DestructionState* ds) : ds(ds) {}

  ~DestructableObject() { ds->destructor_called = true; }

  DestructionState* ds;
};

TEST(VectorView, PointerConstructor) {
  DestructionState ds[3] = {};
  DestructableObject arr[3] = {&ds[0], &ds[1], &ds[2]};
  {
    auto vv = fidl::VectorView<DestructableObject>::FromExternal(arr, 2);
    EXPECT_EQ(vv.size(), 2u);
    EXPECT_FALSE(vv.empty());
    EXPECT_EQ(vv.data(), arr);
  }
  EXPECT_FALSE(ds[0].destructor_called);
  EXPECT_FALSE(ds[1].destructor_called);
  EXPECT_FALSE(ds[1].destructor_called);
}

// Used as container element type to ensure that VectorView does not attempt to
// copy or move the unowned container's elements.
struct NoCopyNoMove {
  int data;

  explicit constexpr NoCopyNoMove(int data) : data(data) {}
  constexpr ~NoCopyNoMove() = default;

  NoCopyNoMove(const NoCopyNoMove&) = delete;
  NoCopyNoMove& operator=(const NoCopyNoMove&) = delete;
};

TEST(VectorView, CopyConstructor) {
  std::array<NoCopyNoMove, 3> array{NoCopyNoMove(1), NoCopyNoMove(2), NoCopyNoMove(3)};
  auto copy_source = fidl::VectorView<NoCopyNoMove>::FromExternal(array);

  fidl::VectorView<NoCopyNoMove> copy_target(copy_source);
  EXPECT_EQ(copy_target.size(), 3u);
  EXPECT_EQ(copy_target.data(), array.data());
  EXPECT_EQ(copy_source.size(), 3u);
  EXPECT_EQ(copy_source.data(), array.data());
}

TEST(VectorView, CopyAssignment) {
  std::array<NoCopyNoMove, 3> array{NoCopyNoMove(1), NoCopyNoMove(2), NoCopyNoMove(3)};
  auto copy_source = fidl::VectorView<NoCopyNoMove>::FromExternal(array);

  fidl::VectorView<NoCopyNoMove> copy_target = copy_source;
  EXPECT_EQ(copy_target.size(), 3u);
  EXPECT_EQ(copy_target.data(), array.data());
  EXPECT_EQ(copy_source.size(), 3u);
  EXPECT_EQ(copy_source.data(), array.data());
}

TEST(VectorView, CopyConstructorWithConst) {
  std::array<NoCopyNoMove, 3> array{NoCopyNoMove(1), NoCopyNoMove(2), NoCopyNoMove(3)};
  auto copy_source = fidl::VectorView<NoCopyNoMove>::FromExternal(array);

  fidl::VectorView<const NoCopyNoMove> copy_target(copy_source);
  EXPECT_EQ(copy_target.size(), 3u);
  EXPECT_EQ(copy_target.data(), array.data());
  EXPECT_EQ(copy_source.size(), 3u);
  EXPECT_EQ(copy_source.data(), array.data());
}

TEST(VectorView, CopyAssignmentWithConst) {
  std::array<NoCopyNoMove, 3> array{NoCopyNoMove(1), NoCopyNoMove(2), NoCopyNoMove(3)};
  auto copy_source = fidl::VectorView<NoCopyNoMove>::FromExternal(array);

  fidl::VectorView<const NoCopyNoMove> copy_target = copy_source;
  EXPECT_EQ(copy_target.size(), 3u);
  EXPECT_EQ(copy_target.data(), array.data());
  EXPECT_EQ(copy_source.size(), 3u);
  EXPECT_EQ(copy_source.data(), array.data());
}

TEST(VectorView, MoveConstructor) {
  std::array<NoCopyNoMove, 3> array{NoCopyNoMove(1), NoCopyNoMove(2), NoCopyNoMove(3)};
  auto move_source = fidl::VectorView<NoCopyNoMove>::FromExternal(array);

  // NOLINTNEXTLINE(performance-move-const-arg): Testing that move works.
  fidl::VectorView<NoCopyNoMove> move_target = std::move(move_source);
  EXPECT_EQ(move_target.size(), 3u);
  EXPECT_EQ(move_target.data(), array.data());
  EXPECT_EQ(move_source.size(), 3u);
  EXPECT_EQ(move_source.data(), array.data());
}

TEST(VectorView, MoveAssignment) {
  std::array<NoCopyNoMove, 3> array{NoCopyNoMove(1), NoCopyNoMove(2), NoCopyNoMove(3)};
  auto move_source = fidl::VectorView<NoCopyNoMove>::FromExternal(array);

  // NOLINTNEXTLINE(performance-move-const-arg): Testing that move works.
  fidl::VectorView<NoCopyNoMove> move_target = std::move(move_source);
  EXPECT_EQ(move_target.size(), 3u);
  EXPECT_EQ(move_target.data(), array.data());
  EXPECT_EQ(move_source.size(), 3u);
  EXPECT_EQ(move_source.data(), array.data());
}

TEST(VectorView, MoveConstructorWithConst) {
  std::array<NoCopyNoMove, 3> array{NoCopyNoMove(1), NoCopyNoMove(2), NoCopyNoMove(3)};
  auto move_source = fidl::VectorView<NoCopyNoMove>::FromExternal(array);

  // NOLINTNEXTLINE(performance-move-const-arg): Testing that move works.
  fidl::VectorView<const NoCopyNoMove> move_target = std::move(move_source);
  EXPECT_EQ(move_target.size(), 3u);
  EXPECT_EQ(move_target.data(), array.data());
  EXPECT_EQ(move_source.size(), 3u);
  EXPECT_EQ(move_source.data(), array.data());
}

TEST(VectorView, MoveAssignmentWithConst) {
  std::array<NoCopyNoMove, 3> array{NoCopyNoMove(1), NoCopyNoMove(2), NoCopyNoMove(3)};
  auto move_source = fidl::VectorView<NoCopyNoMove>::FromExternal(array);

  // NOLINTNEXTLINE(performance-move-const-arg): Testing that move works.
  fidl::VectorView<const NoCopyNoMove> move_target = std::move(move_source);
  EXPECT_EQ(move_target.size(), 3u);
  EXPECT_EQ(move_target.data(), array.data());
  EXPECT_EQ(move_source.size(), 3u);
  EXPECT_EQ(move_source.data(), array.data());
}

TEST(VectorView, Iteration) {
  std::vector<int32_t> vec{1, 2, 3};
  auto vv = fidl::VectorView<int32_t>::FromExternal(vec);
  int32_t i = 1;
  for (auto& val : vv) {
    EXPECT_EQ(&val, &vec.at(i - 1));
    ++i;
  }
  EXPECT_EQ(i, 4);
}

TEST(VectorView, Indexing) {
  std::vector<int32_t> vec{1, 2, 3};
  auto vv = fidl::VectorView<int32_t>::FromExternal(vec);
  for (uint64_t i = 0; i < vv.size(); i++) {
    EXPECT_EQ(&vv[i], &vec.at(i));
  }
}

TEST(VectorView, Mutations) {
  std::vector<int32_t> vec{1, 2, 3};
  auto vv = fidl::VectorView<int32_t>::FromExternal(vec);
  vv.set_size(2);
  *vv.data() = 4;
  vv[1] = 5;
  EXPECT_EQ(vv.size(), 2u);
  EXPECT_EQ(vv.data(), vec.data());
  EXPECT_EQ(vv[0], 4);
  EXPECT_EQ(vv[1], 5);
  EXPECT_EQ(vec[0], 4);
  EXPECT_EQ(vec[1], 5);
}

TEST(VectorView, CopyFromStdVector) {
  fidl::Arena arena;
  std::vector<int32_t> vec{1, 2, 3};
  fidl::VectorView vv{arena, vec};
  vec[0] = 0;
  vec[1] = 0;
  vec[2] = 0;
  EXPECT_EQ(vv.size(), 3u);
  EXPECT_EQ(vv[0], 1);
  EXPECT_EQ(vv[1], 2);
  EXPECT_EQ(vv[2], 3);
}

TEST(VectorView, CopyFromStdSpan) {
  fidl::Arena arena;
  std::vector<int32_t> vec{1, 2, 3};
  cpp20::span<int32_t> span{vec};
  fidl::VectorView vv{arena, span};
  vec[0] = 0;
  vec[1] = 0;
  vec[2] = 0;
  EXPECT_EQ(vv.size(), 3u);
  EXPECT_EQ(vv[0], 1);
  EXPECT_EQ(vv[1], 2);
  EXPECT_EQ(vv[2], 3);
}

TEST(VectorView, CopyFromConstStdSpan) {
  fidl::Arena arena;
  std::vector<int32_t> vec{1, 2, 3};
  cpp20::span<const int32_t> span{vec};
  fidl::VectorView vv{arena, span};
  vec[0] = 0;
  vec[1] = 0;
  vec[2] = 0;
  EXPECT_EQ(vv.size(), 3u);
  EXPECT_EQ(vv[0], 1);
  EXPECT_EQ(vv[1], 2);
  EXPECT_EQ(vv[2], 3);
}

TEST(VectorView, CopyFromIterators) {
  fidl::Arena arena;
  std::vector<int32_t> vec{1, 2, 3};
  cpp20::span<int32_t> span{vec};
  fidl::VectorView<int32_t> vv{arena, span.begin(), span.end()};
  vec[0] = 0;
  vec[1] = 0;
  vec[2] = 0;
  EXPECT_EQ(vv.size(), 3u);
  EXPECT_EQ(vv[0], 1);
  EXPECT_EQ(vv[1], 2);
  EXPECT_EQ(vv[2], 3);
}

TEST(VectorView, CopyFromConstIterators) {
  fidl::Arena arena;
  std::vector<int32_t> vec{1, 2, 3};
  cpp20::span<const int32_t> span{vec};
  fidl::VectorView<int32_t> vv{arena, span.begin(), span.end()};
  vec[0] = 0;
  vec[1] = 0;
  vec[2] = 0;
  EXPECT_EQ(vv.size(), 3u);
  EXPECT_EQ(vv[0], 1);
  EXPECT_EQ(vv[1], 2);
  EXPECT_EQ(vv[2], 3);
}

#if 0
TEST(VectorView, BadIterators) {
  fidl::Arena arena;
  fidl::VectorView<int32_t> vv{arena, 1, 2};
}
#endif

}  // namespace
