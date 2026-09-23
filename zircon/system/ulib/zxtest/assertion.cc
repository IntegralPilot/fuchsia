// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zxtest/base/assertion.h>

namespace zxtest {

Assertion::Assertion(std::string_view desc, std::string_view expected,
                     std::string_view expected_eval, std::string_view actual,
                     std::string_view actual_eval, const SourceLocation& location, bool is_fatal,
                     std::span<zxtest::Message*> traces)
    : message_(desc, location),
      expected_(expected),
      expected_eval_(expected_eval),
      actual_(actual),
      actual_eval_(actual_eval),
      is_fatal_(is_fatal),
      has_values_(true),
      traces_(traces) {}

Assertion::Assertion(std::string_view desc, const SourceLocation& location, bool is_fatal,
                     std::span<zxtest::Message*> traces)
    : message_(desc, location), is_fatal_(is_fatal), has_values_(false), traces_(traces) {}

Assertion::Assertion(Assertion&& other) noexcept = default;
Assertion::~Assertion() = default;

}  // namespace zxtest
