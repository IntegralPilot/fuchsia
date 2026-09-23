// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file
/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	/* No fuchsia,pci-domain: everything below is in domain 0. */
	pci@72000000 {
		compatible = "pci-host-ecam-generic";
		device_type = "pci";
		#address-cells = <3>;
		#size-cells = <2>;
		#interrupt-cells = <1>;
		bus-range = <0x00 0x00>;
		reg = <0x0 0x72000000 0x0 0x1000000>;
		ranges = <0x3000000 0x0 0x70000000 0x0 0x70000000 0x0 0x2000000>;

		/* PCI device 00:01.0 -> PCI_TOPO 0x08. Binds by compatible only. */
		device@1,0 {
			compatible = "fuchsia,test-pci-1";
			reg = <0x0800 0 0 0 0>;
		};

		/* PCI device 00:02.0 -> PCI_TOPO 0x10. Adds a <VID DID> override so it
		   can also bind a driver by PCI vendor/device id. */
		device@2,0 {
			compatible = "fuchsia,test-pci-2";
			reg = <0x1000 0 0 0 0>;
			pci-id = <0x8086 0x15f2>;
		};

		/* No reg property: should be ignored by the visitor. */
		fuchsia,config {
			pci_property = <0>;
		};
	};

	/* A second host bridge, in its own PCI domain. */
	pci@92000000 {
		compatible = "pci-host-ecam-generic";
		device_type = "pci";
		fuchsia,pci-domain = <0x1>;
		#address-cells = <3>;
		#size-cells = <2>;
		#interrupt-cells = <1>;
		bus-range = <0x00 0x01>;
		reg = <0x0 0x92000000 0x0 0x1000000>;
		ranges = <0x3000000 0x0 0x90000000 0x0 0x90000000 0x0 0x2000000>;

		/* PCI device 0001:00:03.0 -> PCI_TOPO 0x18. */
		device@3,0 {
			compatible = "fuchsia,test-pci-3";
			reg = <0x1800 0 0 0 0>;
		};

		/* A PCI-to-PCI bridge at 0001:00:04.0 -> PCI_TOPO 0x20. It carries
		   device_type = "pci" too, so the visitor treats it as a bus and its
		   children inherit the host bridge's domain. */
		pci-bridge@4,0 {
			compatible = "pci-bridge";
			device_type = "pci";
			#address-cells = <3>;
			#size-cells = <2>;
			reg = <0x2000 0 0 0 0>;

			/* PCI device 0001:01:00.0 -> PCI_TOPO 0x100. */
			device@0,0 {
				compatible = "fuchsia,test-pci-4";
				reg = <0x10000 0 0 0 0>;
			};
		};
	};
};
