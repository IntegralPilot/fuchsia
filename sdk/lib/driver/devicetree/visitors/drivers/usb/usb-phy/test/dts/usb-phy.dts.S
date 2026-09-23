// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	usb_phy_1: usb-phy@fe00000 {
		compatible = "test-phy-1";
		reg = <0x0 0xfe00000 0x0 0x2000>;
		#phy-cells = <0>;
	};

	usb_phy: usb-phy@ff00000 {
		compatible = "test-phy";
		reg = <0x0 0xff00000 0x0 0x2000>;
		#phy-cells = <0>;
	};

	test@ff400000 {
		compatible = "test-controller";
		reg = <0x0 0xff400000 0x0 0x2000>;
		phys = <&usb_phy>, <&usb_phy_1>;
		phy-names = "xhci-phy", "another-phy";
	};
};
