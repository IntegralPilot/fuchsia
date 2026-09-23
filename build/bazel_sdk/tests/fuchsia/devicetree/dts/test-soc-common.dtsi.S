// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/dts-v1/;

#include "fuchsia/devicetree/test_soc/include/soc/hw.h"
#include "../test_soc/include/soc/gpio.h"
#include <gic/gic.h>

/ {
	interrupt-parent = <&gic>;

	soc {
		#address-cells = <2>;
		#size-cells = <2>;
		ranges;

		gpio: gpio-controller@ff634400 {
			compatible = "test-soc,gpio";
			reg = <0x0 TEST_SOC_GPIO_BASE 0x0 TEST_SOC_GPIO_LENGTH>;
			interrupts = <GIC_SPI 64 GIC_IRQ_MODE_EDGE_RISING>;
			gpio-controller;
			#gpio-cells = <2>;

			usb_power: usb-power {
				pins = <TEST_SOC_GPIOA(6)>;
				output-high;
			};
		};

		gic: interrupt-controller@ffc01000 {
			compatible = "test-gic";
			reg = <0x0 0xffc01000 0 0x1000>;
            interrupt-controller;
            #interrupt-cells = <3>;
            #address-cells = <0>;
		};
	};
};
