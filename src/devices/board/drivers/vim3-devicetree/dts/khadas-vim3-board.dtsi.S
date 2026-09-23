// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
/dts-v1/;

/include/ "amlogic-a311d.dtsi"

#include <lib/ddk/platform-defs.h>
#include <soc/aml-a311d/a311d-hw.h>
#include <soc/aml-a311d/a311d-gpio.h>
#include "sdk/lib/driver/devicetree/visitors/drivers/interrupt-controllers/arm-gic/arm-gicv2.h"
#include "vim3-iommu.h"

/ {
	compatible = "khadas,vim3", "amlogic,a311d";
	model = "Khadas Vim3";

	stub_iommu: iommu {
		/* This device has no hardware IOMMU. Use unique BTI id as iommu   */
		/* specifier for each consumer device for bookkeeping system BTIs. */
		#iommu-cells = <0x1>;
	};

	suspend {
		compatible = "generic,suspend";
	};

	reserved-memory {
		#address-cells = <0x02>;
		#size-cells = <0x02>;
		ranges;

		sysmem: fuchsia,sysmem {
			compatible = "fuchsia,sysmem";
			iommus = <&stub_iommu SYSMEM_BTI>;
			pid = <PDEV_PID_AMLOGIC_A311D>;
			vid = <PDEV_VID_AMLOGIC>;
			size = <0x00 0x00>;
		};
	};

	cpu_opp_table0: opp-table-0 {
		opp-1000000000 {
			opp-hz = <0 1000000000>;
			opp-microvolt = <761000>;
		};

		opp-1200000000 {
			opp-hz = <0 1200000000>;
			opp-microvolt = <781000>;
		};

		opp-1398000000 {
			opp-hz = <0 1398000000>;
			opp-microvolt = <811000>;
		};

		opp-1512000000 {
			opp-hz = <0 1512000000>;
			opp-microvolt = <861000>;
		};

		opp-1608000000 {
			opp-hz = <0 1608000000>;
			opp-microvolt = <901000>;
		};

		opp-1704000000 {
			opp-hz = <0 1704000000>;
			opp-microvolt = <951000>;
		};

		opp-1800000000 {
			opp-hz = <0 1800000000>;
			opp-microvolt = <1001000>;
		};
	};

	cpu_opp_table1: opp-table-1 {
		opp-1000000000 {
			opp-hz = <0 1000000000>;
			opp-microvolt = <731000>;
		};

		opp-1200000000 {
			opp-hz = <0 1200000000>;
			opp-microvolt = <751000>;
		};

		opp-1398000000 {
			opp-hz = <0 1398000000>;
			opp-microvolt = <771000>;
		};

		opp-1512000000 {
			opp-hz = <0 1512000000>;
			opp-microvolt = <771000>;
		};

		opp-1608000000 {
			opp-hz = <0 1608000000>;
			opp-microvolt = <781000>;
		};

		opp-1704000000 {
			opp-hz = <0 1704000000>;
			opp-microvolt = <791000>;
		};

		opp-1800000000 {
			opp-hz = <0 1800000000>;
			opp-microvolt = <831000>;
		};

		opp-1908000000 {
			opp-hz = <0 1908000000>;
			opp-microvolt = <861000>;
		};

		opp-2016000000 {
			opp-hz = <0 2016000000>;
			opp-microvolt = <911000>;
		};

		// TODO(b/376586778): This opps doesn't seem stable at the moment, investiage why.
		// opp-2108000000 {
		// opp-hz = <0 2108000000>;
		// opp-microvolt = <951000>;
		// };
		opp-2208000000 {
			opp-hz = <0 2208000000>;
			opp-microvolt = <1011000>;
		};
	};

	function_button: adc-buttons {
		compatible = "fuchsia,adc-buttons";
		io-channels = <&saradc 2>;
		io-channel-names = "VIM3_ADC_BUTTON";
	};

	bt_uart: bt-uart@ffd24000 {
		compatible = "amlogic,bt-uart";
		reg = <0x0 A311D_UART_EE_A_BASE 0x0 A311D_UART_EE_A_LENGTH>;
		interrupts = <GIC_SPI 26 GIC_IRQ_MODE_EDGE_RISING>;
		interrupt-names = "primary";
		fuchsia,interrupt-wake-vectors = "primary";
		serial-port = <2 6 3>;
		boot-metadata = <0x43414D6D 0>;
		pinctrl-0 = <&bt_uart_pins>;
		pwm-init = <&pwm_init>;
	};

	power_button: gpio-buttons {
		compatible = "fuchsia,gpio-buttons";
		gpios = <&gpio A311D_GPIOAO(7) 0>;
		gpio-names = "power";
		pinctrl-0 = <&gpio_buttons_pins>;
	};

	wifi {
		compatible = "broadcom,bcm4359";
		gpios = <&gpio A311D_GPIOX(7) 0>;
		gpio-names = "gpio-oob";
		pinctrl-0 = <&wifi_pins>;
	};

	gpio_header {
		gpios = <&gpio A311D_GPIOH(4) 0>;
		gpio-names = "GPIO_HEADER_PIN_37";
	};
};

&gpio {
	i2c_AO_pins: i2c-AO-pins {
		pins = <A311D_GPIOAO(2) A311D_GPIOAO(2)>;
		function-id = <0 A311D_GPIOAO_2_M0_SCL_FN>;
	};

	i2c3_pins: i2c3-pins {
		pins = <A311D_GPIOA(15) A311D_GPIOA(14)>;
		function-id = <0 A311D_GPIOA_15_I2C_EE_M3_SCL_FN>;
	};

	bt_uart_pins: bt-uart-pins {
		pins = <A311D_GPIOX(12) A311D_GPIOX(13) A311D_GPIOX(14) A311D_GPIOX(15)>;
		function-id = <0 1>;
	};

	gpio_buttons_pins: gpio-buttons-pins {
		pins = <A311D_GPIOAO(7)>;
		bias-disable;
		function-id = <0 0>;
		fuchsia,interrupt-wake-vector;
	};

	fusb302_pins: fusb302-pins {
		pins = <A311D_GPIOAO(8)>;
		bias-pull-up;
		output-disable;
	};

	wifi_pins: wifi-pins {
		pins = <A311D_GPIOX(7)>;
		bias-disable;
	};
};

&i2c_AO {
	pinctrl-0 = <&i2c_AO_pins>;

	gpio_expander: gpio-controller@20 {
		compatible = "ti,tca6408a";
		reg = <0x20>;
		gpio-controller;
		#gpio-cells = <2>;
	};

	khadas-mcu@18 {
		compatible = "khadas,vim3-mcu";
		reg = <0x18>;
	};

	fusb302@22 {
		reg = <0x22>;
		gpios = <&gpio A311D_GPIOAO(8) 0>;
		gpio-names = "gpio-int";
		pinctrl-0 = <&fusb302_pins>;
	};

	rtc@51 {
		compatible = "nxp,pcf8563";
		reg = <0x51>;
	};
};

&i2c3 {
	pinctrl-0 = <&i2c3_pins>;
};
