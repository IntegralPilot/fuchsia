// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/versioning_types.h"

#include <zircon/assert.h>

#include <sstream>

#include "tools/fidl/fidlc/src/utils.h"

namespace fidlc {

// static
std::optional<Platform> Platform::Parse(std::string str) {
  if (IsValidLibraryComponent(str)) {
    return Platform(std::move(str));
  }
  return std::nullopt;
}

const uint32_t kMaxNormalVersion = (1ul << 31) - 1;

std::optional<Version> Version::From(uint32_t number) {
  for (auto& version : kSpecialVersions) {
    if (number == version.value_)
      return version;
  }
  if (number == 0 || number > kMaxNormalVersion)
    return std::nullopt;
  return Version(number);
}

std::optional<Version> Version::Parse(std::string_view str) {
  // We need this check because ParseNumeric returns 0 for an empty string.
  if (str.empty()) {
    return std::nullopt;
  }
  for (auto& version : kSpecialVersions) {
    if (str == version.name())
      return version;
  }
  uint32_t value;
  if (ParseNumeric(str, &value) != ParseNumericResult::kSuccess) {
    return std::nullopt;
  }
  return From(value);
}

std::string_view Version::name() const {
  switch (value_) {
    case Version::kNext.value_:
      return "NEXT";
    case Version::kHead.value_:
      return "HEAD";
    case Version::kLegacy.value_:
      return "LEGACY";
    default:
      ZX_PANIC("expected a special version");
  }
}

std::string Version::ToString() const {
  switch (value_) {
    case Version::kNegInf.value_:
      return "-inf";
    case Version::kPosInf.value_:
      return "+inf";
    case Version::kNext.value_:
    case Version::kHead.value_:
    case Version::kLegacy.value_:
      return std::string(name());
    default:
      return std::to_string(value_);
  }
}

Version Version::Predecessor() const {
  ZX_ASSERT(*this != kNegInf && *this != kPosInf && *this != Version(1));
  if (*this == kSpecialVersions[0])
    return Version(kMaxNormalVersion);
  for (size_t i = 1; i < std::size(kSpecialVersions); ++i) {
    if (*this == kSpecialVersions[i])
      return kSpecialVersions[i - 1];
  }
  return Version(value_ - 1);
}

Version Version::Successor() const {
  ZX_ASSERT(*this != kNegInf && *this != kPosInf &&
            *this != kSpecialVersions[std::size(kSpecialVersions) - 1]);
  if (*this == Version(kMaxNormalVersion))
    return kSpecialVersions[0];
  for (size_t i = 0; i < std::size(kSpecialVersions) - 1; ++i) {
    if (*this == kSpecialVersions[i])
      return kSpecialVersions[i + 1];
  }
  return Version(value_ + 1);
}

bool VersionRange::Contains(Version version) const {
  auto [a, b] = pair_;
  return a <= version && version < b;
}

// static
std::optional<VersionRange> VersionRange::Intersect(const std::optional<VersionRange>& lhs,
                                                    const std::optional<VersionRange>& rhs) {
  if (!lhs || !rhs) {
    return std::nullopt;
  }
  auto [a1, b1] = lhs.value().pair_;
  auto [a2, b2] = rhs.value().pair_;
  if (b1 <= a2 || b2 <= a1) {
    return std::nullopt;
  }
  return VersionRange(std::max(a1, a2), std::min(b1, b2));
}

bool VersionSet::Contains(Version version) const {
  auto& [x, maybe_y] = ranges_;
  return x.Contains(version) || (maybe_y && maybe_y.value().Contains(version));
}

// static
std::optional<VersionSet> VersionSet::Intersect(const std::optional<VersionSet>& lhs,
                                                const std::optional<VersionSet>& rhs) {
  if (!lhs || !rhs) {
    return std::nullopt;
  }
  auto& [x1, x2] = lhs.value().ranges_;
  auto& [y1, y2] = rhs.value().ranges_;
  std::optional<VersionRange> z1, z2;
  for (auto range : {
           VersionRange::Intersect(x1, y1),
           VersionRange::Intersect(x1, y2),
           VersionRange::Intersect(x2, y1),
           VersionRange::Intersect(x2, y2),
       }) {
    if (!range) {
      continue;
    }
    if (!z1) {
      z1 = range;
    } else if (!z2) {
      z2 = range;
    } else {
      ZX_PANIC("set intersection is more than two pieces");
    }
  }
  if (!z1) {
    ZX_ASSERT(!z2);
    return std::nullopt;
  }
  return VersionSet(z1.value(), z2);
}

VersionSet Availability::set() const {
  ZX_ASSERT(state_ == State::kInherited || state_ == State::kNarrowed);
  VersionRange range(added_.value(), removed_.value());
  switch (legacy_.value()) {
    case Legacy::kNotApplicable:
    case Legacy::kNo:
      return VersionSet(range);
    case Legacy::kYes:
      return VersionSet(range, VersionRange(Version::kLegacy, Version::kPosInf));
  }
}

std::set<Version> Availability::points() const {
  ZX_ASSERT(state_ == State::kInherited || state_ == State::kNarrowed);
  std::set<Version> result{added_.value(), removed_.value()};
  if (deprecated_) {
    result.insert(deprecated_.value());
  }
  if (legacy_.value() == Legacy::kYes) {
    ZX_ASSERT(result.insert(Version::kLegacy).second);
    ZX_ASSERT(result.insert(Version::kPosInf).second);
  }
  return result;
}

VersionRange Availability::range() const {
  ZX_ASSERT(state_ == State::kNarrowed);
  return VersionRange(added_.value(), removed_.value());
}

bool Availability::is_deprecated() const {
  ZX_ASSERT(state_ == State::kNarrowed);
  return deprecated_.has_value();
}

void Availability::Fail() {
  ZX_ASSERT_MSG(state_ == State::kUnset, "called Fail in the wrong order");
  state_ = State::kFailed;
}

bool Availability::Init(InitArgs args) {
  ZX_ASSERT_MSG(state_ == State::kUnset, "called Init in the wrong order");
  ZX_ASSERT_MSG(args.removed || !args.replaced, "cannot set replaced without removed");
  for (auto version : {args.added, args.deprecated, args.removed}) {
    ZX_ASSERT(version != Version::kNegInf);
    ZX_ASSERT(version != Version::kPosInf);
    ZX_ASSERT(version != Version::kLegacy);
  }
  added_ = args.added;
  deprecated_ = args.deprecated;
  removed_ = args.removed;
  if (args.removed) {
    ending_ = args.replaced ? Ending::kReplaced : Ending::kRemoved;
  }
  bool valid = ValidOrder();
  state_ = valid ? State::kInitialized : State::kFailed;
  return valid;
}

bool Availability::ValidOrder() const {
  auto a = added_.value_or(Version::kNegInf);
  auto d = deprecated_.value_or(a);
  auto r = removed_.value_or(Version::kPosInf);
  return a <= d && d < r;
}

Availability::InheritResult Availability::Inherit(const Availability& parent) {
  ZX_ASSERT_MSG(state_ == State::kInitialized, "called Inherit in the wrong order");
  ZX_ASSERT_MSG(parent.state_ == State::kInherited, "must call Inherit on parent first");
  InheritResult result;
  // Inherit and validate `added`.
  if (!added_) {
    added_ = parent.added_.value();
  } else if (added_.value() < parent.added_.value()) {
    result.added = InheritResult::Status::kBeforeParentAdded;
  } else if (added_.value() >= parent.removed_.value()) {
    result.added = InheritResult::Status::kAfterParentRemoved;
  }
  // Inherit and validate `removed`.
  if (!removed_) {
    removed_ = parent.removed_.value();
  } else if (removed_.value() <= parent.added_.value()) {
    result.removed = InheritResult::Status::kBeforeParentAdded;
  } else if (removed_.value() > parent.removed_.value()) {
    result.removed = InheritResult::Status::kAfterParentRemoved;
  }
  // Inherit and validate `deprecated`.
  if (!deprecated_) {
    // Only inherit deprecation if it occurs before this element is removed.
    if (parent.deprecated_ && parent.deprecated_.value() < removed_.value()) {
      // As a result of inheritance, we can end up with deprecated < added:
      //
      //     @available(added=1, deprecated=5, removed=10)
      //     type Foo = struct {
      //         @available(added=7)
      //         bar bool;
      //     };
      //
      // To maintain `added <= deprecated < removed` in this case, we use
      // std::max below. A different choice would be to disallow this, and
      // consider `Foo` frozen once deprecated. However, going down this path
      // leads to contradictions with the overall design of FIDL Versioning.
      deprecated_ = std::max(parent.deprecated_.value(), added_.value());
    }
  } else if (deprecated_.value() < parent.added_.value()) {
    result.deprecated = InheritResult::Status::kBeforeParentAdded;
  } else if (deprecated_.value() >= parent.removed_.value()) {
    result.deprecated = InheritResult::Status::kAfterParentRemoved;
  } else if (parent.deprecated_ && deprecated_.value() > parent.deprecated_.value()) {
    result.deprecated = InheritResult::Status::kAfterParentDeprecated;
  }
  // Inherit and validate `ending`.
  if (!ending_) {
    ending_ = parent.ending_.value() == Ending::kNone ? Ending::kNone : Ending::kInherited;
  } else if (ending_.value() == Ending::kReplaced && removed_.value() == parent.removed_.value()) {
    result.removed = InheritResult::Status::kAfterParentRemoved;
  }
  // Inherit `legacy`.
  ZX_ASSERT_MSG(!legacy_.has_value(), "legacy cannot be set before Inherit");
  if (removed_.value() == parent.removed_.value()) {
    // Only inherit if the parent was removed at the same time. For example:
    //
    //     @available(added=1, removed=3)
    //     type Foo = table {
    //         1: string bar;
    //         @available(removed=2) 2: string baz;
    //     };
    //
    // When we add back Foo at LEGACY, it should appear the same as it did at 2,
    // i.e. it should only have the bar member, not the baz member.
    legacy_ = parent.legacy_.value();
  } else {
    ZX_ASSERT_MSG(
        removed_.value() != Version::kPosInf,
        "impossible for child to be removed at +inf if parent is not also removed at +inf");
    // By default, removed elements are not added back at LEGACY.
    legacy_ = Legacy::kNo;
  }

  if (result.Ok()) {
    ZX_ASSERT(added_ && removed_ && ending_ && legacy_);
    ZX_ASSERT(added_.value() != Version::kNegInf);
    ZX_ASSERT(ValidOrder());
    state_ = State::kInherited;
  } else {
    state_ = State::kFailed;
  }
  return result;
}

void Availability::SetLegacy() {
  ZX_ASSERT_MSG(state_ == State::kInherited, "called SetLegacy in the wrong order");
  ZX_ASSERT_MSG(legacy_.has_value(), "legacy_ should be set by Inherit");
  ZX_ASSERT_MSG(removed_.value() != Version::kPosInf, "called SetLegacy for non-removed element");
  legacy_ = Legacy::kYes;
}

void Availability::Narrow(VersionRange range) {
  ZX_ASSERT_MSG(state_ == State::kInherited, "called Narrow in the wrong order");
  auto [a, b] = range.pair();
  if (a == Version::kLegacy) {
    ZX_ASSERT_MSG(b == Version::kPosInf, "legacy range must be [LEGACY, +inf)");
    ZX_ASSERT_MSG(legacy_.value() != Legacy::kNo, "must be present at LEGACY");
  } else {
    ZX_ASSERT_MSG(a >= added_ && b <= removed_, "must narrow to a subrange");
  }
  if (b == Version::kPosInf) {
    ending_ = Ending::kNone;
  } else if (removed_ != b) {
    ending_ = Ending::kSplit;
  }
  added_ = a;
  removed_ = b;
  if (deprecated_ && a >= deprecated_.value()) {
    deprecated_ = a;
  } else {
    deprecated_ = std::nullopt;
  }
  if (a <= Version::kLegacy && b > Version::kLegacy) {
    legacy_ = Legacy::kNotApplicable;
  } else {
    legacy_ = Legacy::kNo;
  }
  state_ = State::kNarrowed;
}

template <typename T>
static std::string ToString(const std::optional<T>& opt) {
  return opt ? ToString(opt.value()) : "_";
}

static std::string ToString(const Version& version) { return version.ToString(); }

static std::string ToString(Availability::Legacy legacy) {
  switch (legacy) {
    case Availability::Legacy::kNotApplicable:
      return "n/a";
    case Availability::Legacy::kNo:
      return "no";
    case Availability::Legacy::kYes:
      return "yes";
  }
}

std::string Availability::Debug() const {
  std::stringstream ss;
  ss << ToString(added_) << ' ' << ToString(deprecated_) << ' ' << ToString(removed_) << ' '
     << ToString(legacy_);
  return ss.str();
}

bool VersionSelection::Insert(Platform platform, std::set<Version> versions) {
  ZX_ASSERT_MSG(!platform.is_unversioned(), "version selection cannot contain 'unversioned'");
  ZX_ASSERT_MSG(!versions.empty(), "cannot select an empty set of versions");
  ZX_ASSERT_MSG(versions.count(Version::kLegacy) == 0, "targeting LEGACY is not allowed");
  // TODO(https://fxbug.dev/42085274): Remove this restriction.
  if (versions.size() > 1) {
    ZX_ASSERT_MSG(versions.count(Version::kHead) != 0,
                  "HEAD must be included when targeting multiple levels");
  }
  auto [_, inserted] = map_.emplace(std::move(platform), std::move(versions));
  return inserted;
}

bool VersionSelection::Contains(const Platform& platform) const {
  ZX_ASSERT_MSG(!platform.is_unversioned(), "version selection cannot contain 'unversioned'");
  return map_.count(platform) != 0;
}

Version VersionSelection::Lookup(const Platform& platform) const {
  if (platform.is_unversioned()) {
    return Version::kHead;
  }
  const auto iter = map_.find(platform);
  ZX_ASSERT_MSG(iter != map_.end(), "no version was inserted for platform '%s'",
                platform.name().c_str());
  auto& versions = iter->second;
  // TODO(https://fxbug.dev/42085274): Temporary, for aligning legacy=true with supported levels.
  return versions.size() == 1 ? *versions.begin() : Version::kLegacy;
}

const std::set<Version> kOnlyHead{Version::kHead};

const std::set<Version>& VersionSelection::LookupSet(const Platform& platform) const {
  if (platform.is_unversioned())
    return kOnlyHead;
  const auto iter = map_.find(platform);
  ZX_ASSERT_MSG(iter != map_.end(), "no version was inserted for platform '%s'",
                platform.name().c_str());
  return iter->second;
}

}  // namespace fidlc
