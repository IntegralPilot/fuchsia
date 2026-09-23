// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
/dts-v1/;

/ {
	test-board {
		test-soc {
			rtc: first-rtc {
				compatible = "some-rtc";
			};

			test_device: test-device {
				compatible = "i-need-to-read-time";
				rtcs = <&rtc>;
			};
		};
	};
};
