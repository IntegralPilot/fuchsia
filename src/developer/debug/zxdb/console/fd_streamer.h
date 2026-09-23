// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_FD_STREAMER_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_FD_STREAMER_H_

#include <memory>

#include "src/developer/debug/shared/buffered_fd.h"
#include "src/developer/debug/zxdb/console/console.h"

namespace zxdb {

std::unique_ptr<debug::BufferedFD> StreamFDToConsole(fbl::unique_fd fd, Console* console);

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_FD_STREAMER_H_
