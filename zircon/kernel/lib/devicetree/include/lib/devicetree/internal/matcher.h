// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_DEVICETREE_INCLUDE_LIB_DEVICETREE_INTERNAL_MATCHER_H_
#define ZIRCON_KERNEL_LIB_DEVICETREE_INCLUDE_LIB_DEVICETREE_INTERNAL_MATCHER_H_

#include <cstddef>
#include <string_view>
#include <tuple>
#include <type_traits>

#include <fbl/macros.h>

#include "../devicetree.h"

namespace devicetree {

// Forward-declaration.
enum class ScanState : uint8_t;

namespace internal {

// All visitors have the same return type.
template <typename Visitor, typename... Matchers>
using VisitorResultType =
    std::invoke_result_t<Visitor, typename std::tuple_element_t<0, std::tuple<Matchers...>>,
                         size_t>;

// Helper for visiting each matcher with their respective index (matcher0, .... matcherN-1,
// where N == sizeof...(Matchers...)).
template <typename Visitor, size_t... Is, typename... Matchers>
constexpr void ForEachMatcher(Visitor&& visitor, std::index_sequence<Is...> is,
                              Matchers&&... matchers) {
  // Visitor has void return type, wrap on some callable that just returns true and discard
  // the result.
  auto wrapper = [&](auto& matcher, size_t index) -> bool {
    visitor(matcher, index);
    return true;
  };
  [[maybe_unused]] auto res = {wrapper(matchers, Is)...};
}

// Helper for visiting each matcher.
template <typename Visitor, typename... Matchers>
constexpr void ForEachMatcher(Visitor&& visitor, Matchers&&... matchers) {
  // All visitor must be invocable with the visit state and the matchers.
  static_assert((... && std::is_invocable_v<Visitor, Matchers, size_t>),
                "|Visitor| must provide an overload for each provided matcher.");

  static_assert((... && std::is_same_v<VisitorResultType<Visitor, Matchers...>,
                                       std::invoke_result_t<Visitor, Matchers, size_t>>),
                "|Visitor| must have the same return type for all matcher types.");
  ForEachMatcher(std::forward<Visitor>(visitor), std::make_index_sequence<sizeof...(Matchers)>{},
                 std::forward<Matchers>(matchers)...);
}

// Per matcher state kept by the |Match| infrastructre. This state determines whether a matcher
// is active(visiting all nodes), suspended(not visiting current subtree) or done(not visiting
// anything anymore).
template <typename ScanState>
class MatcherVisitImpl {
 public:
  constexpr MatcherVisitImpl() = default;
  constexpr MatcherVisitImpl(const MatcherVisitImpl&) = default;
  constexpr MatcherVisitImpl& operator=(const MatcherVisitImpl&) = default;

  // Initialize and assign from
  explicit constexpr MatcherVisitImpl(ScanState state) : state_(state) {}

  constexpr ScanState state() const { return state_; }
  constexpr void set_state(ScanState state) {
    state_ = state;
    if (state_ == ScanState::kNeedsPathResolution) {
      extra_alias_scan_ = 1;
    }
  }

  // Prevents the associated matcher's callback from being called in any nodes
  // that are part of the subtree of the current node.
  void Prune(const NodePath& path) { mark_ = &path.back(); }

  // Counterpart to |Prune|, which marks the end of a subtree, and allows for
  // matcher's callbacks to be called again.
  void Unprune(const NodePath& path) {
    if (mark_ == &path.back()) {
      *this = MatcherVisitImpl();
    }
  }

  // Whether an extra scan is required due to this matcher attempting to resolve aliases.
  constexpr size_t extra_alias_scan() const { return extra_alias_scan_; }

 private:
  ScanState state_ = ScanState::kActive;
  const Node* mark_ = nullptr;
  size_t extra_alias_scan_ = 0;
};

using MatcherVisit = MatcherVisitImpl<ScanState>;

template <typename Matcher>
constexpr size_t GetMaxScans() {
  using MatcherType = std::decay_t<Matcher>;
  return MatcherType::kMaxScans;
}

// A Matcher whose sole pupose is to guarantee that aliases will be resolved if present. This allows
// indicating that alias still need to be resolved, if no user provided matcher can make progress
// due to path resolution.
class AliasMatcher {
 public:
  // Aliases must be resolved within one walk.
  static constexpr size_t kMaxScans = 1;

  ScanState OnNode(const NodePath& path, const PropertyDecoder& decoder);

  ScanState OnScan();

  ScanState OnSubtree(const NodePath& path);

  void OnDone() {}

  void OnError(std::string_view error);
};

}  // namespace internal

}  // namespace devicetree

#endif  // ZIRCON_KERNEL_LIB_DEVICETREE_INCLUDE_LIB_DEVICETREE_INTERNAL_MATCHER_H_
