// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file
/dts-v1/;

#include "sdk/lib/driver/devicetree/manager/test-data/basic-properties.h"
#include <lib/ddk/platform-defs.h>

/ {
	compatible = "fuchsia,sample-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	test-properties-device {
		compatible = "fuchsia,test-properties-device";
		bool-property;
		string-property = "hello";
		device_specific_prop = <DEVICE_SPECIFIC_PROP_VALUE>;
		uint64-property = <0x12345678 0x9abcdef0>;
		uint32-vector-property = <1 2>;
		string-list-property = "string1", "string2";
	};

	sample-device@0 {
		compatible = "fuchsia,sample-device";
		reg = <0 TEST_REG_A_BASE 0 TEST_REG_A_LENGTH>;
		device_specific_prop = <DEVICE_SPECIFIC_PROP_VALUE>;
	};

	sample_parent1: reference-parent-1 {
		compatible = "fuchsia,reference-parent";
		#property1-cells = <PROPERTY1_CELLS>;
		#property2-cells = <PROPERTY2_CELLS>;
	};

	sample_parent2: reference-parent-2 {
		compatible = "fuchsia,reference-parent";
		#property2-cells = <PROPERTY2_CELLS>;
	};

	sample_parent3: reference-parent-3 {
		compatible = "fuchsia,reference-parent";
		#property2-cells = <PROPERTY2_CELLS>;
	};

	sample-property1-child {
		compatible = "fuchsia,property1-child";
		property1 = <&sample_parent1 PROPERTY1_SPECIFIER>;
	};

	property2-child {
		compatible = "fuchsia,property2-child";
		property2 = <&sample_parent1 &sample_parent2 &sample_parent3>;
		property2-names = PROPERTY2_NAME1, PROPERTY2_NAME2, PROPERTY2_NAME3;
	};

	parent-device {
		compatible = "fuchsia,parent";

		child1 {
			compatible = "fuchsia,child";
		};

		child2 {
			compatible = "fuchsia,child";
		};
	};
};
