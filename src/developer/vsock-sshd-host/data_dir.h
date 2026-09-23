// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_DEVELOPER_VSOCK_SSHD_HOST_DATA_DIR_H_
#define SRC_DEVELOPER_VSOCK_SSHD_HOST_DATA_DIR_H_

#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/pseudo_dir.h"

fbl::RefPtr<fs::PseudoDir> BuildDataDir();

#endif  // SRC_DEVELOPER_VSOCK_SSHD_HOST_DATA_DIR_H_
