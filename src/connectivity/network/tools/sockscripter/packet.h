// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_TOOLS_SOCKSCRIPTER_PACKET_H_
#define SRC_CONNECTIVITY_NETWORK_TOOLS_SOCKSCRIPTER_PACKET_H_

#include "src/lib/fxl/build_config.h"

#if OS_LINUX
#define PACKET_SOCKETS 1
#elif OS_FUCHSIA
#define PACKET_SOCKETS 1
#endif

#endif  // SRC_CONNECTIVITY_NETWORK_TOOLS_SOCKSCRIPTER_PACKET_H_
