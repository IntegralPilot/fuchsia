// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/layout.h>
#include <lib/ld/tlsdesc.h>

#include <functional>
#include <limits>
#include <optional>
#include <tuple>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "../diagnostics.h"
#include "../tls-desc-resolver.h"

namespace {

using Elf = elfldltl::Elf<>;
using Addr = Elf::Addr;
using size_type = Elf::size_type;
using TlsDescGot = Elf::TlsDescGot<>;
using GotValueType = Elf::GotEntry<>::value_type;

// This is the tiny subset of the <lib/elfldltl/resolve.h> Definition API
// that dl::TlsDescResolver actually uses.
class MockDefinition {
 public:
  using MaybeBias = std::optional<size_type>;

  constexpr MockDefinition(size_type id, Addr value,
                           MaybeBias static_tls_bias = std::nullopt) noexcept
      : sym_{.value = value}, id_{id}, static_tls_bias_{static_tls_bias} {}

  size_type tls_module_id() const {
    EXPECT_NE(id_, 0u);
    return id_;
  }

  constexpr size_type static_tls_bias() const {
    assert(static_tls_bias_);
    return *static_tls_bias_;
  }

  const Elf::Sym& symbol() const { return sym_; }

  constexpr bool undefined_weak() const { return false; }

 private:
  Elf::Sym sym_;
  size_type id_ = 0;
  MaybeBias static_tls_bias_;
};

// The bound matcher argument has to be copyable or movable, so this takes a
// std::reference_wrapper<dl::Diagnostics>.  The subject arg the matcher is
// applied to is a fit::result<bool, ...>.  If this matcher returns true, then
// it's safe to use `.value()` et al on that.  Otherwise, this gets the error
// message from the dl::Diagnostics::take_error().
MATCHER_P(IsOkMatcher, diag_ref, "") {
  dl::Diagnostics& diag = diag_ref;
  if (arg.is_ok()) {
    // The Diagnostics object needs to be notified of success.
    fit::result<dl::Error, bool> result = diag.ok(true);
    return *result;
  }
  fit::result<dl::Error> err = diag.take_error();
  *result_listener << "failed (" << std::boolalpha << arg.error_value()
                   << "): " << std::move(err).error_value().take().take_str();
  return false;
}

// This makes it more natural to use in each test.
auto IsOk(dl::Diagnostics& diag) { return IsOkMatcher(std::ref(diag)); }

// These are just testing the inherited cases and should be more or less
// redundant with tests for ld::LocalRuntimeTlsDescResolver.

TEST(DlTlsDescResolverTests, UndefinedWeak) {
  dl::TlsdescIndirectList indirect;
  dl::TlsDescResolver resolver{0, indirect};
  TlsDescGot got = resolver(0);
  EXPECT_EQ(got.function, ld::LocalRuntimeTlsDescResolver::kRuntimeUndefinedWeak);
  EXPECT_EQ(got.value, 0u);
  EXPECT_TRUE(indirect.is_empty());
}

TEST(DlTlsDescResolverTests, UndefinedWeakAddend) {
  dl::TlsdescIndirectList indirect;
  dl::TlsDescResolver resolver{0, indirect};
  TlsDescGot got = resolver(1);
  EXPECT_EQ(got.function, ld::LocalRuntimeTlsDescResolver::kRuntimeUndefinedWeakAddend);
  EXPECT_EQ(got.value, 1u);
  EXPECT_TRUE(indirect.is_empty());
}

TEST(DlTlsDescResolverTests, Static) {
  dl::TlsdescIndirectList indirect;
  dl::TlsDescResolver resolver{1, indirect};
  dl::Diagnostics diag;
  auto got = resolver(diag, MockDefinition{1, 0x10, 0x20});
  ASSERT_THAT(got, IsOk(diag));
  EXPECT_EQ(got->function, ld::LocalRuntimeTlsDescResolver::kRuntimeStatic);
  EXPECT_EQ(got->value, 0x10u + 0x20u);
  EXPECT_TRUE(indirect.is_empty());
}

// These test the main TlsDescResolver logic.

const Addr kRuntimeSplit = reinterpret_cast<uintptr_t>(dl::_dl_tlsdesc_runtime_dynamic_split);

const Addr kRuntimeIndirect = reinterpret_cast<uintptr_t>(dl::_dl_tlsdesc_runtime_dynamic_indirect);

constexpr int kSplitBits = std::numeric_limits<GotValueType>::digits / 2;
constexpr GotValueType kSplitMax = (GotValueType{1} << kSplitBits) - 1;

constexpr GotValueType SplitValue(size_type index, size_type offset) {
  return (index << kSplitBits) | offset;
}

TEST(DlTlsDescResolverTests, DynamicSplit) {
  dl::TlsdescIndirectList indirect;
  dl::TlsDescResolver resolver{7, indirect};
  dl::Diagnostics diag;
  auto got = resolver(diag, MockDefinition{8, 0x1234});
  ASSERT_THAT(got, IsOk(diag));
  EXPECT_EQ(got->function, kRuntimeSplit);
  EXPECT_EQ(got->value, SplitValue(0, 0x1234));
  EXPECT_TRUE(indirect.is_empty());
}

TEST(DlTlsDescResolverTests, DynamicSplitMax) {
  dl::TlsdescIndirectList indirect;
  dl::TlsDescResolver resolver{7, indirect};
  {
    dl::Diagnostics diag;
    auto got = resolver(diag, MockDefinition{kSplitMax + 8, 0x1234});
    ASSERT_THAT(got, IsOk(diag));
    EXPECT_EQ(got->function, kRuntimeSplit);
    EXPECT_EQ(got->value, SplitValue(kSplitMax, 0x1234));
    EXPECT_TRUE(indirect.is_empty());
  }
  {
    dl::Diagnostics diag;
    auto got = resolver(diag, MockDefinition{108, kSplitMax});
    ASSERT_THAT(got, IsOk(diag));
    EXPECT_EQ(got->function, kRuntimeSplit);
    EXPECT_EQ(got->value, SplitValue(100, kSplitMax));
    EXPECT_TRUE(indirect.is_empty());
  }
}

TEST(DlTlsDescResolverTests, DynamicIndirect) {
  dl::TlsdescIndirectList indirect;
  dl::TlsDescResolver resolver{17, indirect};
  {
    dl::Diagnostics diag;
    auto got = resolver(diag, MockDefinition{23, kSplitMax + 0x123});
    ASSERT_THAT(got, IsOk(diag));
    EXPECT_EQ(indirect.size_slow(), 1u);
    EXPECT_EQ(got->function, kRuntimeIndirect);
    ASSERT_FALSE(indirect.is_empty());
    EXPECT_EQ(got->value, indirect.front().got_value());
    const uintptr_t value = static_cast<uintptr_t>(got->value);
    ASSERT_EQ(got->value, value);
    const auto* ptr = reinterpret_cast<const dl::TlsdescIndirect*>(value);
    ASSERT_NE(ptr, nullptr);
    EXPECT_EQ(ptr->index, 5u);
    EXPECT_EQ(ptr->offset, kSplitMax + 0x123);
  }
  {
    dl::Diagnostics diag;
    auto got = resolver(diag, MockDefinition{kSplitMax + 123, 0x1234});
    ASSERT_THAT(got, IsOk(diag));
    EXPECT_EQ(indirect.size_slow(), 2u);
    EXPECT_EQ(got->function, kRuntimeIndirect);
    const uintptr_t value = static_cast<uintptr_t>(got->value);
    ASSERT_EQ(got->value, value);
    const auto* ptr = reinterpret_cast<const dl::TlsdescIndirect*>(value);
    ASSERT_NE(ptr, nullptr);
    EXPECT_EQ(ptr->index, kSplitMax + 105);
    EXPECT_EQ(ptr->offset, 0x1234u);
  }
}

}  // namespace
