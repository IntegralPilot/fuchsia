// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_STDCOMPAT_ALGORITHM_H_
#define LIB_STDCOMPAT_ALGORITHM_H_

#include <algorithm>
#include <iterator>

#include "internal/algorithm.h"

namespace cpp20 {

#if defined(__cpp_lib_constexpr_algorithms) && __cpp_lib_constexpr_algorithms >= 201806L && \
    !defined(LIB_STDCOMPAT_USE_POLYFILLS)

using std::is_sorted;
using std::sort;

using std::remove;
using std::remove_if;

using std::lower_bound;

using std::find;
using std::find_if;
using std::find_if_not;

#else

template <typename RandomIterator, typename Comparator = std::less<>>
constexpr void sort(RandomIterator first, RandomIterator end, Comparator comp = Comparator{}) {
#if LIB_STDCOMPAT_CONSTEVAL_SUPPORT

  if (!cpp20::is_constant_evaluated()) {
    return std::sort(first, end, comp);
  }

#endif  // LIB_STDCOMPAT_CONSTEVAL_SUPPORT
  return cpp20::internal::sort(first, end, comp);
}

template <typename ForwardIt, typename Comparator = std::less<>>
constexpr bool is_sorted(ForwardIt first, ForwardIt end, Comparator comp = Comparator{}) {
#if LIB_STDCOMPAT_CONSTEVAL_SUPPORT

  if (!cpp20::is_constant_evaluated()) {
    return std::is_sorted(first, end, comp);
  }

#endif  // LIB_STDCOMPAT_CONSTEVAL_SUPPORT
  return cpp20::internal::is_sorted(first, end, comp);
}

using internal::remove;
using internal::remove_if;

template <typename ForwardIt, typename T, typename Comparator>
constexpr ForwardIt lower_bound(ForwardIt first, ForwardIt last, const T& value, Comparator comp) {
  using difference_t = typename std::iterator_traits<ForwardIt>::difference_type;
  difference_t length = std::distance(first, last);

  while (length > 0) {
    difference_t step = length / 2;
    auto it = std::next(first, step);

    if (comp(*it, value)) {
      first = std::next(it);
      length -= step + 1;
    } else {
      length = step;
    }
  }

  return first;
}

template <typename ForwardIt, typename T>
constexpr ForwardIt lower_bound(ForwardIt first, ForwardIt last, const T& value) {
  return ::cpp20::lower_bound(first, last, value, std::less<>{});
}

template <class InputIt, class T>
constexpr InputIt find(InputIt first, InputIt last, const T& value) {
  for (; first != last; ++first) {
    if (*first == value) {
      return first;
    }
  }
  return last;
}

template <class InputIt, class UnaryPredicate>
constexpr InputIt find_if(InputIt first, InputIt last, UnaryPredicate p) {
  for (; first != last; ++first) {
    if (p(*first)) {
      return first;
    }
  }
  return last;
}

template <class InputIt, class UnaryPredicate>
constexpr InputIt find_if_not(InputIt first, InputIt last, UnaryPredicate q) {
  for (; first != last; ++first) {
    if (!q(*first)) {
      return first;
    }
  }
  return last;
}

#endif

}  // namespace cpp20

#endif  // LIB_STDCOMPAT_ALGORITHM_H_
