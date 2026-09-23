// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/dts-v1/;

/include/ "test-soc-common.dtsi"

#include <soc/gpio.h>
#include <soc/hw.h>
#include <gic/gic.h>

/ {
	compatible = "test-soc";
	#address-cells = <2>;
	#size-cells = <2>;

	cpus:cpus {
		#address-cells = <2>;
		#size-cells = <0>;

		cpu-map {
			cluster0:cluster0 {
				core0 {
					cpu = <&CPU0>;
				};
			};
		};

		CPU0:cpu@0 {
			device_type = "cpu";
			compatible = "arm,cortex-a53","arm,armv8";
			reg = <0x0 0x0>;
			enable-method = "psci";
		};

	};
};

&gpio {
	pwm_a0_d_regulator_pins: pwm-a0-d-regulator-pins {
		pins = <TEST_SOC_GPIOE(1)>;
		function-id = <0 TEST_SOC_GPIOE_1_PWM_D_FN>;
		output-low;
	};
};
