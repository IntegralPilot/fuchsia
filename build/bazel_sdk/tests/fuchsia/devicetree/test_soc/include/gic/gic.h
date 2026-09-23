// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef BUILD_BAZEL_SDK_TESTS_FUCHSIA_DEVICETREE_TEST_SOC_INCLUDE_GIC_GIC_H_
#define BUILD_BAZEL_SDK_TESTS_FUCHSIA_DEVICETREE_TEST_SOC_INCLUDE_GIC_GIC_H_

#define GIC_PPI 1
#define GIC_SPI 0

#define GIC_IRQ_MODE_EDGE_RISING (1 << 0)
#define GIC_IRQ_MODE_EDGE_FALLING (1 << 1)
#define GIC_IRQ_MODE_LEVEL_HIGH (1 << 2)
#define GIC_IRQ_MODE_LEVEL_LOW (1 << 3)

#endif  // BUILD_BAZEL_SDK_TESTS_FUCHSIA_DEVICETREE_TEST_SOC_INCLUDE_GIC_GIC_H_
