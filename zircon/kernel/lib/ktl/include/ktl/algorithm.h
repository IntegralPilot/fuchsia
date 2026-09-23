// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_KTL_INCLUDE_KTL_ALGORITHM_H_
#define ZIRCON_KERNEL_LIB_KTL_INCLUDE_KTL_ALGORITHM_H_

#include <algorithm>

namespace ktl {

// "Non-modifying sequence operations"
using std::adjacent_find;
using std::all_of;
using std::any_of;
using std::count;
using std::count_if;
using std::find;
using std::find_end;
using std::find_first_of;
using std::find_if;
using std::find_if_not;
using std::for_each;
using std::for_each_n;
using std::none_of;
using std::search;
using std::search_n;

namespace ranges {
using std::ranges::adjacent_find;
using std::ranges::all_of;
using std::ranges::any_of;
using std::ranges::count;
using std::ranges::count_if;
using std::ranges::find;
using std::ranges::find_end;
using std::ranges::find_first_of;
using std::ranges::find_if;
using std::ranges::find_if_not;
using std::ranges::for_each;
using std::ranges::for_each_n;
using std::ranges::none_of;
using std::ranges::search;
using std::ranges::search_n;
}  // namespace ranges

// "Modifying sequence operations" (subset)
using std::copy;
using std::copy_if;
using std::fill;
using std::fill_n;
using std::reverse;
using std::shuffle;
using std::transform;

namespace ranges {
using std::ranges::copy;
using std::ranges::copy_if;
using std::ranges::fill;
using std::ranges::fill_n;
using std::ranges::reverse;
using std::ranges::shuffle;
using std::ranges::transform;
}  // namespace ranges

// "Sorting operations" (subset)
using std::is_sorted;
using std::is_sorted_until;
using std::sort;
using std::stable_sort;

namespace ranges {
using std::ranges::is_sorted;
using std::ranges::is_sorted_until;
using std::ranges::sort;
using std::ranges::stable_sort;
}  // namespace ranges

// "Binary search operations (on sorted ranges)"
using std::binary_search;
using std::equal_range;
using std::lower_bound;
using std::upper_bound;

namespace ranges {
using std::ranges::binary_search;
using std::ranges::equal_range;
using std::ranges::lower_bound;
using std::ranges::upper_bound;
}  // namespace ranges

// "Minimum/maximum operations"
using std::clamp;
using std::equal;
using std::max;
using std::max_element;
using std::min;
using std::min_element;
using std::minmax;
using std::minmax_element;

namespace ranges {
using std::ranges::clamp;
using std::ranges::max;
using std::ranges::max_element;
using std::ranges::min;
using std::ranges::min_element;
using std::ranges::minmax;
using std::ranges::minmax_element;
}  // namespace ranges

}  // namespace ktl

#endif  // ZIRCON_KERNEL_LIB_KTL_INCLUDE_KTL_ALGORITHM_H_
