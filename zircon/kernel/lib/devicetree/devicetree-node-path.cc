// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <zircon/assert.h>

#include <string_view>

#include "lib/devicetree/devicetree.h"

namespace devicetree {
namespace {

// Gives an incrementing, pairwise comparison of two ranges of string_view
// iterators, returning the first pair of iterators that point to differing
// values.
template <typename IterA, typename IterB>
std::pair<IterA, IterB> CompareStringRanges(IterA start_a, IterA end_a, IterB start_b,
                                            IterB end_b) {
  while (start_a != end_a && start_b != end_b) {
    if (*start_a != *start_b) {
      break;
    }
    ++start_a;
    ++start_b;
  }
  return {start_a, start_b};
}

}  // namespace

NodePath::Comparison NodePath::CompareWith(const ResolvedPath& path) const {
  // The use of `std::next()` below would require a bunch of obscure member type
  // definitions in `StringList<'/'>::iterator`.
  auto next = [](auto it) { return ++it; };

  StringList<'/'> path_prefix = path.Prefix();
  auto path_prefix_begin = path_prefix.begin();
  auto path_prefix_end = path_prefix.end();

  auto [this_prefix_it, path_prefix_it] =
      CompareStringRanges(begin(), end(), path_prefix_begin, path_prefix_end);

  // We found a mismatch in the prefix.
  if (this_prefix_it != end() && path_prefix_it != path_prefix_end) {
    return Comparison::kMismatch;
  }

  // If we iterated through each node in this path and there are still tokens
  // left in the other path, then this is an ancestor of the other
  if (this_prefix_it == end() && path_prefix_it != path_prefix_end) {
    return next(path_prefix_it) == path_prefix_end && path.suffix.empty()
               ? Comparison::kParent
               : Comparison::kIndirectAncestor;
  }

  StringList<'/'> path_suffix = path.Suffix();
  auto path_suffix_begin = path_suffix.begin();
  auto path_suffix_end = path_suffix.end();
  auto [this_suffix_it, path_suffix_it] =
      CompareStringRanges(this_prefix_it, end(), path_suffix_begin, path_suffix_end);

  // We found a mismatch in the suffix.
  if (this_suffix_it != end() && path_suffix_it != path_suffix_end) {
    return Comparison::kMismatch;
  }

  // As above, if we iterated through each node in this path and there are still
  // tokens left in the other, then this is an ancestor.
  if (this_suffix_it == end() && path_suffix_it != path_suffix_end) {
    return next(path_prefix_it) == path_prefix_end ? Comparison::kParent
                                                   : Comparison::kIndirectAncestor;
  }

  // Similarly, if we iterated through each token in the other path and there
  // are still nodes left to compare in this one, then this is a descendent.
  if (this_suffix_it != end() && path_suffix_it == path_suffix_end) {
    return next(this_suffix_it) == end() ? Comparison::kChild : Comparison::kIndirectDescendent;
  }

  return Comparison::kEqual;
}

NodePath::Comparison NodePath::CompareWith(std::string_view path) const {
  ZX_ASSERT(path.empty() || path[0] == '/');
  return CompareWith(ResolvedPath{.prefix = path});
}

}  // namespace devicetree
