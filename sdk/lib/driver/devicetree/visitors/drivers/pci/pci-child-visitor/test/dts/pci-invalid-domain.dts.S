// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file
/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	pci@72000000 {
		compatible = "pci-host-ecam-generic";
		device_type = "pci";
		fuchsia,pci-domain = <0x10000>;
		#address-cells = <3>;
		#size-cells = <2>;
		#interrupt-cells = <1>;
		bus-range = <0x00 0x00>;
		reg = <0x0 0x72000000 0x0 0x1000000>;
		ranges = <0x3000000 0x0 0x70000000 0x0 0x70000000 0x0 0x2000000>;

		device@1,0 {
			compatible = "fuchsia,test-pci-1";
			reg = <0x0800 0 0 0 0>;
		};
	};
};
