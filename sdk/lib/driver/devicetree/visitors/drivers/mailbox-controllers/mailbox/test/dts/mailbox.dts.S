// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file
/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <1>;
	#size-cells = <1>;

	mailbox_1: mailbox@abcd0000 {
		compatible = "vendor1,mailbox";
		reg = <0xabcd0000 0x1000>;
		#mbox-cells = <1>;
	};

	mailbox_2: mailbox@abce0000 {
		compatible = "vendor2,mailbox";
		reg = <0xabce0000 0x1000>;
		#mbox-cells = <1>;
	};

	node@abcf0000 {
		reg = <0xabcf0000 0x1000>;
		mboxes = <&mailbox_1 0x1234>, <&mailbox_1 0x5678>, <&mailbox_2 0x9abc>;
		mbox-names = "mailbox-1-1234", "mailbox-1-5678", "mailbox-2-9abc";
	};

	node@abd00000 {
		reg = <0xabd00000 0x1000>;
		mboxes = <&mailbox_2 0x1234>;
	};

	mailbox_3: mailbox@abcf1000 {
		compatible = "vendor3,mailbox";
		reg = <0xabcf1000 0x1000>;
		#mbox-cells = <2>;
	};

	node@abd10000 {
		reg = <0xabd10000 0x1000>;
		mboxes = <&mailbox_3 0x1111 0x2222>;
		mbox-names = "mailbox-3-1111-2222";
	};
};
