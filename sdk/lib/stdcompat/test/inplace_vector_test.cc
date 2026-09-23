// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/stdcompat/inplace_vector.h>

#include <algorithm>
#include <list>
#include <memory>

#include "gtest.h"

namespace {
struct Counter {
  inline static int move_count = 0;
  inline static int destruct_count = 0;
  Counter() = default;
  Counter(const Counter&) = default;
  Counter(Counter&&) { ++move_count; }
  ~Counter() { ++destruct_count; }
  Counter& operator=(Counter&&) {
    ++move_count;
    return *this;
  }
  Counter& operator=(const Counter&) = default;
};

struct S {
  int x;
  S(int v) : x(v) {}
};

struct Tracker {
  inline static int count = 0;
  Tracker() { ++count; }
  Tracker(const Tracker&) { ++count; }
  ~Tracker() { --count; }
};

// Test safety violations: capacity overflow, out-of-range access, and death test scenarios.
TEST(InplaceVectorTest, SafetyViolationDeathTests) {
  EXPECT_DEATH((cpp26::inplace_vector<int, 2>{1, 2, 3}), "");
  EXPECT_DEATH((cpp26::inplace_vector<int, 3>{1, 2, 3, 4, 5}), "");

  cpp26::inplace_vector<int, 1> v;
  v.push_back(1);
  EXPECT_DEATH(v.push_back(2), "");

  cpp26::inplace_vector<int, 1> v2;
  v2.emplace_back(1);
  EXPECT_DEATH(v2.emplace_back(2), "");

  EXPECT_DEATH((cpp26::inplace_vector<int, 2>(5)), "");
  EXPECT_DEATH((cpp26::inplace_vector<int, 2>(3, 42)), "");

  std::vector<int> source{1, 2, 3, 4, 5};
  EXPECT_DEATH((cpp26::inplace_vector<int, 3>(source.begin(), source.end())), "");
  EXPECT_DEATH((cpp26::inplace_vector<int, 3>(cpp23::from_range, source)), "");

  cpp26::inplace_vector<int, 2> v3;
  std::vector<int> source2{1, 2, 3, 4};
  EXPECT_DEATH(v3.assign(source2.begin(), source2.end()), "");
  EXPECT_DEATH(v3.assign(5, 42), "");
  EXPECT_DEATH(v3.assign({1, 2, 3, 4}), "");
  EXPECT_DEATH(v3.assign_range(source2), "");

  cpp26::inplace_vector<int, 3> v4{1, 2};
  EXPECT_DEATH(v4.resize(5), "");
  EXPECT_DEATH(v4.resize(5, 42), "");

  cpp26::inplace_vector<int, 3> v5{1, 2, 3};
  EXPECT_DEATH(v5.insert(v5.begin(), 42), "");
  EXPECT_DEATH(v5.insert(v5.begin(), 2, 42), "");
  std::vector<int> source3{4, 5};
  EXPECT_DEATH(v5.insert(v5.begin(), source3.begin(), source3.end()), "");
  EXPECT_DEATH(v5.insert(v5.begin(), {4, 5}), "");
  EXPECT_DEATH(v5.insert_range(v5.begin(), source3), "");
  EXPECT_DEATH(v5.emplace(v5.begin(), 42), "");

  cpp26::inplace_vector<int, 3> v6{1, 2};
  std::vector<int> source4{3, 4, 5};
  EXPECT_DEATH(v6.append_range(source4), "");

  auto test_assignment_overflow = []() {
    [[maybe_unused]] cpp26::inplace_vector<int, 3> v_small;
    cpp26::inplace_vector<int, 3> v_large{1, 2, 3, 4, 5};
  };
  EXPECT_DEATH(test_assignment_overflow(), "");

  cpp26::inplace_vector<int, 2> v7{1};
  EXPECT_DEATH(v7.at(1), "");
  const auto& cv7 = v7;
  EXPECT_DEATH(cv7.at(2), "");
}

// Test object lifecycle management: move operations, destruction counting, and complex types.
TEST(InplaceVectorTest, ObjectLifecycleManagement) {
  Counter::move_count = 0;
  Counter::destruct_count = 0;

  {
    cpp26::inplace_vector<Counter, 3> v1;
    v1.emplace_back();
    v1.emplace_back();
    v1.emplace_back();

    cpp26::inplace_vector<Counter, 3> v2;
    v2.emplace_back();
    v2.emplace_back();
    v2.emplace_back();

    EXPECT_EQ(Counter::destruct_count, 0);

    int move_count_before = Counter::move_count;
    int destruct_count_before = Counter::destruct_count;

    v2 = std::move(v1);

    EXPECT_EQ(Counter::move_count, move_count_before + 3);
    EXPECT_GE(Counter::destruct_count, destruct_count_before + 3);

    EXPECT_TRUE(v1.empty());
    EXPECT_EQ(v2.size(), 3u);
  }

  EXPECT_GT(Counter::destruct_count, 0);

  Tracker::count = 0;

  cpp26::inplace_vector<Tracker, 3> v3;
  v3.emplace_back();
  v3.emplace_back();
  EXPECT_EQ(Tracker::count, 2);
  v3.clear();
  EXPECT_EQ(Tracker::count, 0);

  {
    cpp26::inplace_vector<Tracker, 3> v4;
    v4.emplace_back();
    v4.emplace_back();
    v4.emplace_back();
    EXPECT_EQ(Tracker::count, 3);
  }

  EXPECT_EQ(Tracker::count, 0);
}

// Test basic vector operations and various constructor forms.
TEST(InplaceVectorTest, BasicOperationsAndConstruction) {
  cpp26::inplace_vector<int, 4> v;
  EXPECT_TRUE(v.empty());
  EXPECT_EQ(v.size(), 0u);
  EXPECT_EQ(v.capacity(), 4u);
  EXPECT_EQ(v.max_size(), 4u);

  v.push_back(1);
  v.push_back(2);
  v.push_back(3);
  EXPECT_EQ(v.size(), 3u);
  EXPECT_FALSE(v.empty());
  EXPECT_EQ(v[0], 1);
  EXPECT_EQ(v[1], 2);
  EXPECT_EQ(v[2], 3);
  EXPECT_EQ(v.front(), 1);
  EXPECT_EQ(v.back(), 3);

  v.pop_back();
  EXPECT_EQ(v.size(), 2u);
  EXPECT_EQ(v.back(), 2);

  v.clear();
  EXPECT_TRUE(v.empty());
  v.push_back(5);
  EXPECT_EQ(v.size(), 1u);
  EXPECT_EQ(v[0], 5);
  v.clear();

  cpp26::inplace_vector<int, 4> v_init{4, 5, 6};
  EXPECT_EQ(v_init.size(), 3u);
  EXPECT_EQ(v_init[0], 4);
  EXPECT_EQ(v_init[1], 5);
  EXPECT_EQ(v_init[2], 6);

  cpp26::inplace_vector<int, 4> v_copy(v_init);
  EXPECT_EQ(v_copy.size(), 3u);
  EXPECT_EQ(v_copy[1], 5);

  v = v_init;
  EXPECT_EQ(v.size(), 3u);
  EXPECT_EQ(v[2], 6);

  cpp26::inplace_vector<int, 5> v1(3);
  EXPECT_EQ(v1.size(), 3u);
  EXPECT_EQ(v1[0], 0);
  EXPECT_EQ(v1[1], 0);
  EXPECT_EQ(v1[2], 0);

  cpp26::inplace_vector<int, 5> v2(3, 42);
  EXPECT_EQ(v2.size(), 3u);
  EXPECT_EQ(v2[0], 42);
  EXPECT_EQ(v2[1], 42);
  EXPECT_EQ(v2[2], 42);

  std::vector<int> source{1, 2, 3};
  cpp26::inplace_vector<int, 4> v3(source.begin(), source.end());
  EXPECT_EQ(v3.size(), 3u);
  EXPECT_EQ(v3[0], 1);
  EXPECT_EQ(v3[1], 2);
  EXPECT_EQ(v3[2], 3);

  std::vector<int> source2{7, 8, 9};
  cpp26::inplace_vector<int, 4> v4(cpp23::from_range, source2);
  EXPECT_EQ(v4.size(), 3u);
  EXPECT_EQ(v4[0], 7);
  EXPECT_EQ(v4[1], 8);
  EXPECT_EQ(v4[2], 9);

  cpp26::inplace_vector<S, 2> v5;
  v5.emplace_back(42);
  EXPECT_EQ(v5[0].x, 42);

  cpp26::inplace_vector<int, 3> v6{1, 2, 3};
  int sum = 0;
  std::for_each(v6.begin(), v6.end(), [&](int x) { sum += x; });
  EXPECT_EQ(sum, 6);
  std::reverse(v6.begin(), v6.end());
  EXPECT_EQ(v6[0], 3);
  EXPECT_EQ(v6[2], 1);
}

// Test iterator functionality: front/back/data access, reverse iterators, and validity.
TEST(InplaceVectorTest, IteratorFunctionality) {
  cpp26::inplace_vector<int, 3> v{7, 8, 9};
  EXPECT_EQ(v.front(), 7);
  EXPECT_EQ(v.back(), 9);
  EXPECT_EQ(v.data()[1], 8);
  int sum = 0;
  for (auto it = v.begin(); it != v.end(); ++it) {
    sum += *it;
  }
  EXPECT_EQ(sum, 24);
  const auto& cv = v;
  EXPECT_EQ(*(cv.cbegin()), 7);
  EXPECT_EQ(*(cv.cend() - 1), 9);
  EXPECT_EQ(cv.front(), 7);
  EXPECT_EQ(cv.back(), 9);

  cpp26::inplace_vector<int, 2> v2;
  v2.push_back(42);
  EXPECT_EQ(v2.front(), 42);
  EXPECT_EQ(v2.back(), 42);

  v2.push_back(99);
  EXPECT_EQ(v2.front(), 42);
  EXPECT_EQ(v2.back(), 99);

  cpp26::inplace_vector<int, 5> v3{1, 2, 3};
  auto rit = v3.rbegin();
  EXPECT_EQ(*rit, 3);
  ++rit;
  EXPECT_EQ(*rit, 2);
  ++rit;
  EXPECT_EQ(*rit, 1);
  ++rit;
  EXPECT_EQ(rit, v3.rend());

  const auto& cv3 = v3;
  auto crit = cv3.crbegin();
  EXPECT_EQ(*crit, 3);
  EXPECT_EQ(cv3.crend() - cv3.crbegin(), 3);

  cpp26::inplace_vector<int, 5> v4{1, 2, 3};
  auto it = v4.begin() + 1;
  EXPECT_EQ(*it, 2);

  v4.push_back(4);
  EXPECT_EQ(*it, 2);

  v4.insert(v4.begin(), 0);
  EXPECT_EQ(v4.size(), 5u);
}

// Test move-only types like std::unique_ptr and move semantics.
TEST(InplaceVectorTest, MoveOnlyTypeAndMoveSemantics) {
  cpp26::inplace_vector<std::unique_ptr<int>, 2> v;
  v.push_back(std::make_unique<int>(42));
  v.emplace_back(new int(99));
  EXPECT_EQ(*v[0], 42);
  EXPECT_EQ(*v[1], 99);

  cpp26::inplace_vector<std::unique_ptr<int>, 3> v1;
  v1.emplace_back(std::make_unique<int>(42));
  v1.emplace_back(std::make_unique<int>(99));

  auto original_size = v1.size();
  cpp26::inplace_vector<std::unique_ptr<int>, 3> v2(std::move(v1));
  EXPECT_EQ(v2.size(), original_size);
  EXPECT_EQ(*v2[0], 42);
  EXPECT_EQ(*v2[1], 99);
  EXPECT_TRUE(v1.empty());

  cpp26::inplace_vector<std::unique_ptr<int>, 3> v3;
  v3.emplace_back(std::make_unique<int>(1));
  v3.emplace_back(std::make_unique<int>(2));

  cpp26::inplace_vector<std::unique_ptr<int>, 3> v4;
  v4.emplace_back(std::make_unique<int>(99));

  v4 = std::move(v3);
  EXPECT_EQ(v4.size(), 2u);
  EXPECT_EQ(*v4[0], 1);
  EXPECT_EQ(*v4[1], 2);
  EXPECT_TRUE(v3.empty());

  cpp26::inplace_vector<std::unique_ptr<int>, 2> v5(std::move(v));
  EXPECT_EQ(*v5[0], 42);
  EXPECT_EQ(*v5[1], 99);

  cpp26::inplace_vector<std::unique_ptr<int>, 2> v6;
  v6 = std::move(v5);
  EXPECT_EQ(*v6[0], 42);
  EXPECT_EQ(*v6[1], 99);
}

// Test try_*, unchecked_*, and return value operations for insertion methods.
TEST(InplaceVectorTest, SafeAndUnsafeInsertionOperations) {
  cpp26::inplace_vector<int, 3> v1;
  auto* result1 = v1.try_push_back(42);
  ASSERT_NE(result1, nullptr);
  EXPECT_EQ(*result1, 42);
  EXPECT_EQ(v1.size(), 1u);

  auto* result2 = v1.try_push_back(std::move(43));
  ASSERT_NE(result2, nullptr);
  EXPECT_EQ(*result2, 43);
  EXPECT_EQ(v1.size(), 2u);

  cpp26::inplace_vector<int, 1> v2;
  v2.push_back(1);
  auto* result3 = v2.try_push_back(2);
  EXPECT_EQ(result3, nullptr);
  EXPECT_EQ(v2.size(), 1u);

  cpp26::inplace_vector<S, 2> v3;
  auto* result4 = v3.try_emplace_back(99);
  ASSERT_NE(result4, nullptr);
  EXPECT_EQ(result4->x, 99);
  EXPECT_EQ(v3.size(), 1u);

  cpp26::inplace_vector<S, 1> v4;
  v4.emplace_back(1);
  auto* result5 = v4.try_emplace_back(2);
  EXPECT_EQ(result5, nullptr);
  EXPECT_EQ(v4.size(), 1u);

  cpp26::inplace_vector<int, 3> v5;
  auto& ref1 = v5.unchecked_push_back(10);
  EXPECT_EQ(ref1, 10);
  EXPECT_EQ(v5.size(), 1u);

  auto& ref2 = v5.unchecked_push_back(std::move(20));
  EXPECT_EQ(ref2, 20);
  EXPECT_EQ(v5.size(), 2u);

  cpp26::inplace_vector<S, 2> v6;
  auto& ref3 = v6.unchecked_emplace_back(77);
  EXPECT_EQ(ref3.x, 77);
  EXPECT_EQ(v6.size(), 1u);

  cpp26::inplace_vector<int, 2> v7;
  auto& ref4 = v7.push_back(5);
  EXPECT_EQ(ref4, 5);
  EXPECT_EQ(&ref4, &v7.back());

  auto& ref5 = v7.push_back(std::move(10));
  EXPECT_EQ(ref5, 10);
  EXPECT_EQ(&ref5, &v7.back());

  cpp26::inplace_vector<S, 2> v8;
  auto& ref6 = v8.emplace_back(33);
  EXPECT_EQ(ref6.x, 33);
  EXPECT_EQ(&ref6, &v8.back());
}

// Test comprehensive range operations: append_range, assign_range, and insert_range.
TEST(InplaceVectorTest, RangeOperations) {
  cpp26::inplace_vector<int, 5> v{1, 2};
  std::vector<int> extra{3, 4};
  v.append_range(extra);
  EXPECT_EQ(v.size(), 4u);
  EXPECT_EQ(v[0], 1);
  EXPECT_EQ(v[1], 2);
  EXPECT_EQ(v[2], 3);
  EXPECT_EQ(v[3], 4);

  cpp26::inplace_vector<int, 5> v1{1, 2};
  std::vector<int> empty_range;
  v1.append_range(empty_range);
  EXPECT_EQ(v1.size(), 2u);
  EXPECT_EQ(v1[0], 1);
  EXPECT_EQ(v1[1], 2);

  cpp26::inplace_vector<int, 5> v2;
  std::vector<int> source{3, 4, 5};
  v2.append_range(source);
  EXPECT_EQ(v2.size(), 3u);
  EXPECT_EQ(v2[0], 3);
  EXPECT_EQ(v2[1], 4);
  EXPECT_EQ(v2[2], 5);

  std::list<int> list_source{6, 7};
  v2.append_range(list_source);
  EXPECT_EQ(v2.size(), 5u);
  EXPECT_EQ(v2[3], 6);
  EXPECT_EQ(v2[4], 7);

  cpp26::inplace_vector<int, 5> v3{1, 2, 3};
  std::vector<int> assign_source{4, 5, 6};
  v3.assign_range(assign_source);
  EXPECT_EQ(v3.size(), 3u);
  EXPECT_EQ(v3[0], 4);
  EXPECT_EQ(v3[1], 5);
  EXPECT_EQ(v3[2], 6);

  cpp26::inplace_vector<int, 5> v4{1, 2, 3};
  v4.assign_range(empty_range);
  EXPECT_EQ(v4.size(), 0u);
  EXPECT_TRUE(v4.empty());

  cpp26::inplace_vector<int, 6> v5{1, 4};
  std::vector<int> insert_source{2, 3};
  auto it = v5.insert_range(v5.begin() + 1, insert_source);
  EXPECT_EQ(v5.size(), 4u);
  EXPECT_EQ(v5[0], 1);
  EXPECT_EQ(v5[1], 2);
  EXPECT_EQ(v5[2], 3);
  EXPECT_EQ(v5[3], 4);
  EXPECT_EQ(it, v5.begin() + 1);
}

// Test resize operations with and without default values, including edge cases.
TEST(InplaceVectorTest, ResizeOperations) {
  cpp26::inplace_vector<int, 5> v{1, 2};
  v.resize(4);
  EXPECT_EQ(v.size(), 4u);
  EXPECT_EQ(v[0], 1);
  EXPECT_EQ(v[1], 2);
  EXPECT_EQ(v[2], 0);
  EXPECT_EQ(v[3], 0);

  v.resize(2);
  EXPECT_EQ(v.size(), 2u);
  EXPECT_EQ(v[0], 1);
  EXPECT_EQ(v[1], 2);

  v.resize(4, 99);
  EXPECT_EQ(v.size(), 4u);
  EXPECT_EQ(v[0], 1);
  EXPECT_EQ(v[1], 2);
  EXPECT_EQ(v[2], 99);
  EXPECT_EQ(v[3], 99);

  cpp26::inplace_vector<int, 5> v1{1, 2, 3};
  v1.resize(3);
  EXPECT_EQ(v1.size(), 3u);
  EXPECT_EQ(v1[0], 1);
  EXPECT_EQ(v1[1], 2);
  EXPECT_EQ(v1[2], 3);

  v1.resize(3, 99);
  EXPECT_EQ(v1.size(), 3u);
  EXPECT_EQ(v1[0], 1);
  EXPECT_EQ(v1[1], 2);
  EXPECT_EQ(v1[2], 3);

  v1.resize(0);
  EXPECT_EQ(v1.size(), 0u);
  EXPECT_TRUE(v1.empty());

  v1.resize(0, 42);
  EXPECT_EQ(v1.size(), 0u);
  EXPECT_TRUE(v1.empty());

  v1.resize(2, 77);
  EXPECT_EQ(v1.size(), 2u);
  EXPECT_EQ(v1[0], 77);
  EXPECT_EQ(v1[1], 77);
}

// Test assign operations (iterators, count, initializer_list) - excluding ranges.
TEST(InplaceVectorTest, AssignOperations) {
  cpp26::inplace_vector<int, 5> v1{1, 2, 3};
  std::vector<int> source{4, 5};
  v1.assign(source.begin(), source.end());
  EXPECT_EQ(v1.size(), 2u);
  EXPECT_EQ(v1[0], 4);
  EXPECT_EQ(v1[1], 5);

  cpp26::inplace_vector<int, 5> v2{1, 2, 3};
  v2.assign(2, 99);
  EXPECT_EQ(v2.size(), 2u);
  EXPECT_EQ(v2[0], 99);
  EXPECT_EQ(v2[1], 99);

  cpp26::inplace_vector<int, 5> v3{1, 2, 3};
  v3.assign({7, 8, 9});
  EXPECT_EQ(v3.size(), 3u);
  EXPECT_EQ(v3[0], 7);
  EXPECT_EQ(v3[1], 8);
  EXPECT_EQ(v3[2], 9);

  cpp26::inplace_vector<int, 5> v4{1, 2, 3};
  v4.assign(0, 42);
  EXPECT_EQ(v4.size(), 0u);
  EXPECT_TRUE(v4.empty());

  cpp26::inplace_vector<int, 5> v5{1, 2, 3};
  v5.assign({});
  EXPECT_EQ(v5.size(), 0u);
  EXPECT_TRUE(v5.empty());

  std::vector<int> empty_source;
  cpp26::inplace_vector<int, 5> v6{1, 2, 3};
  v6.assign(empty_source.begin(), empty_source.end());
  EXPECT_EQ(v6.size(), 0u);
  EXPECT_TRUE(v6.empty());
}

// Test all insert operation variants.
TEST(InplaceVectorTest, InsertOperations) {
  cpp26::inplace_vector<int, 6> v1{1, 4};
  auto it = v1.insert(v1.begin() + 1, 2);
  EXPECT_EQ(v1.size(), 3u);
  EXPECT_EQ(v1[0], 1);
  EXPECT_EQ(v1[1], 2);
  EXPECT_EQ(v1[2], 4);
  EXPECT_EQ(it, v1.begin() + 1);

  cpp26::inplace_vector<std::unique_ptr<int>, 3> v2;
  v2.emplace_back(std::make_unique<int>(1));
  v2.emplace_back(std::make_unique<int>(3));
  auto ptr = std::make_unique<int>(2);
  auto it2 = v2.insert(v2.begin() + 1, std::move(ptr));
  EXPECT_EQ(v2.size(), 3u);
  EXPECT_EQ(*v2[0], 1);
  EXPECT_EQ(*v2[1], 2);
  EXPECT_EQ(*v2[2], 3);
  EXPECT_EQ(it2, v2.begin() + 1);

  cpp26::inplace_vector<int, 6> v3{1, 4};
  auto it3 = v3.insert(v3.begin() + 1, 2, 99);
  EXPECT_EQ(v3.size(), 4u);
  EXPECT_EQ(v3[0], 1);
  EXPECT_EQ(v3[1], 99);
  EXPECT_EQ(v3[2], 99);
  EXPECT_EQ(v3[3], 4);
  EXPECT_EQ(it3, v3.begin() + 1);

  cpp26::inplace_vector<int, 6> v4{1, 4};
  std::vector<int> source{2, 3};
  auto it4 = v4.insert(v4.begin() + 1, source.begin(), source.end());
  EXPECT_EQ(v4.size(), 4u);
  EXPECT_EQ(v4[0], 1);
  EXPECT_EQ(v4[1], 2);
  EXPECT_EQ(v4[2], 3);
  EXPECT_EQ(v4[3], 4);
  EXPECT_EQ(it4, v4.begin() + 1);

  cpp26::inplace_vector<int, 6> v5{1, 4};
  auto it5 = v5.insert(v5.begin() + 1, {2, 3});
  EXPECT_EQ(v5.size(), 4u);
  EXPECT_EQ(v5[0], 1);
  EXPECT_EQ(v5[1], 2);
  EXPECT_EQ(v5[2], 3);
  EXPECT_EQ(v5[3], 4);
  EXPECT_EQ(it5, v5.begin() + 1);

  cpp26::inplace_vector<S, 3> v6;
  v6.emplace_back(1);
  v6.emplace_back(3);
  auto it6 = v6.emplace(v6.begin() + 1, 2);
  EXPECT_EQ(v6.size(), 3u);
  EXPECT_EQ(v6[0].x, 1);
  EXPECT_EQ(v6[1].x, 2);
  EXPECT_EQ(v6[2].x, 3);
  EXPECT_EQ(it6, v6.begin() + 1);
}

// Test erase operations (single element and range) including edge cases.
TEST(InplaceVectorTest, EraseOperations) {
  cpp26::inplace_vector<int, 5> v1{1, 2, 3, 4};
  auto it = v1.erase(v1.begin() + 1);
  EXPECT_EQ(v1.size(), 3u);
  EXPECT_EQ(v1[0], 1);
  EXPECT_EQ(v1[1], 3);
  EXPECT_EQ(v1[2], 4);
  EXPECT_EQ(it, v1.begin() + 1);

  cpp26::inplace_vector<int, 5> v2{1, 2, 3, 4, 5};
  auto it2 = v2.erase(v2.begin() + 1, v2.begin() + 3);
  EXPECT_EQ(v2.size(), 3u);
  EXPECT_EQ(v2[0], 1);
  EXPECT_EQ(v2[1], 4);
  EXPECT_EQ(v2[2], 5);
  EXPECT_EQ(it2, v2.begin() + 1);

  cpp26::inplace_vector<int, 3> v3;
  auto it3 = v3.erase(v3.begin());
  EXPECT_EQ(it3, v3.end());
  auto it4 = v3.erase(v3.begin(), v3.end());
  EXPECT_EQ(it4, v3.end());

  cpp26::inplace_vector<int, 5> v4{1, 2, 3, 4, 5};
  auto it5 = v4.erase(v4.begin() + 2, v4.begin() + 2);
  EXPECT_EQ(v4.size(), 5u);
  EXPECT_EQ(it5, v4.end());

  auto it6 = v4.erase(v4.begin(), v4.end());
  EXPECT_TRUE(v4.empty());
  EXPECT_EQ(it6, v4.end());

  cpp26::inplace_vector<int, 5> v5{1, 2, 3, 4, 5};
  auto it7 = v5.erase(v5.end());
  EXPECT_EQ(it7, v5.end());
  EXPECT_EQ(v5.size(), 5u);

  v5.assign({1, 2, 3});
  auto it8 = v5.erase(v5.end(), v5.end());
  EXPECT_EQ(v5.size(), 3u);
  EXPECT_EQ(it8, v5.end());
}

// Test swap operations (member function and global function) with different sizes.
TEST(InplaceVectorTest, SwapOperations) {
  cpp26::inplace_vector<int, 3> v1{1, 2};
  cpp26::inplace_vector<int, 3> v2{3, 4, 5};

  v1.swap(v2);

  EXPECT_EQ(v1.size(), 3u);
  EXPECT_EQ(v1[0], 3);
  EXPECT_EQ(v1[1], 4);
  EXPECT_EQ(v1[2], 5);

  EXPECT_EQ(v2.size(), 2u);
  EXPECT_EQ(v2[0], 1);
  EXPECT_EQ(v2[1], 2);

  cpp26::inplace_vector<int, 3> v3{1, 2};
  cpp26::inplace_vector<int, 3> v4{3, 4, 5};

  using std::swap;
  swap(v3, v4);

  EXPECT_EQ(v3.size(), 3u);
  EXPECT_EQ(v3[0], 3);
  EXPECT_EQ(v3[1], 4);
  EXPECT_EQ(v3[2], 5);

  EXPECT_EQ(v4.size(), 2u);
  EXPECT_EQ(v4[0], 1);
  EXPECT_EQ(v4[1], 2);

  cpp26::inplace_vector<int, 5> v5{1, 2, 3, 4};
  cpp26::inplace_vector<int, 5> v6{5};

  v5.swap(v6);

  EXPECT_EQ(v5.size(), 1u);
  EXPECT_EQ(v5[0], 5);

  EXPECT_EQ(v6.size(), 4u);
  EXPECT_EQ(v6[0], 1);
  EXPECT_EQ(v6[1], 2);
  EXPECT_EQ(v6[2], 3);
  EXPECT_EQ(v6[3], 4);

  cpp26::inplace_vector<int, 0> v7;
  cpp26::inplace_vector<int, 0> v8;

  v7.swap(v8);
  EXPECT_EQ(v7.size(), 0u);
  EXPECT_EQ(v8.size(), 0u);

  using std::swap;
  swap(v7, v8);
  EXPECT_EQ(v7.size(), 0u);
  EXPECT_EQ(v8.size(), 0u);
}

// Test comparison operators (==, !=, <, <=, >, >=, <=>).
TEST(InplaceVectorTest, ComparisonOperators) {
  cpp26::inplace_vector<int, 3> v1{1, 2, 3};
  cpp26::inplace_vector<int, 3> v2{1, 2, 3};
  cpp26::inplace_vector<int, 3> v3{1, 2, 4};
  cpp26::inplace_vector<int, 3> v4{1, 2};

  EXPECT_TRUE(v1 == v2);
  EXPECT_FALSE(v1 != v2);
  EXPECT_FALSE(v1 == v3);
  EXPECT_TRUE(v1 != v3);
  EXPECT_FALSE(v1 == v4);

  cpp26::inplace_vector<int, 5> v5{1, 2};
  cpp26::inplace_vector<int, 5> v6{1, 2, 3};

  EXPECT_FALSE(v5 == v6);
  EXPECT_TRUE(v5 != v6);

#if __cpp_lib_three_way_comparison >= 201907L
  EXPECT_TRUE((v1 <=> v2) == std::strong_ordering::equal);
  EXPECT_TRUE((v1 <=> v3) == std::strong_ordering::less);
  EXPECT_TRUE((v3 <=> v1) == std::strong_ordering::greater);
  EXPECT_TRUE((v5 <=> v6) == std::strong_ordering::less);
#else
  EXPECT_TRUE(v1 < v3);
  EXPECT_FALSE(v3 < v1);
  EXPECT_TRUE(v1 <= v2);
  EXPECT_TRUE(v1 <= v3);
  EXPECT_FALSE(v3 <= v1);
  EXPECT_TRUE(v3 > v1);
  EXPECT_FALSE(v1 > v3);
  EXPECT_TRUE(v2 >= v1);
  EXPECT_TRUE(v3 >= v1);
  EXPECT_FALSE(v1 >= v3);
  EXPECT_TRUE(v5 < v6);
  EXPECT_FALSE(v5 > v6);
  EXPECT_TRUE(v5 <= v6);
  EXPECT_FALSE(v5 >= v6);
#endif
}

// Test global erase and erase_if algorithms.
TEST(InplaceVectorTest, GlobalEraseOperations) {
  cpp26::inplace_vector<int, 5> v1{1, 2, 2, 3, 2};
  auto erase_count = stdcompat::erase(v1, 2);
  EXPECT_EQ(erase_count, 3u);
  EXPECT_EQ(v1.size(), 2u);
  EXPECT_EQ(v1[0], 1);
  EXPECT_EQ(v1[1], 3);

  cpp26::inplace_vector<int, 5> v2{1, 2, 3, 4, 5};
  auto erase_if_count = stdcompat::erase_if(v2, [](int x) { return x % 2 == 0; });
  EXPECT_EQ(erase_if_count, 2u);
  EXPECT_EQ(v2.size(), 3u);
  EXPECT_EQ(v2[0], 1);
  EXPECT_EQ(v2[1], 3);
  EXPECT_EQ(v2[2], 5);

  cpp26::inplace_vector<int, 5> v3{1, 3, 5};
  auto no_erase_count = stdcompat::erase(v3, 2);
  EXPECT_EQ(no_erase_count, 0u);
  EXPECT_EQ(v3.size(), 3u);

  auto no_erase_if_count = stdcompat::erase_if(v3, [](int x) { return x > 10; });
  EXPECT_EQ(no_erase_if_count, 0u);
  EXPECT_EQ(v3.size(), 3u);
}

// Test container management: reserve, capacity, max_size, and self-operation safety.
TEST(InplaceVectorTest, ContainerManagement) {
  cpp26::inplace_vector<int, 5> v{1, 2, 3};

  v.reserve(5);
  EXPECT_EQ(v.size(), 3u);

  v.shrink_to_fit();
  EXPECT_EQ(v.size(), 3u);

  EXPECT_DEATH(v.reserve(10), "");

  EXPECT_EQ(v.max_size(), 5u);
  EXPECT_EQ(v.capacity(), 5u);

  cpp26::inplace_vector<int, 3> v2{1, 2, 3};
  auto* ptr = &v2;

  v2 = *ptr;
  EXPECT_EQ(v2.size(), 3u);
  EXPECT_EQ(v2[0], 1);
  EXPECT_EQ(v2[1], 2);
  EXPECT_EQ(v2[2], 3);

  v2 = std::move(*ptr);
  EXPECT_EQ(v2.size(), 3u);
  EXPECT_EQ(v2[0], 1);
  EXPECT_EQ(v2[1], 2);
  EXPECT_EQ(v2[2], 3);

  v2.swap(v2);
  EXPECT_EQ(v2.size(), 3u);
  EXPECT_EQ(v2[0], 1);
  EXPECT_EQ(v2[1], 2);
  EXPECT_EQ(v2[2], 3);
}

// Test type aliases functionality, constexpr operations, and move semantics through runtime
// behavior.
TEST(InplaceVectorTest, TypeAliasesAndRuntimeBehavior) {
  constexpr auto capacity_5 = cpp26::inplace_vector<int, 5>::capacity();
  EXPECT_EQ(capacity_5, 5u);
  constexpr auto max_size_10 = cpp26::inplace_vector<int, 10>::max_size();
  EXPECT_EQ(max_size_10, 10u);

  cpp26::inplace_vector<int, 3> v;
  EXPECT_TRUE(v.empty());
  EXPECT_EQ(v.size(), 0u);
  EXPECT_EQ(v.capacity(), 3u);
  EXPECT_EQ(v.max_size(), 3u);

  v.push_back(1);
  v.push_back(2);
  EXPECT_EQ(v.size(), 2u);
  EXPECT_EQ(v[0], 1);
  EXPECT_EQ(v[1], 2);

  using Vec = cpp26::inplace_vector<int, 5>;
  Vec v3{1, 2, 3};

  Vec::value_type val = 42;
  v3.push_back(val);
  EXPECT_EQ(v3.back(), 42);

  Vec::size_type expected_size = 4;
  EXPECT_EQ(v3.size(), expected_size);
  EXPECT_TRUE(v3.size() < Vec::size_type{10});

  Vec::difference_type distance = v3.end() - v3.begin();
  EXPECT_EQ(distance, 4);
  EXPECT_TRUE(distance > Vec::difference_type{0});

  Vec::reference ref = v3[0];
  EXPECT_EQ(ref, 1);
  ref = 100;
  EXPECT_EQ(v3[0], 100);

  const Vec& cv3 = v3;
  Vec::const_reference const_ref = cv3[0];
  EXPECT_EQ(const_ref, 100);

  Vec::pointer ptr = v3.data();
  EXPECT_EQ(*ptr, 100);
  *ptr = 200;
  EXPECT_EQ(v3[0], 200);

  Vec::const_pointer const_ptr = cv3.data();
  EXPECT_EQ(*const_ptr, 200);

  constexpr auto capacity_7 = cpp26::inplace_vector<int, 7>::capacity();
  EXPECT_EQ(capacity_7, 7u);
  constexpr auto max_size_7 = cpp26::inplace_vector<int, 7>::max_size();
  EXPECT_EQ(max_size_7, 7u);

  EXPECT_EQ(v3.at(0), 200);
  EXPECT_EQ(v3.at(1), 2);
  EXPECT_EQ(v3.at(2), 3);
  EXPECT_EQ(v3.at(3), 42);

  const Vec& cv3_at = v3;
  EXPECT_EQ(cv3_at.at(0), 200);
  EXPECT_EQ(cv3_at.at(1), 2);
  EXPECT_EQ(cv3_at.at(2), 3);
  EXPECT_EQ(cv3_at.at(3), 42);

  auto it = v3.begin();
  EXPECT_EQ(*it, 200);
  *it = 300;
  EXPECT_EQ(v3[0], 300);

  auto cit = cv3_at.cbegin();
  EXPECT_EQ(*cit, 300);

  auto rit = v3.rbegin();
  EXPECT_EQ(*rit, 42);

  auto crit = cv3_at.crbegin();
  EXPECT_EQ(*crit, 42);

  EXPECT_EQ(std::distance(v3.begin(), v3.end()), 4);
  EXPECT_EQ(std::distance(v3.rbegin(), v3.rend()), 4);

  cpp26::inplace_vector<int, 2> safe_test;
  auto* result = safe_test.try_push_back(1);
  EXPECT_NE(result, nullptr);
  EXPECT_EQ(*result, 1);

  auto* emplace_result = safe_test.try_emplace_back(2);
  EXPECT_NE(emplace_result, nullptr);
  EXPECT_EQ(*emplace_result, 2);

  cpp26::inplace_vector<int, 3> move_test1{1, 2, 3};
  cpp26::inplace_vector<int, 3> move_test2{std::move(move_test1)};
  EXPECT_EQ(move_test2.size(), 3u);
  EXPECT_EQ(move_test2[0], 1);
  EXPECT_EQ(move_test2[1], 2);
  EXPECT_EQ(move_test2[2], 3);
  EXPECT_TRUE(move_test1.empty());

  cpp26::inplace_vector<int, 3> move_test3;
  move_test3 = std::move(move_test2);
  EXPECT_EQ(move_test3.size(), 3u);
  EXPECT_EQ(move_test3[0], 1);
  EXPECT_EQ(move_test3[1], 2);
  EXPECT_EQ(move_test3[2], 3);
  EXPECT_TRUE(move_test2.empty());
}

// Test constructor SFINAE, template constraints, and from_range variants.
TEST(InplaceVectorTest, ConstructorSFINAE) {
  cpp26::inplace_vector<int, 5> v1(3, 42);
  EXPECT_EQ(v1.size(), 3u);
  EXPECT_EQ(v1[0], 42);
  EXPECT_EQ(v1[1], 42);
  EXPECT_EQ(v1[2], 42);

  std::vector<int> source{1, 2, 3};
  cpp26::inplace_vector<int, 5> v2(source.begin(), source.end());
  EXPECT_EQ(v2.size(), 3u);
  EXPECT_EQ(v2[0], 1);
  EXPECT_EQ(v2[1], 2);
  EXPECT_EQ(v2[2], 3);

  std::vector<int> source1{1, 2, 3};
  cpp26::inplace_vector<int, 5> v3(cpp23::from_range, source1);
  EXPECT_EQ(v3.size(), 3u);
  EXPECT_EQ(v3[0], 1);
  EXPECT_EQ(v3[1], 2);
  EXPECT_EQ(v3[2], 3);

  int source2[] = {4, 5, 6, 7};
  cpp26::inplace_vector<int, 6> v4(cpp23::from_range, source2);
  EXPECT_EQ(v4.size(), 4u);
  EXPECT_EQ(v4[0], 4);
  EXPECT_EQ(v4[1], 5);
  EXPECT_EQ(v4[2], 6);
  EXPECT_EQ(v4[3], 7);

  std::list<int> source3{8, 9};
  cpp26::inplace_vector<int, 4> v5(cpp23::from_range, source3);
  EXPECT_EQ(v5.size(), 2u);
  EXPECT_EQ(v5[0], 8);
  EXPECT_EQ(v5[1], 9);

  std::vector<int> empty_source;
  cpp26::inplace_vector<int, 3> v6(cpp23::from_range, empty_source);
  EXPECT_EQ(v6.size(), 0u);
  EXPECT_TRUE(v6.empty());
}

// Test batch insert implementation edge cases and iterator optimization.
TEST(InplaceVectorTest, BatchInsertEdgeCases) {
  cpp26::inplace_vector<int, 5> v1{1, 3};
  std::vector<int> empty_range;
  auto it1 = v1.insert(v1.begin() + 1, empty_range.begin(), empty_range.end());
  EXPECT_EQ(v1.size(), 2u);
  EXPECT_EQ(v1[0], 1);
  EXPECT_EQ(v1[1], 3);
  EXPECT_EQ(it1, v1.begin() + 1);

  auto it2 = v1.insert(v1.begin() + 1, 0, 42);
  EXPECT_EQ(v1.size(), 2u);
  EXPECT_EQ(v1[0], 1);
  EXPECT_EQ(v1[1], 3);
  EXPECT_EQ(it2, v1.begin() + 1);

  auto it3 = v1.insert(v1.end(), 2, 99);
  EXPECT_EQ(v1.size(), 4u);
  EXPECT_EQ(v1[0], 1);
  EXPECT_EQ(v1[1], 3);
  EXPECT_EQ(v1[2], 99);
  EXPECT_EQ(v1[3], 99);
  EXPECT_EQ(it3, v1.begin() + 2);

  cpp26::inplace_vector<int, 5> v2{3, 4};
  auto it4 = v2.insert(v2.begin(), 2, 1);
  EXPECT_EQ(v2.size(), 4u);
  EXPECT_EQ(v2[0], 1);
  EXPECT_EQ(v2[1], 1);
  EXPECT_EQ(v2[2], 3);
  EXPECT_EQ(v2[3], 4);
  EXPECT_EQ(it4, v2.begin());

  std::vector<int> source{1, 2, 3};
  cpp26::inplace_vector<int, 6> v3{0, 4};
  auto it5 = v3.insert(v3.begin() + 1, source.begin(), source.end());
  EXPECT_EQ(v3.size(), 5u);
  EXPECT_EQ(v3[0], 0);
  EXPECT_EQ(v3[1], 1);
  EXPECT_EQ(v3[2], 2);
  EXPECT_EQ(v3[3], 3);
  EXPECT_EQ(v3[4], 4);
  EXPECT_EQ(it5, v3.begin() + 1);

  std::list<int> list_source{5, 6, 7};
  cpp26::inplace_vector<int, 6> v4{0, 8};
  auto it6 = v4.insert(v4.begin() + 1, list_source.begin(), list_source.end());
  EXPECT_EQ(v4.size(), 5u);
  EXPECT_EQ(v4[0], 0);
  EXPECT_EQ(v4[1], 5);
  EXPECT_EQ(v4[2], 6);
  EXPECT_EQ(v4[3], 7);
  EXPECT_EQ(v4[4], 8);
  EXPECT_EQ(it6, v4.begin() + 1);
}

// Test zero-capacity edge cases more thoroughly.
TEST(InplaceVectorTest, ZeroCapacityEdgeCases) {
  cpp26::inplace_vector<int, 0> v;

  EXPECT_EQ(v.size(), 0u);
  EXPECT_EQ(v.capacity(), 0u);
  EXPECT_EQ(v.max_size(), 0u);
  EXPECT_TRUE(v.empty());
  EXPECT_TRUE(v.size() == v.capacity());

  EXPECT_DEATH(v.push_back(1), "");

  EXPECT_EQ(v.begin(), v.end());
  EXPECT_EQ(v.cbegin(), v.cend());
  EXPECT_EQ(v.rbegin(), v.rend());
  EXPECT_EQ(v.crbegin(), v.crend());

  v.resize(0);
  EXPECT_EQ(v.size(), 0u);

  v.clear();
  EXPECT_EQ(v.size(), 0u);

  v.reserve(0);
  EXPECT_EQ(v.capacity(), 0u);

  v.shrink_to_fit();
  EXPECT_EQ(v.capacity(), 0u);

  EXPECT_NE(v.data(), nullptr);

  cpp26::inplace_vector<int, 0> v2(v);
  EXPECT_EQ(v2.size(), 0u);

  cpp26::inplace_vector<int, 0> v3;
  v3 = v;
  EXPECT_EQ(v3.size(), 0u);

  cpp26::inplace_vector<int, 0> v4(std::move(v));
  EXPECT_EQ(v4.size(), 0u);
  EXPECT_EQ(v.size(), 0u);

  cpp26::inplace_vector<int, 0> v5;
  v5 = std::move(v4);
  EXPECT_EQ(v5.size(), 0u);

  v.swap(v2);
  EXPECT_EQ(v.size(), 0u);
  EXPECT_EQ(v2.size(), 0u);

  EXPECT_TRUE(v == v2);
  EXPECT_FALSE(v != v2);

  std::vector<int> empty_source;
  auto it1 = v.insert(v.begin(), empty_source.begin(), empty_source.end());
  EXPECT_EQ(it1, v.begin());
  EXPECT_EQ(v.size(), 0u);

  auto it2 = v.insert(v.begin(), 0, 42);
  EXPECT_EQ(it2, v.begin());
  EXPECT_EQ(v.size(), 0u);

  auto it3 = v.insert(v.begin(), {});
  EXPECT_EQ(it3, v.begin());
  EXPECT_EQ(v.size(), 0u);
}

TEST(InplaceVectorTest, ConstructionIsConstexprForTrivialTypes) {
  constexpr cpp26::inplace_vector<int, 5> kEmptyVector;
  EXPECT_TRUE(kEmptyVector.empty());

  constexpr cpp26::inplace_vector<int, 5> kVectorFromInitializerList{1, 2, 3};
  EXPECT_EQ(kVectorFromInitializerList.size(), 3u);
  EXPECT_THAT(kVectorFromInitializerList, ::testing::ElementsAre(1, 2, 3));

  constexpr cpp26::inplace_vector<int, 5> kCopyConstructedVector(kVectorFromInitializerList);
  EXPECT_EQ(kCopyConstructedVector.size(), 3u);
  EXPECT_THAT(kCopyConstructedVector, ::testing::ElementsAre(1, 2, 3));

  constexpr cpp26::inplace_vector<int, 5> kCopyAssignedVector = kVectorFromInitializerList;
  EXPECT_EQ(kCopyAssignedVector.size(), 3u);
  EXPECT_THAT(kCopyAssignedVector, ::testing::ElementsAre(1, 2, 3));
}

}  // namespace
