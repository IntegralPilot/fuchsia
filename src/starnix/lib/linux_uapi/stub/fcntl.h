// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_LIB_LINUX_UAPI_STUB_FCNTL_H_
#define SRC_STARNIX_LIB_LINUX_UAPI_STUB_FCNTL_H_

// Missing splice() flags
#define SPLICE_F_MOVE 1
#define SPLICE_F_NONBLOCK 2
#define SPLICE_F_MORE 4
#define SPLICE_F_GIFT 8

#endif  // SRC_STARNIX_LIB_LINUX_UAPI_STUB_FCNTL_H_
