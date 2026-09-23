// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file
/dts-v1/;

#include "reg.h"

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	test-i2c {
		compatible = "fuchsia,test-i2c";
		#address-cells = <2>;
		#size-cells = <0>;

		test-i2c-child-1@I2C_CHILD_UNIT {
			compatible = "fuchsia,test-i2c-child";
			reg = <0x0 I2C_CHILD_ADDRESS>;
		};
	};
};
