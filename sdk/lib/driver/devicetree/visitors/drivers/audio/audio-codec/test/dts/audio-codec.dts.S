// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	codec_1: audio-codec@48 {
		compatible = "test-codec";
		reg = <0x0 0x48 0x0 0x10>;
	};

	codec_2: audio-codec@49 {
		compatible = "test-codec";
		reg = <0x0 0x49 0x0 0x10>;
	};

	audio@ff642000 {
		compatible = "test-audio-controller";
		reg = <0x0 0xff642000 0x0 0x2000>;
		codecs = <&codec_1 &codec_2>;
	};
};
