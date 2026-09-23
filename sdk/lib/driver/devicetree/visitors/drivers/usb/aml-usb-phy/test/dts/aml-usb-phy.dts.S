// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	usb_phy: phy@ffe00000 {
		compatible = "amlogic,g12b-usb-phy";
		reg = <0x0 0xFFE00000 0x0 0x2000>, <0x0 0xFFF00000 0x0 0x1000>, <0x0 0xFFF10000 0x0 0x1000>, <0x0 0xFFF20000 0x0 0x1000>;
		reg-names = "usb-ctrl", "usb2-phy", "usb2-otg-phy", "usb3-phy";
		dr_modes = "host", "peripheral", "host";
		#phy-cells = <0>;
	};
};
