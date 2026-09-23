// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file
#include "i2c.h"

/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	i2c@ffffa000 {
		compatible = "fuchsia,i2c";
		reg = <0x0 0xffffa000 0x0 0x1000>;
		#address-cells = <1>;
		#size-cells = <0>;

		child@c {
			compatible = "fuchsia,test";
			reg = <I2C_ADDRESS1>;
		};

		child-dup@c {
			compatible = "fuchsia,test";
			reg = <I2C_ADDRESS1>;
		};

		child@1e {
			compatible = "fuchsia,test";
			reg = <I2C_ADDRESS2>;
		};

		child@2b {
			compatible = "fuchsia,test";
			reg = <I2C_ADDRESS3 I2C_ADDRESS4>;
		};

		child-names@30 {
			compatible = "fuchsia,test";
			reg = <I2C_ADDRESS5 I2C_ADDRESS6>;
			reg-names = "control", "data";
		};
	};
};
