// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_DEVICETREE_INCLUDE_LIB_DEVICETREE_MATCHER_H_
#define ZIRCON_KERNEL_LIB_DEVICETREE_INCLUDE_LIB_DEVICETREE_MATCHER_H_

#include <stddef.h>

#include <concepts>

#include "devicetree.h"
#include "internal/matcher.h"

namespace devicetree {

// The Match API uses the following terms:
//
//  * `walk` : Refers to |Devicetree::Walk| operation, which consists on visiting all nodes on the
//  tree.
//
//  * `scan` : Refers to inspecting nodes in the devicetree during a |walk|, to retrieve interesting
//  information for all registered matchers.
//
//  * `matcher` : Entity collecting information through 1 or more |scans|. Each |scan| may collect a
//  piece of the information. See `Matcher` concept in "matcher-type.h" for contract details.
//
//  * `registered matcher` : Matcher who has not yet reached completion, and will continue to be
//  part of the scan process.

// Return value of `Matcher` methods.
enum class ScanState : uint8_t {
  // Matcher has finished collecting information, no more scans are needed.
  kDone,

  // Matcher cannot do further progress in the current path.
  kDoneWithSubtree,

  // Matcher needs nodes in the current path, so it wishes to visit offspring.
  kActive,

  // Matcher cannot make further progress until the aliases node is resolved.
  kNeedsPathResolution,
};

// Helper concept.
template <typename T>
concept UnsignedIntegralConstant = std::unsigned_integral<std::remove_reference_t<T>> &&
                                   std::is_const_v<std::remove_reference_t<T>>;

// A `Matcher` type object must follow the following compile time contract and is meant
// to be used in conjunction with `Devicetree::Match`.
template <typename MatcherType>
concept MatcherImpl = requires(MatcherType matcher, const NodePath& path,
                               const PropertyDecoder& decoder, std::string_view error_message) {
  // Defines the number of scans required to reach completion.
  // Alias resolution are not taken into account.
  { MatcherType::kMaxScans } -> UnsignedIntegralConstant;
  { std::integral_constant<size_t, MatcherType::kMaxScans>{} };

  // During a tree scan, `Matcher::OnNode` is called for each node in the tree,
  // as long as the matcher has not reached `ScanState::kDone` state or
  // `ScanState::kIgnoreSubtree` on parent node or `ScanState::kNeedsAliases`.
  { matcher.OnNode(path, decoder) } -> std::same_as<ScanState>;

  // Called after every node in the subtree rooted at `path` has been visited, if the
  // matcher's state is `ScanState::kActive`.
  //
  // * Returning `ScanState::kNeedsPathResolution` is considered an error. }
  //
  // * Returning `ScanState::kDoneWithSubtree` is equivalent to `ScanState::kDone`,
  //   users should prefer the latter.
  { matcher.OnSubtree(path) } -> std::convertible_to<ScanState>;

  // Called after the tree has been fully visited each time. Equivalent
  // to `OnSubtree` where `path` is the root.
  { matcher.OnScan() } -> std::convertible_to<ScanState>;

  // Called if the matcher returns 'ScanState::kDone`, at the end of the matching process. May
  // be used to finalize decoded state.
  { matcher.OnDone() };

  // Called when an error is encountered during matching operation. The error may come from
  // the matching infrastructure or the matcher implementation.
  { matcher.OnError(error_message) } -> std::same_as<void>;
} && MatcherType::kMaxScans > 0;

template <typename T>
concept Matcher = MatcherImpl<std::decay_t<T>>;

// `OkNodeMatcher` fulfills the `Matcher` concept, but declares that is only interested
// in visiting nodes whose status is `okay` or equivalent.
//
// To achieve this a `OkNodeMatcher` implementation must define a constexpr bool member
// `kMatchOkNodesOnly` as true.
template <typename T>
concept OkNodeMatcher = Matcher<T> && requires {
  { std::bool_constant<std::decay_t<T>::kMatchOkNodesOnly>{} };
} && T::kMatchOkNodesOnly;

// Returns true if all `matchers` completed successfully.
//
// Usage example:
// ```
// // Assume |dt| is a |devicetree::Devicetree| object.
//
// struct FooMatcher {
//   constexpr size_t kMaxScans = 1;
//   ScanState OnNode(const NodePath& path, const PropertyDecoder& decoder) {
//     if (path.back() == "foo") {
//        foo_count++;
//        subtree_start_ = &path.back();
//        return ScanState::kActive;
//     }
//   }
//
//   ScanState OnSubtree(const NodePath& path) {
//      if (&path.back() ==  subtree_start_) {
//          // All childs of |subtree_start_| have been visited.
//      }
//   }
//
//   ScanState OnScan() {
//     return ScanState::kDone;
//   }
//
//   void OnDone() {}
//
//   void OnError(std::string_view err) {
//     std::cout << " Foo Matcher had an error: " << err << std::endl;
//   }
//
//   int foo_count = 0;
//  }
//
//  ...
//  FooMatcher foo_matcher;
//  if (!devicetree::Match(dt, foo_matcher)) {
//    return;
//  }
//  std::cout << " Nodes names foo: " << foo_matcher.foo_count << std::endl;
// ```
//
template <Matcher... Matchers>
constexpr bool Match(const devicetree::Devicetree& devicetree, Matchers&&... matchers) {
  using internal::AliasMatcher;
  using internal::ForEachMatcher;
  using internal::MatcherVisit;
  static_assert(Matcher<AliasMatcher>);

  // Matcher that prevents short circuiting the alias node, when other matchers cant make forward
  // progress.
  AliasMatcher alias_matcher;

  // Add an extra walk, for possible alias resolution step.
  constexpr size_t kMaxScanForMatchers = std::max({internal::GetMaxScans<Matchers>()...}) + 1;

  // Extra state for the alias matcher.
  std::array<MatcherVisit, sizeof...(Matchers) + 1> visit_state;

  // Call |OnNode| on all matchers that are not done or avoiding the subtree.
  auto visit_and_prune = [&visit_state, &alias_matcher, &matchers...](
                             const NodePath& path, const PropertyDecoder& decoder) {
    auto on_each_matcher = [&visit_state, &path, &decoder]<typename MatcherT>(MatcherT& matcher,
                                                                              size_t index) {
      auto& matcher_state = visit_state[index];
      if (matcher_state.state() == ScanState::kActive) {
        // If the matcher only wants to visit `okay` status node.
        if constexpr (OkNodeMatcher<MatcherT>) {
          if (!decoder.is_status_ok()) {
            matcher_state.set_state(ScanState::kDoneWithSubtree);
            matcher_state.Prune(path);
            return;
          }
        }
        matcher_state.set_state(matcher.OnNode(path, decoder));
        if (matcher_state.state() == ScanState::kDoneWithSubtree) {
          matcher_state.Prune(path);
        }
      }
    };
    ForEachMatcher(on_each_matcher, matchers..., alias_matcher);
    // Return whether we still need to visit any node in the underlying subtree.
    return std::any_of(visit_state.begin(), visit_state.end(), [](auto& visit_state) {
      return visit_state.state() == ScanState::kActive ||
             visit_state.state() == ScanState::kNeedsPathResolution;
    });
  };

  // Unprune any pruned Node, as a post order visitor.
  auto unprune = [&visit_state, &matchers..., &alias_matcher](const NodePath& path,
                                                              const PropertyDecoder& decoder) {
    ForEachMatcher(
        [&visit_state, &path](auto& matcher, size_t index) {
          auto state = visit_state[index].state();
          if (state == ScanState::kActive) {
            ScanState subtree_state = matcher.OnSubtree(path);
            visit_state[index].set_state(
                subtree_state == ScanState::kDoneWithSubtree ? ScanState::kActive : subtree_state);
            ZX_ASSERT(visit_state[index].state() != ScanState::kNeedsPathResolution);
          }
          visit_state[index].Unprune(path);
        },
        matchers..., alias_matcher);
  };

  // Call OnScan on ever matcher
  auto on_scan = [](auto& visit_state, auto&... matchers) {
    ForEachMatcher(
        [&visit_state](auto& matcher, size_t index) {
          if (visit_state[index].state() != ScanState::kDone &&
              visit_state[index].state() != ScanState::kNeedsPathResolution) {
            visit_state[index].set_state(matcher.OnScan());
          }
        },
        matchers...);
  };

  // Call OnDone on ever matcher, when appropriate.
  auto on_done = [](auto& visit_state, auto&... matchers) {
    ForEachMatcher(
        [&visit_state](auto& matcher, size_t index) {
          if (visit_state[index].state() == ScanState::kDone) {
            matcher.OnDone();
          }
        },
        matchers...);
  };

  // Verify that matchers fulfill their scan contract, that is every matcher visit state must be
  // |kDone| after finishing the current devicetree scan. Return value:
  enum class ScanResult {
    kMatchersDone,
    kMatchersPending,
    kMatchersWithError,
  };

  auto all_matchers_done = [&visit_state](size_t current_scan, auto&... matchers) {
    int error_count = 0;
    int finished_count = 0;
    ForEachMatcher(
        [&error_count, &finished_count, &visit_state, current_scan](auto& matcher, size_t index) {
          using MatcherType = std::decay_t<decltype(matcher)>;
          if (visit_state[index].state() != ScanState::kDone) {
            if (current_scan > 0 && visit_state[index].state() == ScanState::kNeedsPathResolution) {
              matcher.OnError("Matcher failed to resolved path after first scan.");
              error_count++;
            } else if (current_scan >=
                       (MatcherType::kMaxScans + visit_state[index].extra_alias_scan() - 1)) {
              matcher.OnError("Matcher failed to reach completion the requested scan number.");
              error_count++;
            }
            return;
          }
          finished_count++;
        },
        matchers...);

    if (finished_count == sizeof...(matchers)) {
      return ScanResult::kMatchersDone;
    }

    if (error_count == 0) {
      return ScanResult::kMatchersPending;
    }
    return ScanResult::kMatchersWithError;
  };

  for (size_t i = 0; i < kMaxScanForMatchers; ++i) {
    devicetree.Walk(visit_and_prune, unprune);
    on_scan(visit_state, matchers..., alias_matcher);

    // If result == 1 then no errors found, but not all matchers are done.
    if (auto res = all_matchers_done(i, matchers...); res != ScanResult::kMatchersPending) {
      on_done(visit_state, matchers...);
      return res == ScanResult::kMatchersDone;
    }

    // Clear path resolution if its first iteration
    if (i == 0) {
      for (auto& state : visit_state) {
        if (state.state() == ScanState::kNeedsPathResolution) {
          state.set_state(ScanState::kActive);
        }
      }
    }
  }

  return true;
}

}  // namespace devicetree

#endif  // ZIRCON_KERNEL_LIB_DEVICETREE_INCLUDE_LIB_DEVICETREE_MATCHER_H_
