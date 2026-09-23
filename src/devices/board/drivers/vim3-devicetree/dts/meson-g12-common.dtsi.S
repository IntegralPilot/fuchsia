// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
/dts-v1/;

#include "src/devices/lib/amlogic/include/soc/aml-a311d/a311d-hw.h"
#include "src/devices/lib/amlogic/include/soc/aml-a311d/a311d-gpio.h"
#include "sdk/lib/driver/devicetree/visitors/drivers/interrupt-controllers/arm-gic/arm-gicv2.h"
#include "vim3-iommu.h"

/ {
	interrupt-parent = <&gic>;

	soc {
		#address-cells = <2>;
		#size-cells = <2>;
		ranges;

		dwmac@ff3f0000 {
			compatible = "amlogic,meson-g12a-dwmac";
			reg = <0x0 0xff3f0000 0x0 0x10000>;
			interrupts = <GIC_SPI 8 GIC_IRQ_MODE_LEVEL_HIGH>;
			phys = <&ethmac>;
			iommus = <&stub_iommu DWMAC_BTI>;
			boot-metadata = <0x43414D6D 0>;

			/*ZBI_TYPE_DRV_MAC_ADDRESS*/
		};

		video-decoder@ffd00000 {
			compatible = "amlogic,g12b-vdec";
			reg = <0x0 A311D_FULL_CBUS_BASE 0x0 A311D_FULL_CBUS_LENGTH>, <0x0 A311D_DOS_BASE 0x0 A311D_DOS_LENGTH>, <0x0 A311D_HIU_BASE 0x0 A311D_HIU_LENGTH>, <0x0 A311D_AOBUS_BASE 0x0 0x1000>, <0x0 A311D_DMC_BASE 0x0 A311D_DMC_LENGTH>;
			iommus = <&stub_iommu VDEC_BTI>;
			interrupts = <GIC_SPI 23 GIC_IRQ_MODE_EDGE_RISING>, <GIC_SPI 32 GIC_IRQ_MODE_EDGE_RISING>, <GIC_SPI 43 GIC_IRQ_MODE_EDGE_RISING>, <GIC_SPI 44 GIC_IRQ_MODE_EDGE_RISING>;
			amlogic,canvas = <&canvas>;
			clocks = <&clock 0x10005>, <&clock 0x10007>;
			clock-names = "clock-dos-vdec", "clock-dos";
		};

		ethmac: ethernet-phy@ff634000 {
			compatible = "amlogic,ethernet-phy";
			reg = <0x0 0xff634000 0x0 0x1000>, <0x0 0xff63c000 0x0 0x2000>;
			gpios = <&gpio A311D_GPIOZ(14) 0>;
			gpio-names = "gpio-int";
			pinctrl-0 = <&ethernet_phy_cfg>;
			#phy-cells = <0>;
		};

		gpio: gpio-controller@ff634400 {
			compatible = "amlogic,meson-g12a-gpio";
			reg = <0x0 A311D_GPIO_BASE 0x0 A311D_GPIO_LENGTH>, <0x0 A311D_GPIO_AO_BASE 0x0 A311D_GPIO_AO_LENGTH>, <0x0 A311D_GPIO_INTERRUPT_BASE 0x0 A311D_GPIO_INTERRUPT_LENGTH>;
			interrupts = <GIC_SPI 64 GIC_IRQ_MODE_EDGE_RISING>, <GIC_SPI 65 GIC_IRQ_MODE_EDGE_RISING>, <GIC_SPI 66 GIC_IRQ_MODE_EDGE_RISING>, <GIC_SPI 67 GIC_IRQ_MODE_EDGE_RISING>, <GIC_SPI 68 GIC_IRQ_MODE_EDGE_RISING>, <GIC_SPI 69 GIC_IRQ_MODE_EDGE_RISING>, <GIC_SPI 70 GIC_IRQ_MODE_EDGE_RISING>, <GIC_SPI 71 GIC_IRQ_MODE_EDGE_RISING>;
			interrupt-names = "0", "1", "2", "3", "4", "5", "6", "7";
			fuchsia,interrupt-wake-vectors = "0";
			gpio-controller;
			#gpio-cells = <2>;

			audio_tdm_a_cfg1: audio-tdm-a-cfg1 {
				pins = <A311D_GPIOX(11) A311D_GPIOX(10) A311D_GPIOX(9)>;
				function-id = <0 1>;
				drive-strength-microamp = <0 3000>;
			};

			audio_tdm_a_cfg2: audio-tdm-a-cfg2 {
				pins = <A311D_GPIOX(8)>;
				function-id = <0 2>;
			};

			audio_tdm_b_cfg1: audio-tdm-b-cfg1 {
				pins = <A311D_GPIOA(1) A311D_GPIOA(2) A311D_GPIOA(3)>;
				function-id = <0 1>;
				drive-strength-microamp = <0 3000>;
			};

			audio_tdm_b_cfg2: audio-tdm-b-cfg2 {
				pins = <A311D_GPIOA(4)>;
				function-id = <0 2>;
			};

			usb_power: usb-power {
				pins = <A311D_GPIOA(6)>;
				output-high;
			};

			emmc_d0_d7_cfg: emmc-d0-d7-cfg {
				pins = <A311D_GPIOBOOT(0) A311D_GPIOBOOT(1) A311D_GPIOBOOT(2) A311D_GPIOBOOT(3)>, <A311D_GPIOBOOT(4) A311D_GPIOBOOT(5) A311D_GPIOBOOT(6) A311D_GPIOBOOT(7)>;
				function-id = <0 1>;
			};

			emmc_clk_cfg: emmc-clk-cfg {
				pins = <A311D_GPIOBOOT(8)>;
				function-id = <0 1>;
			};

			emmc_cmd_cfg: emmc-cmd-cfg {
				pins = <A311D_GPIOBOOT(10)>;
				function-id = <0 1>;
			};

			emmc_ds_cfg: emmc-ds-cfg {
				pins = <A311D_GPIOBOOT(13)>;
				function-id = <0 1>;
			};

			emmc_cs_cfg: emmc-cs-cfg {
				pins = <A311D_GPIOBOOT(14)>;
				output-high;
			};

			sdcard_d0_d3_cfg: sdcard-d0-d3-cfg {
				pins = <A311D_GPIOC(0) A311D_GPIOC(1) A311D_GPIOC(2) A311D_GPIOC(3)>;
				function-id = <0 1>;
			};

			sdcard_clk_cfg: sdcard-clk-cfg {
				pins = <A311D_GPIOC(4)>;
				function-id = <0 1>;
			};

			sdcard_cmd_cfg: sdcard-cmd-cfg {
				pins = <A311D_GPIOC(5)>;
				function-id = <0 1>;
			};

			sdio_d0_d3_cfg: sdio-d0-d3-cfg {
				pins = <A311D_GPIOX(0) A311D_GPIOX(1) A311D_GPIOX(2) A311D_GPIOX(3)>;
				bias-disable;
				function-id = <0 1>;
				drive-strength-microamp = <0 4000>;
			};

			sdio_clk_cfg: sdio-clk-cfg {
				pins = <A311D_GPIOX(4)>;
				bias-disable;
				function-id = <0 1>;
				drive-strength-microamp = <0 4000>;
			};

			sdio_cmd_cfg: sdio-cmd-cfg {
				pins = <A311D_GPIOX(5)>;
				bias-disable;
				function-id = <0 1>;
				drive-strength-microamp = <0 4000>;
			};

			ethernet_phy_cfg: ethernet-phy-cfg {
				pins = <A311D_GPIOZ(0) A311D_GPIOZ(1) A311D_GPIOZ(2) A311D_GPIOZ(3)>, <A311D_GPIOZ(4) A311D_GPIOZ(5) A311D_GPIOZ(6) A311D_GPIOZ(7)>, <A311D_GPIOZ(8) A311D_GPIOZ(9) A311D_GPIOZ(10) A311D_GPIOZ(11)>, <A311D_GPIOZ(12) A311D_GPIOZ(13)>;
				function-id = <0 1>;
				drive-strength-microamp = <0 2500>;
			};

			wifi_lpo_pwm: wifi-lpo-pwm {
				pins = <A311D_GPIOX(16)>;
				function-id = <0 1>;
			};
		};

		cpu_temp: temperature-sensor@ff634800 {
			compatible = "amlogic,g12a-thermal";
			reg = <0x0 A311D_TEMP_SENSOR_PLL_BASE 0x0 A311D_TEMP_SENSOR_PLL_LENGTH>, <0x0 A311D_TEMP_SENSOR_PLL_TRIM 0x0 A311D_TEMP_SENSOR_TRIM_LENGTH>;
			interrupts = <GIC_SPI 35 GIC_IRQ_MODE_EDGE_RISING>;
			assigned-clocks = <&clock 0x1000d>;
			#thermal-sensor-cells = <0>;
		};

		ddr_temp: temperature-sensor@ff634c00 {
			compatible = "amlogic,g12a-thermal";
			reg = <0x0 A311D_TEMP_SENSOR_DDR_BASE 0x0 A311D_TEMP_SENSOR_DDR_LENGTH>, <0x0 A311D_TEMP_SENSOR_DDR_TRIM 0x0 A311D_TEMP_SENSOR_TRIM_LENGTH>;
			interrupts = <GIC_SPI 36 GIC_IRQ_MODE_EDGE_RISING>;
			assigned-clocks = <&clock 0x1000d>;
			#thermal-sensor-cells = <0>;
		};

		audio: audio-controller@ff642000 {
			compatible = "amlogic,audio-controller";
			reg = <0x0 0xff642000 0x0 0x2000>;
			iommus = <&stub_iommu AUDIO_BTI>;
			clocks = <&clock 0x1000b &clock 0x20002>;
			clock-names = "clock-gate", "clock-pll";
			assigned-clocks = <&clock 0x20002>;
			assigned-clock-rates = <768000000>;
			gpios = <&gpio A311D_GPIOX(11) 0 &gpio A311D_GPIOA(1) 0>;
			gpio-names = "gpio-tdm-a-sclk", "gpio-tdm-b-sclk";
			pinctrl-0 = <&audio_tdm_a_cfg1 &audio_tdm_a_cfg2>, <&audio_tdm_b_cfg1 &audio_tdm_b_cfg2>;
		};

		aobus: bus@ff800000 {
			reg = <0x0 0xff800000 0x0 0x100000>;
			#address-cells = <2>;
			#size-cells = <2>;
			ranges = <0x0 0x0 0x0 0xff800000 0x0 0x100000>;

			i2c_AO: i2c@5000 {
				compatible = "amlogic,meson-axg-i2c";
				reg = <0x0 0x05000 0x0 0x20>;
				interrupts = <GIC_SPI 195 GIC_IRQ_MODE_EDGE_RISING>;
				#address-cells = <1>;
				#size-cells = <0>;
			};

			saradc: adc@9000 {
				compatible = "amlogic,meson-g12a-saradc";
				reg = <0x0 0x9000 0x0 0x1000>, <0x0 0x0 0x0 0x1000>;
				interrupts = <GIC_SPI 200 GIC_IRQ_MODE_EDGE_RISING>;
				#io-channel-cells = <1>;
			};
		};

		gic: interrupt-controller@ffc01000 {
			compatible = "arm,gic-400";
			reg = <0x0 0xffc01000 0 0x1000>, <0x0 0xffc02000 0 0x2000>, <0x0 0xffc04000 0 0x2000>, <0x0 0xffc06000 0 0x2000>;
			interrupt-controller;
			interrupts = <GIC_PPI 9 ( GIC_CPU_MASK_SIMPLE(8) | GIC_IRQ_MODE_LEVEL_HIGH )>;
			#interrupt-cells = <3>;
			#address-cells = <0>;
		};

		cbus: bus@ffd00000 {
			reg = <0x0 0xFFD00000 0x0 0x100000>;
			#address-cells = <2>;
			#size-cells = <2>;
			ranges = <0x0 0x0 0x0 0xFFD00000 0x0 0x100000>;

			hrtimer: hrtimer@0 {
				compatible = "amlogic,hrtimer";
				reg = <0x0 0x0 0x0 A311D_TIMER_LENGTH>;

				/*
				 * Interrupts:
				 * - Timer A: GIC_SPI 10 GIC_IRQ_MODE_EDGE_RISING
				 * - Timer B: GIC_SPI 11 GIC_IRQ_MODE_EDGE_RISING
				 * - Timer C: GIC_SPI 6 GIC_IRQ_MODE_EDGE_RISING
				 * - Timer D: GIC_SPI 29 GIC_IRQ_MODE_EDGE_RISING
				 * - Timer F: GIC_SPI 60 GIC_IRQ_MODE_EDGE_RISING (Timer E has no IRQ)
				 * - Timer G: GIC_SPI 61 GIC_IRQ_MODE_EDGE_RISING
				 * - Timer H: GIC_SPI 62 GIC_IRQ_MODE_EDGE_RISING
				 * - Timer I: GIC_SPI 63 GIC_IRQ_MODE_EDGE_RISING
				 */
				interrupts = <GIC_SPI 10 GIC_IRQ_MODE_EDGE_RISING>, <GIC_SPI 11 GIC_IRQ_MODE_EDGE_RISING>, <GIC_SPI 6 GIC_IRQ_MODE_EDGE_RISING>, <GIC_SPI 29 GIC_IRQ_MODE_EDGE_RISING>, <GIC_SPI 60 GIC_IRQ_MODE_EDGE_RISING>, <GIC_SPI 61 GIC_IRQ_MODE_EDGE_RISING>, <GIC_SPI 62 GIC_IRQ_MODE_EDGE_RISING>, <GIC_SPI 63 GIC_IRQ_MODE_EDGE_RISING>;
				interrupt-names = "timer-a", "timer-b", "timer-c", "timer-d", "timer-f", "timer-g", "timer-h", "timer-i";
				fuchsia,interrupt-wake-vectors = "timer-a", "timer-b", "timer-c", "timer-d", "timer-f", "timer-g", "timer-h", "timer-i";
			};

			reset: register-controller@1000 {
				compatible = "fuchsia,registers";
				reg = <0x0 0x1000 0x0 0x100>;
				#register-cells = <4>;
			};

			mipi_dsi: dsi@7000 {
				compatible = "dw,dsi";
				reg = <0x0 0x7000 0x0 0x1000>;
			};

			i2c3: i2c@1c000 {
				compatible = "amlogic,meson-axg-i2c";
				reg = <0x0 0x1c000 0x0 0x20>;
				interrupts = <GIC_SPI 39 GIC_IRQ_MODE_EDGE_RISING>;
				#address-cells = <1>;
				#size-cells = <0>;
			};
		};

		/*
		 * Registers:
		 * - PWM_AB
		 * - PWM_CD
		 * - PWM_EF
		 * - PWM_A0_AB
		 * - PWM_A0_CD
		 */
		pwm: pwm@ffd1b000 {
			compatible = "amlogic,meson-g12a-ee-pwm";
			reg = <0x0 0xffd1b000 0x0 0x1000>, <0x0 0xffd1a000 0x0 0x1000>, <0x0 0xffd19000 0x0 0x1000>, <0x0 0xff807000 0x0 0x1000>, <0x0 0xff802000 0x0 0x1000>;
			#pwm-cells = <3>;
		};

		/* pwms = <&pwm 4 0 0> : A311D_PWM_E */
		pwm_init: pwm-init {
			pwms = <&pwm 4 0 0>;
			gpios = <&gpio A311D_GPIOX(17) 0>;
			gpio-names = "gpio-bt";
			pinctrl-0 = <&wifi_lpo_pwm>;
		};

		usb_phy: usb-phy@ffe09000 {
			compatible = "amlogic,g12b-usb-phy";
			reg = <0x0 A311D_USBCTRL_BASE 0x0 A311D_USBCTRL_LENGTH>, <0x0 A311D_USBPHY20_BASE 0x0 A311D_USBPHY20_LENGTH>, <0x0 A311D_USBPHY21_BASE 0x0 A311D_USBPHY21_LENGTH>, <0x0 A311D_USB3_PCIE_PHY_BASE 0x0 A311D_USB3_PCIE_PHY_LENGTH>;
			reg-names = "usb-ctrl", "usb2-phy", "usb2-otg-phy", "usb3-phy";
			dr_modes = "host", "peripheral", "host";
			interrupts = <GIC_SPI 16 GIC_IRQ_MODE_EDGE_RISING>;
			iommus = <&stub_iommu USB_PHY_BTI>;
			assigned-clocks = <&clock 0x10008 &clock 0x10009 &clock 0x20001>;
			assigned-clock-rates = <0 0 100000000>;
			pinctrl-0 = <&usb_power>;
			registers = <&reset A311D_RESET1_REGISTER 4 0x0 0x10004>, <&reset A311D_RESET1_LEVEL 4 0x0 0x30000>;
			#phy-cells = <0>;
		};

		dwc2: usb@ff400000 {
			compatible = "snps,dwc2";
			reg = <0x0 A311D_USB1_BASE 0x0 A311D_USB1_LENGTH>;
			interrupts = <GIC_SPI 31 GIC_IRQ_MODE_EDGE_RISING>;
			interrupt-names = "primary";
			fuchsia,interrupt-wake-vectors = "primary";
			iommus = <&stub_iommu DWC2_BTI>;
			phys = <&usb_phy>;
			phy-names = "dwc2-phy";
			g-rx-fifo-size = <256>;
			g-np-tx-fifo-size = <32>;
			g-tx-fifo-size = <128 4 128 128>;
			g-turnaround-time = <9>;
			dma-burst-len = <5>;

			/*
			 * Boot Metadata:
			 * - 0x43414D6D 0: ZBI_TYPE_DRV_MAC_ADDRESS
			 * - 0x4e4c5253 0: ZBI_TYPE_SERIAL_NUMBER
			 */
			boot-metadata = <0x43414D6D 0>, <0x4e4c5253 0>;
		};

		xhci: usb@ff500000 {
			compatible = "generic-xhci";
			reg = <0x0 A311D_USB0_BASE 0x0 A311D_USB0_LENGTH>;
			interrupts = <GIC_SPI 30 GIC_IRQ_MODE_EDGE_RISING>;
			interrupt-names = "primary";
			fuchsia,interrupt-wake-vectors = "primary";
			iommus = <&stub_iommu XHCI_BTI>;
			phys = <&usb_phy>;
			phy-names = "xhci-phy";
		};

		sd_emmc_a: mmc@ffe03000 {
			compatible = "amlogic,meson-axg-mmc";
			reg = <0x0 0xffe03000 0x0 0x2000>;
			interrupts = <GIC_SPI 189 GIC_IRQ_MODE_EDGE_RISING>;
			iommus = <&stub_iommu EMMC_A_BTI>;
			gpios = <&gpio A311D_GPIOX(6) 0>;
			gpio-names = "gpio-reset";
			pinctrl-0 = <&sdio_d0_d3_cfg &sdio_clk_cfg &sdio_cmd_cfg>;
			max-frequency = <100000000>;
			pwm-init = <&pwm_init>;
		};

		sd_emmc_b: mmc@ffe05000 {
			compatible = "amlogic,meson-axg-mmc";
			reg = <0x0 0xffe05000 0x0 0x2000>;
			interrupts = <GIC_SPI 190 GIC_IRQ_MODE_EDGE_RISING>;
			iommus = <&stub_iommu EMMC_B_BTI>;
			pinctrl-0 = <&sdcard_d0_d3_cfg &sdcard_clk_cfg &sdcard_cmd_cfg>;
			max-frequency = <50000000>;
		};

		sd_emmc_c: mmc@ffe07000 {
			compatible = "amlogic,meson-axg-mmc";
			reg = <0x0 0xffe07000 0x0 0x2000>;
			interrupts = <GIC_SPI 191 GIC_IRQ_MODE_EDGE_RISING>;
			iommus = <&stub_iommu EMMC_C_BTI>;
			boot-metadata = <0x5452506D 0>;
			clocks = <&clock 0x1000C>;
			gpios = <&gpio A311D_GPIOBOOT(12) 0>;
			gpio-names = "gpio-reset";
			pinctrl-0 = <&emmc_d0_d7_cfg &emmc_clk_cfg &emmc_cmd_cfg>, <&emmc_ds_cfg &emmc_cs_cfg>;
			max-frequency = <120000000>;
			non-removable;
			no-mmc-hs400;
			use-fidl;
		};

		/*
		 * Registers:
		 * - MALI GPU: 0xffe40000
		 * - HUI:      0xff63c000
		 */
		aml_gpu: gpu@ffe40000 {
			compatible = "amlogic,gpu";
			reg = <0x0 0xffe40000 0x0 0x40000>, <0x0 0xff63c000 0x0 0x2000>;
			registers = <&reset A311D_RESET0_MASK 4 0x0 0x100000>, <&reset A311D_RESET0_LEVEL 4 0x0 0x100000>, <&reset A311D_RESET2_MASK 4 0x0 0x4000>, <&reset A311D_RESET2_LEVEL 4 0x0 0x4000>;
			#address-cells = <2>;
			#size-cells = <2>;
			ranges = <0x0 0x0 0x0 0xffe40000 0x0 0x40000>;

			arm_mali: arm-mali@0 {
				compatible = "arm,mali-gpu";
				reg = <0x0 0x0 0x0 0x40000>;
				interrupts = <GIC_SPI 162 GIC_IRQ_MODE_LEVEL_HIGH>, <GIC_SPI 161 GIC_IRQ_MODE_LEVEL_HIGH>, <GIC_SPI 160 GIC_IRQ_MODE_LEVEL_HIGH>;
				mali-gpu-parent = <&aml_gpu>;
				iommus = <&stub_iommu MALI_BTI>;
			};
		};
	};

	nna@ff100000 {
		compatible = "amlogic,nna";
		reg = <0x0 A311D_NNA_BASE 0x0 A311D_NNA_LENGTH>, <0x0 A311D_HIU_BASE 0x0 A311D_HIU_LENGTH>, <0x0 A311D_POWER_DOMAIN_BASE 0x0 A311D_POWER_DOMAIN_LENGTH>, <0x0 A311D_MEMORY_PD_BASE 0x0 A311D_MEMORY_PD_LENGTH>, <0x0 A311D_NNA_SRAM_BASE 0x0 A311D_NNA_SRAM_LENGTH>;
		interrupts = <GIC_SPI 147 GIC_IRQ_MODE_LEVEL_HIGH>;
		iommus = <&stub_iommu NNA_BTI>;
		registers = <&reset A311D_RESET2_LEVEL 4 0x0 0x1000>;
	};

	thermal-zones {
		cpu_thermal: cpu-thermal {
			thermal-sensors = <&cpu_temp>;

			trips {
				cpu_critical: cpu-critical {
					temperature = <101000>;

					/* millicelsius */
					type = "critical";
				};
			};
		};

		ddr_thermal: ddr-thermal {
			thermal-sensors = <&ddr_temp>;

			trips {
				ddr_critical: ddr-critical {
					temperature = <110000>;

					/* millicelsius */
					type = "critical";
				};
			};
		};
	};
};
