// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
/dts-v1/;

/include/ "meson-g12-common.dtsi"

#include <soc/aml-a311d/a311d-gpio.h>
#include <soc/aml-a311d/a311d-hw.h>
#include "sdk/lib/driver/devicetree/visitors/drivers/interrupt-controllers/arm-gic/arm-gicv2.h"
#include "vim3-iommu.h"

/ {
	compatible = "amlogic,a311d";
	#address-cells = <2>;
	#size-cells = <2>;

	cpus: cpus {
		#address-cells = <2>;
		#size-cells = <0>;

		cpu-map {
			cluster0: cluster0 {
				core0 {
					cpu = <&CPU0>;
				};

				core1 {
					cpu = <&CPU1>;
				};
			};

			cluster1: cluster1 {
				core0 {
					cpu = <&CPU2>;
				};

				core1 {
					cpu = <&CPU3>;
				};

				core2 {
					cpu = <&CPU4>;
				};

				core3 {
					cpu = <&CPU5>;
				};
			};
		};

		CPU0: cpu@0 {
			device_type = "cpu";
			compatible = "arm,cortex-a53", "arm,armv8";
			reg = <0x0 0x0>;
			enable-method = "psci";
		};

		CPU1: cpu@1 {
			device_type = "cpu";
			compatible = "arm,cortex-a53", "arm,armv8";
			reg = <0x0 0x1>;
			enable-method = "psci";
		};

		CPU2: cpu@100 {
			device_type = "cpu";
			compatible = "arm,cortex-a73", "arm,armv8";
			reg = <0x0 0x100>;
			enable-method = "psci";
		};

		CPU3: cpu@101 {
			device_type = "cpu";
			compatible = "arm,cortex-a73", "arm,armv8";
			reg = <0x0 0x101>;
			enable-method = "psci";
		};

		CPU4: cpu@102 {
			device_type = "cpu";
			compatible = "arm,cortex-a73", "arm,armv8";
			reg = <0x0 0x102>;
			enable-method = "psci";
		};

		CPU5: cpu@103 {
			device_type = "cpu";
			compatible = "arm,cortex-a73", "arm,armv8";
			reg = <0x0 0x103>;
			enable-method = "psci";
		};
	};

	pwrc: power-controller {
		compatible = "amlogic,power";
		#power-domain-cells = <1>;
		fuchsia,legacy-power;
		regulators = <&vddcpu0 &vddcpu1>;
		regulator-names = "vreg-pwm-little", "vreg-pwm-big";
	};

	canvas: canvas@ff638000 {
		compatible = "amlogic,canvas";
		reg = <0x0 0xff638000 0x0 0x2000>;
		iommus = <&stub_iommu CANVAS_BTI>;
	};

	clock: clock-controller@ff63c000 {
		compatible = "amlogic,g12b-clk";
		reg = <0x00 0xff63c000 0x00 0x2000>, <0x00 0xff620000 0x00 0x10000>, <0x00 0xffd18000 0x00 0x1000>;
		#clock-cells = <1>;
	};

	display@ff900000 {
		compatible = "amlogic,display";
		reg = <0x0 A311D_VPU_BASE 0x0 A311D_VPU_LENGTH>, <0x0 A311D_TOP_MIPI_DSI_BASE 0x0 A311D_TOP_MIPI_DSI_LENGTH>, <0x0 A311D_DSI_PHY_BASE 0x0 A311D_DSI_PHY_LENGTH>, <0x0 A311D_MIPI_DSI_BASE 0x0 A311D_MIPI_DSI_LENGTH>, <0x0 A311D_HIU_BASE 0x0 A311D_HIU_LENGTH>, <0x0 A311D_AOBUS_BASE 0x0 0x1000>, <0x0 A311D_RESET_BASE 0x0 A311D_RESET_LENGTH>, <0x0 A311D_GPIO_BASE 0x0 A311D_GPIO_LENGTH>, <0x0 A311D_HDMITX_CONTROLLER_IP_BASE 0x0 A311D_HDMITX_CONTROLLER_IP_LENGTH>, <0x0 A311D_HDMITX_TOP_LEVEL_BASE 0x0 A311D_HDMITX_TOP_LEVEL_LENGTH>;
		reg-names = "vpu", "dsi-top", "dsi-phy", "dsi-controller", "hhi", "always-on-rti", "ee-reset", "gpio-mux", "hdmitx-controller", "hdmitx-top";
		interrupts = <GIC_SPI 3 GIC_IRQ_MODE_EDGE_RISING>, <GIC_SPI 89 GIC_IRQ_MODE_EDGE_RISING>, <GIC_SPI 86 GIC_IRQ_MODE_EDGE_RISING>;
		interrupt-names = "viu1-vsync", "rdma-done", "vdin1-write-done";
		iommus = <&stub_iommu DISPLAY_BTI>;
		smcs = <2 1 0>;
		gpios = <&gpio_expander 0 0>, <&gpio A311D_GPIOH(2) 0>;
		gpio-names = "gpio-lcd-reset", "gpio-hdmi-hotplug-detect";
		pinctrl-0 = <&hdmi_hotplug_detect_pins>;
		amlogic,canvas = <&canvas>;
	};

	vddcpu0: pwm_a0_d-regulator {
		compatible = "amlogic,pwm-regulator";
		regulator-name = "vreg-pwm-little";
		pwms = <&pwm 9 1250 0>;

		// A311D_PWM_A0_D
		pinctrl-0 = <&pwm_a0_d_regulator_pins>;
		regulator-min-microvolt = <690000>;
		regulator-max-microvolt = <1050000>;
		regulator-step-microvolt = <1000>;
		#regulator-cells = <0>;
	};

	vddcpu1: pwm_a-regulator {
		compatible = "amlogic,pwm-regulator";
		regulator-name = "vreg-pwm-big";
		pwms = <&pwm 0 1250 0>;

		// A311D_PWM_A
		pinctrl-0 = <&pwm_a_regulator_pins>;
		regulator-min-microvolt = <690000>;
		regulator-max-microvolt = <1050000>;
		regulator-step-microvolt = <1000>;
		#regulator-cells = <0>;
	};
};

&gpio {
	pwm_a0_d_regulator_pins: pwm-a0-d-regulator-pins {
		pins = <A311D_GPIOE(1)>;
		function-id = <0 A311D_GPIOE_1_PWM_D_FN>;
		output-low;
	};

	pwm_a_regulator_pins: pwm-a-regulator-pins {
		pins = <A311D_GPIOE(2)>;
		function-id = <0 A311D_GPIOE_2_PWM_D_FN>;
		output-low;
	};

	hdmi_hotplug_detect_pins: hdmi-hotplug-detect-pins {
		pins = <A311D_GPIOH(2)>;
		output-disable;
		bias-pull-down;
	};
};

&aobus {
	cpu-controller@0 {
		compatible = "amlogic,cpu";
		reg = <0x0 0x0 0x0 0x1000>;
		clocks = <&clock 0x10000 &clock 0x10001 &clock 0x100000>, <&clock 0x10003 &clock 0x10004 &clock 0x100001>;
		clock-names = "sys-pll-div16", "sys-cpu-div16", "sys-cpu-big-clk", "sys-pllb-div16", "sys-cpub-div16", "sys-cpu-little-clk";
		power-domains = <&pwrc 0 &pwrc 1>;

		performance-domains {
			a311d-arm-a73-domain {
				domain-id = <1>;
				relative-performance = <255>;
				cpus = <&CPU2 &CPU3 &CPU4 &CPU5>;
				operating-points = <&cpu_opp_table1>;
			};

			a311d-arm-a53-domain {
				domain-id = <2>;
				relative-performance = <112>;
				cpus = <&CPU0 &CPU1>;
				operating-points = <&cpu_opp_table0>;
			};
		};
	};
};
