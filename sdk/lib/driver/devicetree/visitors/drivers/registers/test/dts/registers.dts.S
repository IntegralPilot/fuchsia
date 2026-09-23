// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file
#include "registers.h"

/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	reset: register-controller@ffffa000 {
		compatible = "fuchsia,registers";
		reg = <0x0 0xffffa000 0x0 0x1000>;
		#register-cells = <4>;
		overlap_check_on;
	};

	reset2: register-controller@ffffb000 {
		compatible = "fuchsia,registers";
		reg = <0x0 0xffffb000 0x0 0x2000>;
		#register-cells = <4>;
	};

	usb@0 {
		reg = <0x0 0x0 0x0 0x100>;
		compatible = "usb-peripheral";
		registers = <&reset REGISTER_OFFSET1 REGISTER_LENGTH1 0x0 REGISTER_MASK1>, <&reset REGISTER_OFFSET2 REGISTER_LENGTH2 REGISTER_MASK2_1 REGISTER_MASK2_0>;
	};

	usb@100 {
		reg = <0x0 0100 0x0 0x100>;
		compatible = "usb-peripheral-b";
		registers = <&reset REGISTER_OFFSET3 REGISTER_LENGTH3 0x0 REGISTER_MASK3>, <&reset2 REGISTER_OFFSET4 REGISTER_LENGTH4 0x0 REGISTER_MASK4>;
		register-names = REGISTER_NAME3, REGISTER_NAME4;
	};
};
