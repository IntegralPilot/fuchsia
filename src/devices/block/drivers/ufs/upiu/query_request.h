// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_UPIU_QUERY_REQUEST_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_UPIU_QUERY_REQUEST_H_

#include "upiu_transactions.h"

namespace ufs {

class QueryReadRequestUpiu : public QueryRequestUpiu {
 public:
  explicit QueryReadRequestUpiu(QueryOpcode query_opcode, uint8_t type, uint8_t index = 0)
      : QueryRequestUpiu(QueryFunction::kStandardReadRequest, query_opcode, type, index) {}
};

class QueryWriteRequestUpiu : public QueryRequestUpiu {
 public:
  explicit QueryWriteRequestUpiu(QueryOpcode query_opcode, uint8_t type, uint8_t index = 0)
      : QueryRequestUpiu(QueryFunction::kStandardWriteRequest, query_opcode, type, index) {}
};

}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_UPIU_QUERY_REQUEST_H_
