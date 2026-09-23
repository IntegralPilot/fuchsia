// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_TESTS_VKEXT_CONFIG_QUERY_H_
#define SRC_GRAPHICS_TESTS_VKEXT_CONFIG_QUERY_H_

bool SupportsSysmemYuv();

bool SupportsSysmemYv12();

bool SupportsSysmemA2B10G10R10();

bool SupportsSysmemRenderableLinear();

bool SupportsSysmemLinearNonRgba();

bool SupportsProtectedMemory();

#endif  // SRC_GRAPHICS_TESTS_VKEXT_CONFIG_QUERY_H_
