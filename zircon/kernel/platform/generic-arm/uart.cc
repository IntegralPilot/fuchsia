// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <cstdint>

#include <platform/uart.h>

ktl::optional<uint32_t> PlatformUartGetIrqNumber(uint32_t irq_num) { return irq_num; }
