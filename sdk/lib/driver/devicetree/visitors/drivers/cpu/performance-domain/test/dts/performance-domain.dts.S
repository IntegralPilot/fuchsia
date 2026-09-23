// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "performance-domain-test.h"

/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	cpus {
		#address-cells = <0x02>;
		#size-cells = <0x00>;

		cpu0: cpu@0 {
			compatible = "arm,cortex-a53\0arm,armv8";
			device_type = "cpu";
			reg = <0x00 0x00>;
		};

		cpu1: cpu@1 {
			compatible = "arm,cortex-a53\0arm,armv8";
			device_type = "cpu";
			reg = <0x00 0x01>;
		};

		cpu100: cpu@100 {
			compatible = "arm,cortex-a73\0arm,armv8";
			device_type = "cpu";
			reg = <0x00 0x100>;
		};

		cpu101: cpu@101 {
			compatible = "arm,cortex-a73\0arm,armv8";
			device_type = "cpu";
			reg = <0x00 0x101>;
		};

		cpu102: cpu@102 {
			compatible = "arm,cortex-a73\0arm,armv8";
			device_type = "cpu";
			reg = <0x00 0x102>;
		};

		cpu103: cpu@103 {
			compatible = "arm,cortex-a73\0arm,armv8";
			device_type = "cpu";
			reg = <0x00 0x103>;
		};
	};

	cpu_opp_table0: opp-table0 {
		opp-1 {
			opp-hz = <0 TEST_DOMAIN_1_OPP_1_HZ>;
			opp-microvolt = <TEST_DOMAIN_1_OPP_1_UV>;
		};

		opp-2 {
			opp-hz = <0 TEST_DOMAIN_1_OPP_2_HZ>;
			opp-microvolt = <TEST_DOMAIN_1_OPP_2_UV>;
		};

		opp-3 {
			opp-hz = <0 TEST_DOMAIN_1_OPP_3_HZ>;
			opp-microvolt = <TEST_DOMAIN_1_OPP_3_UV>;
		};

		opp-4 {
			opp-hz = <0 TEST_DOMAIN_1_OPP_4_HZ>;
			opp-microvolt = <TEST_DOMAIN_1_OPP_4_UV>;
		};
	};

	cpu_opp_table1: opp-table-1 {
		opp-1 {
			opp-hz = <0 TEST_DOMAIN_2_OPP_1_HZ>;
			opp-microvolt = <TEST_DOMAIN_2_OPP_1_UV>;
		};

		opp-2 {
			opp-hz = <0 TEST_DOMAIN_2_OPP_2_HZ>;
			opp-microvolt = <TEST_DOMAIN_2_OPP_2_UV>;
		};

		opp-3 {
			opp-hz = <0 TEST_DOMAIN_2_OPP_3_HZ>;
			opp-microvolt = <TEST_DOMAIN_2_OPP_3_UV>;
		};

		opp-4 {
			opp-hz = <0 TEST_DOMAIN_2_OPP_4_HZ>;
			opp-microvolt = <TEST_DOMAIN_2_OPP_4_UV>;
		};
	};

	cpu-controller {
		compatible = "board-x,cpu";

		performance-domains {
			arm-a73-domain {
				cpus = <&cpu100 &cpu101 &cpu102 &cpu103>;
				domain-id = <TEST_DOMAIN_1>;
				operating-points = <&cpu_opp_table0>;
				relative-performance = <TEST_DOMAIN_1_PERFORMANCE>;
			};

			arm-a53-domain {
				cpus = <&cpu0 &cpu1>;
				domain-id = <TEST_DOMAIN_2>;
				operating-points = <&cpu_opp_table1>;
				relative-performance = <TEST_DOMAIN_2_PERFORMANCE>;
			};
		};
	};
};
