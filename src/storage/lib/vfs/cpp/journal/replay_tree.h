// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_VFS_CPP_JOURNAL_REPLAY_TREE_H_
#define SRC_STORAGE_LIB_VFS_CPP_JOURNAL_REPLAY_TREE_H_

#include "src/storage/lib/operation/operation.h"
#include "src/storage/lib/operation/operation_tree.h"

namespace fs {

using ReplayTree = storage::OperationTree<storage::BufferedOperation>;

}  // namespace fs

#endif  // SRC_STORAGE_LIB_VFS_CPP_JOURNAL_REPLAY_TREE_H_
