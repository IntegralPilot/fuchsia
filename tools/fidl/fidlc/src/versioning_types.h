// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_VERSIONING_TYPES_H_
#define TOOLS_FIDL_FIDLC_SRC_VERSIONING_TYPES_H_

#include <lib/fit/function.h>
#include <zircon/assert.h>

#include <map>
#include <optional>
#include <set>
#include <string>

namespace fidlc {

// This file defines types used for FIDL Versioning. For more detail, read
// https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0083_fidl_versioning#formalism.

// A platform represents a group of FIDL libraries that are versioned together.
// Usually all the library names begin with a common prefix, the platform name.
// Libraries that don't use versioning belong to a platform named "unversioned".
class Platform final {
 public:
  // Returns the "unversioned" platform.
  static Platform Unversioned() { return Platform(kUnversionedName); }
  // Creates a platform. Returns null if `str` is not a valid name.
  static std::optional<Platform> Parse(std::string str);

  // Returns true if this is the unversioned platform.
  bool is_unversioned() const { return name_ == kUnversionedName; }
  const std::string& name() const { return name_; }

  constexpr bool operator==(const Platform& rhs) const { return name_ == rhs.name_; }
  constexpr bool operator!=(const Platform& rhs) const { return name_ != rhs.name_; }

  struct Compare {
    bool operator()(const Platform& lhs, const Platform& rhs) const {
      return lhs.name_ < rhs.name_;
    }
  };

 private:
  static constexpr const char* kUnversionedName = "unversioned";
  explicit Platform(std::string name) : name_(std::move(name)) {}

  std::string name_;
};

// A version represents a particular state of a platform.
//
// The primary use case of FIDL versioning is to version the "fuchsia" platform
// by API level, so versions are designed to accommodate API levels. A version
// is one of the following 32-bit unsigned integers:
//
//     Normal
//         1, ..., 2^31-1
//     Special: see RFC-0246 for the definitions
//         0xFFD00000  NEXT
//         0xFFE00000  HEAD
//         0xFFF00000  PLATFORM / LEGACY
//     Infinite: only used internally to simplify algorithms
//         0x00000000  -inf
//         0xFFFFFFFF  +inf
//
// All others (i.e. 2^31 and up, apart from the listed exceptions) are invalid.
class Version final {
 public:
  // Succeeds if `number` corresponds to a valid finite version.
  static std::optional<Version> From(uint32_t number);
  // Succeeds if `str` is the decimal number or name of a valid finite version.
  static std::optional<Version> Parse(std::string_view str);

  // Returns the version's underlying number.
  uint32_t number() const { return value_; }
  // Returns the version's name. Panics if this is not a special version.
  std::string_view name() const;
  // Returns the RFC-0246 canonical string representation of the version.
  std::string ToString() const;
  // Returns the version that comes before this one. Panics if infinite.
  Version Predecessor() const;
  // Returns the version that comes after this one. Panics if infinite.
  Version Successor() const;

  constexpr bool operator==(const Version& rhs) const { return value_ == rhs.value_; }
  constexpr bool operator!=(const Version& rhs) const { return value_ != rhs.value_; }
  constexpr bool operator<(const Version& rhs) const { return value_ < rhs.value_; }
  constexpr bool operator<=(const Version& rhs) const { return value_ <= rhs.value_; }
  constexpr bool operator>(const Version& rhs) const { return value_ > rhs.value_; }
  constexpr bool operator>=(const Version& rhs) const { return value_ >= rhs.value_; }

  // These are defined below because the Version type is incomplete here.
  static const Version kNegInf;
  static const Version kPosInf;
  static const Version kNext;
  static const Version kHead;
  static const Version kLegacy;
  static const Version kSpecialVersions[];

 private:
  constexpr explicit Version(uint32_t value) : value_(value) {}

  uint32_t value_;
};

constexpr Version Version::kNegInf(0);
constexpr Version Version::kPosInf(UINT32_MAX);
constexpr Version Version::kNext(0xFFD00000);
constexpr Version Version::kHead(0xFFE00000);
constexpr Version Version::kLegacy(0xFFF00000);
constexpr Version Version::kSpecialVersions[] = {kNext, kHead, kLegacy};

// A version range is a nonempty set of versions in some platform, from an
// inclusive lower bound to an exclusive upper bound.
class VersionRange final {
 public:
  constexpr VersionRange(Version lower, Version upper_exclusive) : pair_(lower, upper_exclusive) {
    ZX_ASSERT_MSG(lower < upper_exclusive, "invalid version range");
  }

  // Returns the [lower, upper) version pair.
  std::pair<Version, Version>& pair() { return pair_; }
  const std::pair<Version, Version>& pair() const { return pair_; }

  // Returns true if this range contains `version`.
  bool Contains(Version version) const;

  // Returns the intersection of two (possibly empty) ranges.
  static std::optional<VersionRange> Intersect(const std::optional<VersionRange>& lhs,
                                               const std::optional<VersionRange>& rhs);

  constexpr bool operator==(const VersionRange& rhs) const { return pair_ == rhs.pair_; }
  constexpr bool operator!=(const VersionRange& rhs) const { return pair_ != rhs.pair_; }
  constexpr bool operator<(const VersionRange& rhs) const { return pair_ < rhs.pair_; }
  constexpr bool operator<=(const VersionRange& rhs) const { return pair_ <= rhs.pair_; }
  constexpr bool operator>(const VersionRange& rhs) const { return pair_ > rhs.pair_; }
  constexpr bool operator>=(const VersionRange& rhs) const { return pair_ >= rhs.pair_; }

 private:
  std::pair<Version, Version> pair_;
};

// A version set is a nonempty set of versions in some platform, made of either
// one range or two disjoint ranges.
class VersionSet final {
 public:
  constexpr explicit VersionSet(VersionRange first,
                                std::optional<VersionRange> second = std::nullopt)
      : ranges_(first, second) {
    ZX_ASSERT_MSG(VersionRange::Intersect(first, second) == std::nullopt,
                  "ranges must be disjoint");
    if (second) {
      auto [a1, b1] = first.pair();
      auto [a2, b2] = second.value().pair();
      ZX_ASSERT_MSG(b1 < a2, "ranges must be in order and noncontiguous");
    }
  }

  // Returns the first range and optional second range.
  const std::pair<VersionRange, std::optional<VersionRange>>& ranges() const { return ranges_; }

  // Returns true if this set contains `version`.
  bool Contains(Version version) const;

  // Returns the intersection of two (possibly empty) sets. The result must be
  // expressible as a VersionSet, i.e. not more than 2 pieces.
  static std::optional<VersionSet> Intersect(const std::optional<VersionSet>& lhs,
                                             const std::optional<VersionSet>& rhs);

  constexpr bool operator==(const VersionSet& rhs) const { return ranges_ == rhs.ranges_; }
  constexpr bool operator!=(const VersionSet& rhs) const { return ranges_ != rhs.ranges_; }
  constexpr bool operator<(const VersionSet& rhs) const { return ranges_ < rhs.ranges_; }
  constexpr bool operator<=(const VersionSet& rhs) const { return ranges_ <= rhs.ranges_; }
  constexpr bool operator>(const VersionSet& rhs) const { return ranges_ > rhs.ranges_; }
  constexpr bool operator>=(const VersionSet& rhs) const { return ranges_ >= rhs.ranges_; }

 private:
  std::pair<VersionRange, std::optional<VersionRange>> ranges_;
};

// An availability represents the versions when a FIDL element was added (A),
// deprecated (D), removed (R), and re-added as legacy (L) in a platform. These
// versions break the platform's timeline into the following regions:
//
//     Present        -- [A, R) and [L, +inf) if L is set
//         Available  -- [A, D or R)
//         Deprecated -- [D, R) if D is set
//         Legacy     -- [L, +inf) if L is set
//     Absent         -- (-inf, A) and [R, L or +inf)
//
// Here is what the timeline looks like for finite versions A, D, R:
//
//   -inf         A-1  A              D-1  D              R-1  R         +inf
//     o--- ... ---o---o--- ....... ---o---o--- ....... ---o---o--- ... ---o
//     |           |   |-- Available --|   |-- Deprecated -|   |           |
//     |-- Absent -|   |-------------- Present ------------|   |-- Absent -|
//
// Here is what the timeline looks like for a legacy element (L = LEGACY):
//
//   -inf         A-1  A              R-1  R          L-1   L          +inf
//     o--- ... ---o---o--- ....... ---o---o--- ... ---o----o---- ... ---o
//     |           |   |-- Available --|   |           |    |-- Legacy --|
//     |-- Absent -|   |--- Present ---|   |-- Absent -|    |-- Present -|
//
// Here is what the timeline looks like for Availability::Unbounded():
//
//   -inf                                                                +inf
//     o-------------------------------------------------------------------o
//     |---------------------------- Available ----------------------------|
//     |----------------------------- Present -----------------------------|
//
class Availability final {
 public:
  constexpr Availability() = default;

  // Returns an availability that exists forever. This only exists as the base
  // case for calling `Inherit`. It never occurs as a final result.
  static constexpr Availability Unbounded() {
    Availability unbounded(State::kInherited);
    unbounded.added_ = Version::kNegInf;
    unbounded.removed_ = Version::kPosInf;
    unbounded.ending_ = Ending::kNone;
    unbounded.legacy_ = Legacy::kNotApplicable;
    return unbounded;
  }

  // An availability advances through four states. All reach kNarrowed on
  // success, except for library availabilities, which stay at kInherited
  // because libraries do not get decomposed.
  enum class State : uint8_t {
    // 1. Default constructed. All fields are null.
    kUnset,
    // 2. `Init` succeeded. Some fields might be set, and they are in order.
    kInitialized,
    // 3. `Inherit` succeeded. Now `added`, `removed`, `ending`, and `legacy` are always set.
    kInherited,
    // 4. `Narrow` succeeded. Now `deprecated` is unset or equal to `added`, and
    //    `legacy` is either kNotApplicable or kNo.
    kNarrowed,
    // One of the steps failed.
    kFailed,
  };

  State state() const { return state_; }

  // Returns the points demarcating the availability: `added`, `removed`,
  // `deprecated` (if deprecated), and LEGACY and +inf (if Legacy::kYes).
  // Must be in the kInherited or kNarrowed state.
  std::set<Version> points() const;

  // Returns the presence set: [added, removed) and possibly [LEGACY, +inf).
  // Must be in the kInherited or kNarrowed state.
  VersionSet set() const;

  // Returns the presence range: [added, removed). Must be in the kNarrowed state.
  VersionRange range() const;

  // Returns true if the whole range is deprecated, and false if none of it is.
  // Must be in the kNarrowed state (where deprecation is all-or-nothing).
  bool is_deprecated() const;

  // Explicitly mark the availability as failed. Must not have called Init yet.
  void Fail();

  // Represents whether an availability includes legacy support.
  enum class Legacy : uint8_t {
    // Not applicable because [added, removed) already includes LEGACY,
    // i.e. `removed` is +inf.
    kNotApplicable,
    // No legacy support: do not re-add at LEGACY.
    kNo,
    // Legacy support: re-add at LEGACY.
    kYes,
  };

  // Meaning of the `removed` version, how this availability ends.
  enum class Ending : uint8_t {
    // This availability never ends, i.e. `removed` is +inf.
    kNone,
    // Directly marked removed=N. Validate that there IS NOT an added=N replacement.
    kRemoved,
    // Directly marked replaced=N. Validate that there IS an added=N replacement.
    kReplaced,
    // The availability ends because an ancestor was kRemoved or kReplaced.
    kInherited,
    // During decomposition, only the last piece keeps the original ending.
    // All the others get the kSplit ending.
    kSplit,
  };

  // Returns the availability's ending. Must be in the kNarrowed state.
  Ending ending() const {
    ZX_ASSERT(state_ == State::kNarrowed);
    return ending_.value();
  }

  // Named arguments for Init.
  struct InitArgs {
    std::optional<Version> added, deprecated, removed;
    bool replaced;
  };

  // Must be called first. Initializes the availability from @available fields.
  // Returns false if they do not satisfy `added <= deprecated < removed`.
  bool Init(InitArgs args);

  struct InheritResult {
    enum class Status : uint8_t {
      kOk,
      // Child {added, deprecated, or removed} < Parent added.
      kBeforeParentAdded,
      // Child deprecated > Parent deprecated.
      kAfterParentDeprecated,
      // Child {added or deprecated} >= Parent removed,
      // or Child removed > Parent removed.
      kAfterParentRemoved,
    };

    Status added = Status::kOk;
    Status deprecated = Status::kOk;
    Status removed = Status::kOk;

    bool Ok() const {
      return added == Status::kOk && deprecated == Status::kOk && removed == Status::kOk;
    }
  };

  // Must be called second. Inherits unset fields from `parent`.
  InheritResult Inherit(const Availability& parent);

  // Optionally call this after Inherit to mark the availability as legacy=true.
  // TODO(https://fxbug.dev/42085274): This is temporary for the transition from
  // the LEGACY model to the RFC-0232 model. It will be removed.
  void SetLegacy();

  // Must be called third. Narrows the availability to the given range, which
  // must be a subset of range().
  void Narrow(VersionRange range);

  // Returns a string representation of the availability for debugging, of the
  // form "<added> <deprecated> <removed> <legacy>", using "_" for null values.
  std::string Debug() const;

 private:
  constexpr explicit Availability(State state) : state_(state) {}

  bool ValidOrder() const;

  State state_ = State::kUnset;
  std::optional<Version> added_, deprecated_, removed_;
  std::optional<Ending> ending_;
  std::optional<Legacy> legacy_;
};

// A version selection is an assignment of versions to platforms.
class VersionSelection final {
 public:
  // Inserts versions. The platform must not be "unversioned". The version set
  // must be nonempty. Returns false if already called for this platform.
  bool Insert(Platform platform, std::set<Version> versions);

  // Returns true if versions were inserted for the given platform.
  bool Contains(const Platform& platform) const;

  // Returns the version for the given platform. Always returns HEAD for the
  // unversioned platform. Otherwise, assumes the platform was inserted.
  Version Lookup(const Platform& platform) const;

  // TODO(https://fxbug.dev/42085274): Remove.
  const std::set<Version>& LookupSet(const Platform& platform) const;

  // Iterator over the (platform, version) pairs.
  auto begin() const { return map_.begin(); }
  auto end() const { return map_.end(); }

 private:
  std::map<Platform, std::set<Version>, Platform::Compare> map_;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_VERSIONING_TYPES_H_
