// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-protocols/cpp/inplace-vector.h"

#include <zircon/assert.h>

#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

namespace display::internal {

namespace {

TEST(InplaceVectorTest, InitializerListConstructor) {
  const InplaceVector<int, 10> vector = {40, 41, 42, 43};

  EXPECT_THAT(vector, ::testing::ElementsAre(40, 41, 42, 43));
}

TEST(InplaceVectorTest, EmplaceBack) {
  InplaceVector<int, 10> vector;

  int& emplace_back0_result = vector.emplace_back(40);
  int& emplace_back1_result = vector.emplace_back(41);
  int& emplace_back2_result = vector.emplace_back(42);
  int& emplace_back3_result = vector.emplace_back(43);

  EXPECT_EQ(&emplace_back0_result, vector.data());
  EXPECT_EQ(&emplace_back1_result, vector.data() + 1);
  EXPECT_EQ(&emplace_back2_result, vector.data() + 2);
  EXPECT_EQ(&emplace_back3_result, vector.data() + 3);

  EXPECT_THAT(vector, ::testing::ElementsAre(40, 41, 42, 43));
}

TEST(InplaceVectorTest, EmptyConst) {
  const InplaceVector<int, 10> empty_vector;
  EXPECT_EQ(empty_vector.empty(), true);
  EXPECT_EQ(empty_vector.size(), 0u);
}

TEST(InplaceVectorTest, Empty) {
  InplaceVector<int, 10> empty_vector;
  EXPECT_EQ(empty_vector.empty(), true);
  EXPECT_EQ(empty_vector.size(), 0u);
}

TEST(InplaceVectorTest, CapacityMethodsConst) {
  const InplaceVector<int, 10> vector = {40, 41, 42, 43, 44, 45};

  EXPECT_EQ(vector.empty(), false);
  EXPECT_EQ(vector.size(), 6u);
  EXPECT_EQ(vector.max_size(), 10u);
  EXPECT_EQ(vector.capacity(), 10u);
}

TEST(InplaceVectorTest, CapacityMethods) {
  InplaceVector<int, 10> vector = {40, 41, 42, 43, 44, 45};

  EXPECT_EQ(vector.empty(), false);
  EXPECT_EQ(vector.size(), 6u);
  EXPECT_EQ(vector.max_size(), 10u);
  EXPECT_EQ(vector.capacity(), 10u);
}

TEST(InplaceVectorTest, ElementAccessMethodsConst) {
  const InplaceVector<int, 10> vector = {40, 41, 42, 43, 44, 45};

  EXPECT_EQ(*vector.data(), 40);

  // NOLINTNEXTLINE(readability-container-data-pointer): Need to test [0].
  EXPECT_EQ(&vector[0], vector.data());
  EXPECT_EQ(&vector[1], vector.data() + 1);
  EXPECT_EQ(&vector[2], vector.data() + 2);

  EXPECT_EQ(&vector.front(), vector.data());
  EXPECT_EQ(&vector.back(), vector.data() + 5);
}

TEST(InplaceVectorTest, Data) {
  InplaceVector<int, 10> vector = {40, 41, 42, 43, 44, 45};

  *vector.data() = 80;
  EXPECT_THAT(vector, ::testing::ElementsAre(80, 41, 42, 43, 44, 45));
}

TEST(InplaceVectorTest, ArrayOperator) {
  InplaceVector<int, 10> vector = {40, 41, 42, 43, 44};

  ++vector[0];
  EXPECT_THAT(vector, ::testing::ElementsAre(41, 41, 42, 43, 44));

  vector[1] += 1000;
  EXPECT_THAT(vector, ::testing::ElementsAre(41, 1041, 42, 43, 44));

  vector[2] += 2000;
  EXPECT_THAT(vector, ::testing::ElementsAre(41, 1041, 2042, 43, 44));
}

TEST(InplaceVectorTest, Front) {
  InplaceVector<int, 10> vector = {40, 41, 42, 43, 44};

  EXPECT_EQ(&vector.front(), vector.data());

  vector.front() += 1000;
  EXPECT_THAT(vector, ::testing::ElementsAre(1040, 41, 42, 43, 44));
}

TEST(InplaceVectorTest, Back) {
  InplaceVector<int, 10> vector = {40, 41, 42, 43, 44};

  EXPECT_EQ(&vector.back(), vector.data() + 4);

  vector.back() += 1000;
  EXPECT_THAT(vector, ::testing::ElementsAre(40, 41, 42, 43, 1044));
}

TEST(InplaceVectorTest, RangeForLoopConst) {
  const InplaceVector<int, 10> vector = {40, 41, 42, 43};

  std::vector<int> iterated_values;
  for (const int& value : vector) {
    iterated_values.push_back(value);
  }
  EXPECT_THAT(iterated_values, ::testing::ElementsAre(40, 41, 42, 43));
}

TEST(InplaceVectorTest, IteratorMethodsConst) {
  const InplaceVector<int, 10> vector = {40, 41, 42, 43, 44};

  EXPECT_EQ(&(*vector.begin()), vector.data());
  EXPECT_EQ(&(*(++vector.begin())), vector.data() + 1);
  EXPECT_EQ(&(*(++(++vector.begin()))), vector.data() + 2);

  EXPECT_EQ(vector.end(), vector.begin() + 5);

  EXPECT_EQ(vector.cbegin(), vector.begin());
  EXPECT_EQ(vector.cend(), vector.end());

  EXPECT_EQ(&(*vector.rbegin()), vector.data() + 4);
  EXPECT_EQ(&(*(++vector.rbegin())), vector.data() + 3);
  EXPECT_EQ(&(*(++(++vector.rbegin()))), vector.data() + 2);
  EXPECT_EQ(vector.rend(), vector.rbegin() + 5);

  EXPECT_EQ(vector.crbegin(), vector.rbegin());
  EXPECT_EQ(vector.crend(), vector.rend());
}

TEST(InplaceVectorTest, RangeForLoop) {
  InplaceVector<int, 10> vector = {40, 41, 42, 43};

  for (int& value : vector) {
    value += 1000;
  }
  EXPECT_THAT(vector, ::testing::ElementsAre(1040, 1041, 1042, 1043));
}

TEST(InplaceVectorTest, IteratorMethods) {
  InplaceVector<int, 10> vector = {40, 41, 42, 43, 44};

  EXPECT_EQ(&(*vector.begin()), vector.data());
  EXPECT_EQ(&(*(++vector.begin())), vector.data() + 1);
  EXPECT_EQ(&(*(++(++vector.begin()))), vector.data() + 2);

  EXPECT_EQ(vector.end(), vector.begin() + 5);

  EXPECT_EQ(&(*vector.rbegin()), vector.data() + 4);
  EXPECT_EQ(&(*(++vector.rbegin())), vector.data() + 3);
  EXPECT_EQ(&(*(++(++vector.rbegin()))), vector.data() + 2);
  EXPECT_EQ(vector.rend(), vector.rbegin() + 5);
}

class DestructionLogger {
 public:
  explicit DestructionLogger(int id, std::vector<int>* destruction_log)
      : id_(id), destruction_log_(*destruction_log) {
    ZX_DEBUG_ASSERT(destruction_log != nullptr);
  }

  DestructionLogger(const DestructionLogger&) = delete;
  DestructionLogger& operator=(const DestructionLogger&) = delete;

  ~DestructionLogger() { destruction_log_.push_back(id_); }

 private:
  const int id_;
  std::vector<int>& destruction_log_;
};

TEST(InplaceVectorTest, DestructorOnlyCalledOnUsedElements) {
  std::vector<int> destruction_log;
  {
    InplaceVector<DestructionLogger, 10> vector;
    vector.emplace_back(1, &destruction_log);
    vector.emplace_back(2, &destruction_log);
    vector.emplace_back(3, &destruction_log);
  }
  EXPECT_THAT(destruction_log, ::testing::ElementsAre(1, 2, 3));
}

struct NoMove {
  explicit constexpr NoMove(int id) : id(id) {}

  NoMove(const NoMove&) = default;
  NoMove& operator=(const NoMove&) = default;
  NoMove(NoMove&&) = delete;
  NoMove& operator=(NoMove&&) = delete;

  int id;
};
constexpr bool operator==(const NoMove& lhs, const NoMove& rhs) { return lhs.id == rhs.id; }

TEST(InplaceVectorTest, PushBackCopy) {
  InplaceVector<NoMove, 10> vector;

  constexpr NoMove element1(1), element2(2), element3(3);
  vector.push_back(element1);
  vector.push_back(element2);
  vector.push_back(element3);

  EXPECT_THAT(vector, ::testing::ElementsAre(element1, element2, element3));
}

struct MoveOnly {
  explicit constexpr MoveOnly(int id) : id(id) {}

  MoveOnly(const MoveOnly&) = delete;
  MoveOnly& operator=(const MoveOnly&) = delete;

  MoveOnly(MoveOnly&& rhs) : id(rhs.id) { rhs.id = 0; }

  MoveOnly& operator=(MoveOnly&& rhs) {
    id = rhs.id;
    rhs.id = 0;
    return *this;
  }

  int id;
};

TEST(InplaceVectorTest, PushBackMove) {
  InplaceVector<MoveOnly, 10> vector;

  MoveOnly element1(1), element2(2), element3(3);
  vector.push_back(std::move(element1));
  vector.push_back(std::move(element2));
  vector.push_back(std::move(element3));

  EXPECT_EQ(vector[0].id, 1);
  EXPECT_EQ(vector[1].id, 2);
  EXPECT_EQ(vector[2].id, 3);

  EXPECT_EQ(element1.id, 0);
  EXPECT_EQ(element2.id, 0);
  EXPECT_EQ(element3.id, 0);
}

TEST(InplaceVectorTest, Clear) {
  std::vector<int> destruction_log;
  {
    InplaceVector<DestructionLogger, 10> vector;
    vector.emplace_back(1, &destruction_log);
    vector.emplace_back(2, &destruction_log);
    vector.emplace_back(3, &destruction_log);

    vector.clear();
    EXPECT_THAT(vector, ::testing::IsEmpty());
    EXPECT_THAT(destruction_log, ::testing::ElementsAre(1, 2, 3));
    destruction_log.clear();
  }
  EXPECT_THAT(destruction_log, ::testing::IsEmpty());
}

}  // namespace

}  // namespace display::internal
