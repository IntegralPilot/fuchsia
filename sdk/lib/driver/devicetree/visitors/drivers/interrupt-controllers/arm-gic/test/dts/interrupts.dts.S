// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file
/dts-v1/;

#include "interrupts.h"

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;
	interrupt-parent = <&gic>;

	gic: interrupt-controller@ffffd0000 {
		compatible = "arm,gic-400";
		#interrupt-cells = <3>;
		interrupt-controller;
		reg = <0x0 0xFFFFD000 0x0 0x1000>;
	};

	gicv3: interrupt-controller@ffffe0000 {
		compatible = "arm,gic-v3";
		#interrupt-cells = <4>;
		interrupt-controller;
		reg = <0x0 0xFFFFE000 0x0 0x1000>;
	};

	sample-device@1 {
		compatible = "fuchsia,sample-device";
		interrupts = <0 IRQ1_SPI IRQ1_MODE 1 IRQ2_PPI IRQ2_MODE>;
		interrupt-names = "interrupt-first", "interrupt-second";
		reg = <0x0 0x1 0x0 0x1>;
	};

	sample-device@2 {
		compatible = "fuchsia,sample-device";
		interrupts-extended = <&gic 0 IRQ3_SPI IRQ3_MODE &gic 1 IRQ4_PPI IRQ4_MODE>;
		interrupt-names = "interrupt-first", "interrupt-second";
		reg = <0x0 0x2 0x0 0x1>;
	};

	sample-device@3 {
		compatible = "fuchsia,sample-device";
		interrupts-extended = <&gicv3 0 IRQ5_SPI IRQ5_MODE 0 &gicv3 0 IRQ6_SPI IRQ6_MODE 0>;
		reg = <0x0 0x3 0x0 0x1>;
	};

	wake-vectors-without-names {
		compatible = "wakeup-source-device";
		interrupts = <0 IRQ1_SPI IRQ1_MODE 1 IRQ2_PPI IRQ2_MODE>;
		fuchsia,interrupt-wake-vectors = "irq1";
	};

	wake-vectors {
		compatible = "wakeup-source-device";
		interrupts = <0 IRQ1_SPI IRQ1_MODE 1 IRQ2_PPI IRQ2_MODE>;
		interrupt-names = "irq1", "irq2";
		fuchsia,interrupt-wake-vectors = "irq2";
	};

	sample-device@4 {
		compatible = "fuchsia,sample-device";

		/* This node is both an interrupt controller and has interrupts. */
		#interrupt-cells = <2>;
		interrupt-controller;
		reg = <0x0 0x4 0x0 0x1>;
		interrupts = <0 IRQ7_SPI IRQ7_MODE 0 IRQ8_SPI IRQ8_MODE>;
	};
};
