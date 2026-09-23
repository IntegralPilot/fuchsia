// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/test_lib/internal/non_fuchsia_internal_header_does_not_appear_in_api_file.h>

int PlatformSpecificFunction() { return -internal_platform_value; }
