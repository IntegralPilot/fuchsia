// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file
#include "gpio.h"

/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	gpioA: gpio-controller@ffffa000 {
		compatible = "fuchsia,gpio";
		reg = <0x0 0xffffa000 0x0 0x1000>;
		#gpio-cells = <2>;
		gpio-controller;

		init-hog-HOG_PIN1 {
			gpio-hog;
			gpios = <HOG_PIN1 0>;
			output-low;
		};

		init-hog-HOG_PIN2 {
			gpio-hog;
			gpios = <HOG_PIN2 HOG_PIN2_FLAG HOG_PIN3 HOG_PIN3_FLAG>;
			input;
		};

		group1: group1 {
			pins = <GROUP1_PIN1 GROUP1_PIN2>;
			function-id = <0x0 GROUP1_FUNCTION>;
			drive-strength-microamp = <0x0 GROUP1_DRIVE_STRENGTH>;
			drive-open-drain;
		};

		group2: group2 {
			pins = <GROUP2_PIN1 GROUP2_PIN2>;
			output-low;
			power-source = <GROUP2_POWER_SOURCE>;
		};

		group3: group3 {
			pins = <GROUP3_PIN1>;
			output-disable;
			bias-disable;
			drive-open-source;
			fuchsia,interrupt-wake-vector;
		};

		group6: group6 {
			pins = <GROUP6_PIN1>;
			drive-strength = <GROUP6_DRIVE_STRENGTH>;
		};
	};

	gpio-controller@ffffb000 {
		compatible = "fuchsia,gpio";
		reg = <0x0 0xffffb000 0x0 0x1000>;
		#gpio-cells = <2>;
		gpio-controller;

		group4: group4 {
			pins = <GROUP4_PIN1>;
			bias-pull-up;
			drive-push-pull;
		};

		group5: group5 {
			pins = <GROUP5_PIN1>;
			function = "spi-bus";
		};
	};

	audio@ffffc000 {
		compatible = "test,audio";
		reg = <0x0 0xffffc000 0x0 0x1000>;
		gpios = <&gpioA PIN1 0 &gpioA PIN2 0>;
		gpio-names = PIN1_NAME, PIN2_NAME;
		pinctrl-0 = <&group1>;
		pinctrl-1 = <&group3>;
		pinctrl-names = "default", "sleep";
	};

	video@ffffd000 {
		compatible = "test,video";
		reg = <0x0 0xffffd000 0x0 0x1000>;
		pinctrl-0 = <&group2 &group4 &group5 &group6>;
	};
};
