// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/expr/abi_riscv64.h"

#include "src/developer/debug/zxdb/symbols/base_type.h"
#include "src/developer/debug/zxdb/symbols/collection.h"

namespace zxdb {

using debug::RegisterID;

bool AbiRiscv64::IsRegisterCalleeSaved(debug::RegisterID reg) const {
  switch (reg) {
    case debug::RegisterID::kRiscv64_sp:
    case debug::RegisterID::kRiscv64_s0:
    case debug::RegisterID::kRiscv64_s1:
    case debug::RegisterID::kRiscv64_s2:
    case debug::RegisterID::kRiscv64_s3:
    case debug::RegisterID::kRiscv64_s4:
    case debug::RegisterID::kRiscv64_s5:
    case debug::RegisterID::kRiscv64_s6:
    case debug::RegisterID::kRiscv64_s7:
    case debug::RegisterID::kRiscv64_s8:
    case debug::RegisterID::kRiscv64_s9:
    case debug::RegisterID::kRiscv64_s10:
    case debug::RegisterID::kRiscv64_s11:
      return true;
    default:
      return false;
  }
}

std::optional<debug::RegisterID> AbiRiscv64::GetReturnRegisterForBaseType(
    const BaseType* base_type) {
  // Not implemented.
  return std::nullopt;
}

std::optional<AbiRiscv64::CollectionReturn> AbiRiscv64::GetCollectionReturnByRefLocation(
    const Collection* collection) {
  // Not implemented.
  return std::nullopt;
}

std::optional<Abi::CollectionByValueReturn> AbiRiscv64::GetCollectionReturnByValueLocation(
    const fxl::RefPtr<EvalContext>& eval_context, const Collection* collection) {
  // Not implemented.
  return std::nullopt;
}

std::optional<std::vector<debug::RegisterID>> AbiRiscv64::GetFunctionParameterRegisters() {
  return std::vector({
      debug::RegisterID::kRiscv64_a0,
      debug::RegisterID::kRiscv64_a1,
      debug::RegisterID::kRiscv64_a2,
      debug::RegisterID::kRiscv64_a3,
      debug::RegisterID::kRiscv64_a4,
      debug::RegisterID::kRiscv64_a5,
      debug::RegisterID::kRiscv64_a6,
      debug::RegisterID::kRiscv64_a7,
  });
}

}  // namespace zxdb
