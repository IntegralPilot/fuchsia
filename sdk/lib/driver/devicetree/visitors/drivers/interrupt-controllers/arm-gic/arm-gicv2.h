// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_INTERRUPT_CONTROLLERS_ARM_GIC_ARM_GICV2_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_INTERRUPT_CONTROLLERS_ARM_GIC_ARM_GICV2_H_

#define GIC_PPI 1
#define GIC_SPI 0

#define GIC_IRQ_MODE_EDGE_RISING (1 << 0)
#define GIC_IRQ_MODE_EDGE_FALLING (1 << 1)
#define GIC_IRQ_MODE_LEVEL_HIGH (1 << 2)
#define GIC_IRQ_MODE_LEVEL_LOW (1 << 3)

#define GIC_CPU_MASK_RAW(x) ((x) << 8)
// This flag is currently not used by the zircon kernel GIC. It is included in the devicetree for
// parity. This may be used in the future.
#define GIC_CPU_MASK_SIMPLE(num) GIC_CPU_MASK_RAW((1 << (num)) - 1)

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_INTERRUPT_CONTROLLERS_ARM_GIC_ARM_GICV2_H_
