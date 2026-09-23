// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "thermal-zones-test.h"

/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	ddr_sensor: ddr-sensor {
		compatible = "test,temperature-sensor";
		#thermal-sensor-cells = <0>;
	};

	thermal-zones {
		ddr-thermal {
			thermal-sensors = <&ddr_sensor>;

			trips {
				ddr_critical {
					temperature = <CRITICAL_TEMP>;
					type = "critical";
				};
			};
		};
	};
};
