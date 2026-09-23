// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/bringup/bin/netsvc/netcp.h"

#include <errno.h>
#include <fcntl.h>

#include <string>

#include <zxtest/zxtest.h>

namespace {

TEST(NetcpTest, OpenWriteRejectsOverlongPathPrefix) {
  // Use one "." followed by many slashes and then a missing directory.
  // The OS path parser collapses repeated slashes, so open() resolves this
  // like "./missing/file.bin" and fails with ENOENT instead of hitting VFS
  // component limits. netcp_mkdir(), however, scans the raw string slash by
  // slash and trips its 1024-byte prefix bound before any mkdir() call.
  std::string path = ".";
  path.append(1024, '/');
  path += "missing/file.bin";

  ASSERT_GT(path.find_last_of('/'), static_cast<size_t>(1023));

  ASSERT_EQ(netcp_open(path.c_str(), O_WRONLY, nullptr), -ENAMETOOLONG);
}

}  // namespace
