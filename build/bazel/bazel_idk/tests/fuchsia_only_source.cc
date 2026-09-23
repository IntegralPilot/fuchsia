// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/test_lib/fuchsia_only_header.h>
#include <lib/test_lib/internal/fuchsia_only_internal_header_appears_in_api_file.h>

int PlatformSpecificFunction() { return 10 * internal_platform_value; }

int FuchsiaSpecificFunction() { return 2 * internal_platform_value; }
