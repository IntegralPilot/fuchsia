// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package testsharder

import (
	"fmt"

	"go.fuchsia.dev/fuchsia/tools/build"
)

// emuTypeToPackage maps emulator device types to the associated CIPD package and subdir
// to download the package to with respect to the current working directory of the shard.
var emuTypeToPackage = map[string]CIPDPackage{
	"AEMU":                           {Name: "fuchsia/third_party/android/aemu/release-gfxstream/${platform}", Subdir: "aemu/bin"},
	"QEMU":                           {Name: "fuchsia/third_party/qemu/${platform}", Subdir: "qemu"},
	"crosvm":                         {Name: "fuchsia/third_party/crosvm/${platform}", Subdir: "crosvm/bin"},
	"EDK2":                           {Name: "fuchsia/third_party/edk2", Subdir: "firmware/edk2"},
	"arm-trusted-firmware-qemu-bios": {Name: "fuchsia/firmware/arm-trusted-firmware-qemu-bios", Subdir: "firmware/arm-trusted-firmware-qemu-bios"},
}

// AddEmuVersion applies the necessary emulator and edk2 package info required for the shard
// based on the emulator type it targets.
func AddEmuVersion(s *Shard, prebuiltVersions []build.PrebuiltVersion) error {
	if !s.Env.TargetsEmulator() {
		return nil
	}
	var pkgs []CIPDPackage
	if emuPkg, ok := emuTypeToPackage[s.Env.Dimensions.DeviceType()]; ok {
		if version, err := build.GetPackageVersion(prebuiltVersions, emuPkg.Name); err != nil {
			return err
		} else {
			emuPkg.Version = version
		}
		pkgs = append(pkgs, emuPkg)
	} else {
		return fmt.Errorf("%s is not a supported emulator type", s.Env.Dimensions.DeviceType())
	}

	for _, firmware := range []string{"EDK2", "arm-trusted-firmware-qemu-bios"} {
		fwPkg := emuTypeToPackage[firmware]
		if version, err := build.GetPackageVersion(prebuiltVersions, fwPkg.Name); err != nil {
			return err
		} else {
			fwPkg.Version = version
		}
		pkgs = append(pkgs, fwPkg)
	}
	s.CIPDPackages = pkgs
	return nil
}
