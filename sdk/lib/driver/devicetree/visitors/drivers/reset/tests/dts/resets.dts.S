// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "resets.h"

/dts-v1/;

/ {
	test-board {
		test-soc {
			reset_controller1: first-reset-controller {
				compatible = "reset-controller1";
				#reset-cells = <0>;
			};

			reset_controller2: second-reset-controller {
				compatible = "reset-controller2";
				#reset-cells = <1>;
			};

			test_device: test-device {
				compatible = "a-cool-reset-device";
				resets = <&reset_controller1>, <&reset_controller2 RESET_ID_1>, <&reset_controller2 RESET_ID_2>;
				reset-names = RESET_NAME_0, RESET_NAME_1, RESET_NAME_2;
			};
		};
	};
};
