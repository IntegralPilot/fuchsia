
// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_INPUT_STREAM_ID_H_
#define SRC_UI_SCENIC_LIB_INPUT_STREAM_ID_H_

#include <stdint.h>

namespace scenic_impl::input {

// TODO(https://fxbug.dev/https://fxbug.dev/42153191): Rename all instances of "stream" to
// "interaction".
using StreamId = uint64_t;
constexpr StreamId kInvalidStreamId = 0;
StreamId NewStreamId();

}  // namespace scenic_impl::input

#endif  // SRC_UI_SCENIC_LIB_INPUT_STREAM_ID_H_
