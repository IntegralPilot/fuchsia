// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ZXDUMP_TEST_DATA_HOLDER_H_
#define SRC_LIB_ZXDUMP_TEST_DATA_HOLDER_H_

#include <lib/zxdump/task.h>
#include <zircon/syscalls/object.h>

#include <string>
#include <string_view>
#include <tuple>
#include <type_traits>

#include <gtest/gtest.h>

namespace zxdump::testing {

// TestDataHolder<T...> holds data from get_info and get_property calls.
// Each T is InfoTraits<...> or PropertyTraits<...>.  Get<T>() yields a
// TestDataItem<T>, defined below.  Data can be easily checked against
// another identically-typed data holder, or individual items can be
// checked against other like items.

template <class Traits>
class TestDataItem;

template <class... ItemTraits>
class TestDataHolder {
 public:
  // Get the TestDataItem for the given InfoTraits<...> or PropertyTraits<...>.
  template <class Item>
  decltype(auto) Get() {
    return std::get<TestDataItem<Item>>(items_);
  }

  template <class Item>
  decltype(auto) Get() const {
    return std::get<TestDataItem<Item>>(items_);
  }

  template <class Item>
  void Check(const TestDataItem<Item>& other) const {
    Get<Item>().Check(other);
  }

  // Check each item.
  void Check(const TestDataHolder& other) const { (Check(other.Get<ItemTraits>()), ...); }

  // This fills each item using get_info / get_property.
  void Fill(zxdump::Object& object) { (Get<ItemTraits>().Fill(object), ...); }

 private:
  std::tuple<TestDataItem<ItemTraits>...> items_;
};

// This gets instantiated with the InfoTraits<...> or PropertyTraits<...>
// type so it can be specialized on those.  The partial specializations below
// just instantiate this again with the Traits::type.  This default
// instantiation works for simple types where EXPECT_EQ works.  It also works
// when the constexpr variable kTestDataValueStruct<T> is specialized to
// return a tuple of T::* member pointers.
template <typename T>
struct TestDataValueType {
  using type = T;

  static constexpr bool kIsVector = false;

  static void Check(std::string_view name, const type& old_value, const type& new_value) {
    EXPECT_EQ(old_value, new_value) << name;
  }
};

// When Traits::type is a span, this default specialization just checks for
// matching elements by instantiating again on the element type.
template <typename T>
struct TestDataValueType<std::span<const T>> {
  using type = std::vector<T>;

  static constexpr bool kIsVector = true;

  static void Check(std::string_view name, std::span<const T> old_value,
                    std::span<const T> new_value) {
    ASSERT_EQ(old_value.size(), new_value.size()) << name;
    for (size_t i = 0; i < old_value.size(); ++i) {
      TestDataValueType<T>::Check(std::string(name) + "[" + std::to_string(i) + "]", old_value[i],
                                  new_value[i]);
    }
  }
};

// If not specialized on a specific InfoTraits<...>, instantiate for the data
// type.
template <uint32_t Topic>
struct TestDataValueType<InfoTraits<Topic>>
    : public TestDataValueType<typename InfoTraits<Topic>::type> {};

// If not specialized on a specific PropertyTraits<...>, instantiate for the
// data type.
template <uint32_t Property>
struct TestDataValueType<PropertyTraits<Property>>
    : public TestDataValueType<typename PropertyTraits<Property>::type> {};

// TestDataItem looks a little like a smart pointer to Traits::type.  When
// Traits::type is a span<T>, it acts like a smart pointer to std::vector<T>.
template <class Traits>
class TestDataItem {
 public:
  const auto& operator*() const { return value_; }

  const auto* operator->() const { return &value_; }

  // This fills the item with an explicit value, which might be a span.
  void Fill(const typename Traits::type& value) {
    if constexpr (TestDataValueType<Traits>::kIsVector) {
      value_ = typename TestDataValueType<Traits>::type(value.begin(), value.end());
    } else {
      value_ = value;
    }
  }

  // This fills the item using get_info / get_property.
  void Fill(zxdump::Object& object) {
    auto result = GetData{}(object);
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    Fill(result.value());
  }

  // Check a new value against this value.  For thread and process items this
  // is usually just doing EXPECT_EQ, expecting both samples to be taken
  // while the process is suspended and so nothing changes.  For job and
  // kernel values that keep changing, it checks for the new value being
  // plausible for a sample taken later, e.g. EXPECT_GE for statistics.
  void Check(const TestDataItem& new_item) const { Check(*new_item); }

  // Check a new raw value rather than another identical TestDataItem.
  // This has specializations for all the traits types.
  void Check(const typename Traits::type& new_value) const {
    ItemValueType::Check(Traits::kName, value_, new_value);
  }

 private:
  using ItemValueType = TestDataValueType<Traits>;

  // This is a separate template on Traits so it can be specialized.
  template <class T = Traits>
  struct GetData;

  template <uint32_t Topic>
  struct GetData<InfoTraits<Topic>> {
    auto operator()(zxdump::Object& object) const { return object.get_info<Topic>(); }
  };

  template <uint32_t Property>
  struct GetData<PropertyTraits<Property>> {
    auto operator()(zxdump::Object& object) const { return object.get_property<Property>(); }
  };

  typename TestDataValueType<Traits>::type value_;
};

template <>
struct TestDataValueType<zx_info_cpu_stats_t> {
  using type = zx_info_cpu_stats_t;

  static constexpr bool kIsVector = false;

  static void Check(std::string_view name, const type& old_value, const type& new_value);
};

template <>
struct TestDataValueType<zx_info_kmem_stats_t> {
  using type = zx_info_kmem_stats_t;

  static constexpr bool kIsVector = false;

  static void Check(std::string_view name, const type& old_value, const type& new_value);
};

template <>
struct TestDataValueType<zx_arm64_info_guest_stats_t> {
  using type = zx_arm64_info_guest_stats_t;

  static constexpr bool kIsVector = false;

  static void Check(std::string_view name, const type& old_value, const type& new_value);
};

template <>
struct TestDataValueType<zx_riscv64_info_guest_stats_t> {
  using type = zx_riscv64_info_guest_stats_t;

  static constexpr bool kIsVector = false;

  static void Check(std::string_view name, const type& old_value, const type& new_value);
};

template <>
struct TestDataValueType<zx_x86_64_info_guest_stats_t> {
  using type = zx_x86_64_info_guest_stats_t;

  static constexpr bool kIsVector = false;

  static void Check(std::string_view name, const type& old_value, const type& new_value);
};

}  // namespace zxdump::testing

#endif  // SRC_LIB_ZXDUMP_TEST_DATA_HOLDER_H_
