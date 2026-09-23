// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package virtual_device

import (
	"go.fuchsia.dev/fuchsia/tools/lib/productbundle"
	"go.fuchsia.dev/fuchsia/tools/qemu"
	fvdpb "go.fuchsia.dev/fuchsia/tools/virtual_device/proto"
)

// AEMUCommand sets options to run Fuchsia in AEMU on the given AEMUCommandBuilder.
//
// This returns an error if `Validate(fvd, pb)` returns an error.
func AEMUCommand(b *qemu.AEMUCommandBuilder, fvd *fvdpb.VirtualDevice, pb *productbundle.ProductBundle, overrides ImageOverrides) error {
	if err := QEMUCommand(&b.QEMUCommandBuilder, fvd, pb, overrides); err != nil {
		return err
	}
	if fvd.Hw.EnableKvm {
		b.SetFeature("KVM")
	}
	return nil
}
