// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "pwm.h"

/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	pwmA: pwm@ffffa000 {
		compatible = "test,pwm";
		reg = <0x0 0xffffa000 0x0 0x1000>;
		#pwm-cells = <3>;
	};

	unused_pwm: pwm@ffffb000 {
		compatible = "test,pwm";
		reg = <0x0 0xffffb000 0x0 0x1000>;
		#pwm-cells = <2>;
		phandle = <0x10>;
	};

	audio@ffffc000 {
		compatible = "test,audio";
		reg = <0x0 0xffffc000 0x0 0x1000>;
		pwms = <&pwmA PIN1 PIN1_PERIOD PIN1_FLAG &pwmA PIN2 PIN2_PERIOD PIN2_FLAG>;
		pwm-names = PIN1_NAME, PIN2_NAME;
	};
};
