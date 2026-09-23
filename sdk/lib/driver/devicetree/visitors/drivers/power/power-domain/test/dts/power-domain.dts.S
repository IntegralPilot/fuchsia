// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "power-domain-test.h"

/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	pwrc: power-controller {
		compatible = "test,power";
		#power-domain-cells = <1>;
		fuchsia,legacy-power;
	};

	cpufreq: cpufreq {
		compatible = "test,cpufreq";
		power-domains = <&pwrc TEST_DOMAIN_ID>;
		power-domain-names = "ice";
	};

	pwrc_basic: basic-pwrc {
		compatible = "test,power-basic";
		#power-domain-cells = <1>;
	};

	device_basic: device-basic-a {
		compatible = "test,device-basic";
		power-domains = <&pwrc_basic 2>;
	};

	device_basic_2: device-basic-b {
		compatible = "test,device-basic-2";
		power-domains = <&pwrc_basic 2>;
		power-domain-names = "basic_power_2";
	};
};
