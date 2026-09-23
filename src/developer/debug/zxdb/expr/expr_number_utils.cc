// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/expr/expr_number_utils.h"

#include <limits>

#include "src/developer/debug/zxdb/common/err.h"
#include "src/developer/debug/zxdb/common/err_or.h"
#include "src/developer/debug/zxdb/expr/expr_language.h"
#include "src/developer/debug/zxdb/expr/number_parser.h"
#include "src/developer/debug/zxdb/symbols/base_type.h"
#include "src/lib/fxl/strings/trim.h"

namespace zxdb {

Err StringToInt(const std::string& s, int* out) {
  int64_t value64;
  Err err = StringToInt64(s, &value64);
  if (err.has_error())
    return err;

  // Range check it can be stored in an int.
  if (value64 < static_cast<int64_t>(std::numeric_limits<int>::min()) ||
      value64 > static_cast<int64_t>(std::numeric_limits<int>::max()))
    return Err("This value is too large for an integer.");

  *out = static_cast<int>(value64);
  return Err();
}

Err StringToInt64(const std::string& s, int64_t* out) {
  *out = 0;

  // StringToNumber expects pre-trimmed input.
  std::string trimmed(fxl::TrimString(s, " "));

  ErrOrValue number_value = StringToNumber(ExprLanguage::kC, trimmed);
  if (number_value.has_error())
    return number_value.err();

  // Be careful to read the number out in its original sign-edness.
  if (number_value.value().GetBaseType() == BaseType::kBaseTypeUnsigned) {
    uint64_t u64;
    Err err = number_value.value().PromoteTo64(&u64);
    if (err.has_error())
      return err;

    // Range-check that the unsigned value can be put in a signed.
    if (u64 > static_cast<uint64_t>(std::numeric_limits<int64_t>::max()))
      return Err("This value is too large.");
    *out = static_cast<int64_t>(u64);
    return Err();
  }

  // Expect everything else to be a signed number.
  if (number_value.value().GetBaseType() != BaseType::kBaseTypeSigned)
    return Err("This value is not the correct type.");
  return number_value.value().PromoteTo64(out);
}

Err StringToUint32(const std::string& s, uint32_t* out) {
  // Reuses StringToUint64's and just size-checks the output.
  uint64_t value64;
  Err err = StringToUint64(s, &value64);
  if (err.has_error())
    return err;

  if (value64 > static_cast<uint64_t>(std::numeric_limits<uint32_t>::max())) {
    return Err("Expected 32-bit unsigned value, but %s is too large.", s.c_str());
  }
  *out = static_cast<uint32_t>(value64);
  return Err();
}

Err StringToUint64(const std::string& s, uint64_t* out) {
  *out = 0;

  // StringToNumber expects pre-trimmed input.
  std::string trimmed(fxl::TrimString(s, " "));

  ErrOrValue number_value = StringToNumber(ExprLanguage::kC, trimmed);
  if (number_value.has_error())
    return number_value.err();

  // Be careful to read the number out in its original sign-edness.
  if (number_value.value().GetBaseType() == BaseType::kBaseTypeSigned) {
    int64_t s64;
    Err err = number_value.value().PromoteTo64(&s64);
    if (err.has_error())
      return err;

    // Range-check that the signed value can be put in an unsigned.
    if (s64 < 0)
      return Err("This value can not be negative.");
    *out = static_cast<uint64_t>(s64);
    return Err();
  }

  // Expect everything else to be an unsigned number.
  if (number_value.value().GetBaseType() != BaseType::kBaseTypeUnsigned)
    return Err("This value is not the correct type.");
  return number_value.value().PromoteTo64(out);
}

}  // namespace zxdb
