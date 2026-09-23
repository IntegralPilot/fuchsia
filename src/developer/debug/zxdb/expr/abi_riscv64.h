// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_EXPR_ABI_RISCV64_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_EXPR_ABI_RISCV64_H_

#include "src/developer/debug/zxdb/expr/abi.h"

namespace zxdb {

class Type;

class AbiRiscv64 : public Abi {
 public:
  // Abi implementation.
  bool IsRegisterCalleeSaved(debug::RegisterID reg) const final;
  debug::RegisterID GetReturnRegisterForMachineInt() const final {
    return debug::RegisterID::kRiscv64_a0;
  }
  std::optional<debug::RegisterID> GetReturnRegisterForBaseType(const BaseType* base_type) final;
  std::optional<CollectionReturn> GetCollectionReturnByRefLocation(
      const Collection* collection) final;
  std::optional<CollectionByValueReturn> GetCollectionReturnByValueLocation(
      const fxl::RefPtr<EvalContext>& eval_context, const Collection* collection) final;

  std::optional<std::vector<debug::RegisterID>> GetFunctionParameterRegisters() final;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_EXPR_ABI_RISCV64_H_
