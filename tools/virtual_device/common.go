// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package virtual_device

import (
	"errors"
	"fmt"
	"math"
	"regexp"
	"strconv"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/lib/productbundle"
	fvdpb "go.fuchsia.dev/fuchsia/tools/virtual_device/proto"
)

// Regular expressions for validating FVD properties.
var (
	// ramRe matches a system RAM size description, like '50G'.
	//
	// FindStringSubmatch captures three groups from a valid string: The first is the
	// entire string, the second and third are the ram number and units respectively.
	//
	// See //tools/virtual_device/proto/virtual_device.proto for a full description of the
	// format.
	ramRe = regexp.MustCompile(`^([0-9]+)([mMgG])$`)

	// macRe matches a MAC address.
	macRe = regexp.MustCompile(`^([0-9A-Fa-f]{2}[:-]){5}[0-9A-Fa-f]{2}$`)
)

// Default returns a VirtualDevice with default values.
func Default() *fvdpb.VirtualDevice {
	return &fvdpb.VirtualDevice{
		Name:   "default",
		Kernel: "qemu-kernel",
		Initrd: "zircon-a",
		Drive: &fvdpb.Drive{
			Id:    "maindisk",
			Image: "storage-full",
		},
		Hw: &fvdpb.HardwareProfile{
			Arch:     "x64",
			CpuCount: 1,
			Ram:      "1M",
			Mac:      "52:54:00:63:5e:7a",
		},
	}
}

// ImageKey uniquely identifies a system image by its name and type.
type ImageKey struct {
	Name string
	Type string
}

// ImageOverrides is a map of image keys to their overridden absolute host paths.
// This is used to bypass the paths defined in the product bundle.
type ImageOverrides map[ImageKey]string

// Validate returns nil iff the given FVD is valid for the given product bundle and overrides.
//
// All system images referenced in the FVD must exist in the product bundle or overrides.
func Validate(fvd *fvdpb.VirtualDevice, pb *productbundle.ProductBundle, overrides ImageOverrides) error {
	if fvd == nil {
		return errors.New("virtual device cannot be nil")
	}
	if pb == nil {
		return errors.New("product bundle cannot be nil")
	}

	// Ensure the images referenced in the FVD exist in the image manifest or overrides.
	imageByNameAndType := map[ImageKey][]string{}
	for _, image := range pb.SystemA {
		key := ImageKey{Name: image.Name, Type: image.Type}
		imageByNameAndType[key] = append(imageByNameAndType[key], pb.ImagePath(image))
	}
	for _, image := range pb.SystemR {
		if image.Type == "zbi" || image.Type == "vbmeta" {
			key := ImageKey{Name: "zircon-r", Type: image.Type}
			imageByNameAndType[key] = append(imageByNameAndType[key], pb.ImagePath(image))
		}
	}

	// Apply overrides.
	for key, path := range overrides {
		imageByNameAndType[key] = []string{path}
	}

	// A helper function that ensures an image exists in the manifest, has a unique
	// name within the manifest, and has a non-empty path.
	uniqueImageExists := func(name, typ string) error {
		if imagePaths, ok := imageByNameAndType[ImageKey{Name: name, Type: typ}]; !ok {
			return fmt.Errorf("image %q of type %q not found", name, typ)
		} else if len(imagePaths) != 1 {
			return fmt.Errorf("manifest contains multiple images named %q of type %q: %v", name, typ, imagePaths)
		} else if imagePaths[0] == "" {
			return fmt.Errorf("no path specified for image %q", name)
		}
		return nil
	}

	if err := uniqueImageExists(fvd.Kernel, "kernel"); err != nil {
		return err
	}

	if err := uniqueImageExists(fvd.Initrd, "zbi"); err != nil {
		return err
	}

	// If drive points to a file instead of an entry in the image manifest, the filepath
	// will be checked at runtime instead since it may not exist when this function is
	// called (e.g. it could be a MinFS image which is created during a test run).
	if fvd.Drive != nil && !fvd.Drive.IsFilename {
		if err := uniqueImageExists(fvd.Drive.Image, "blk"); err != nil {
			return fmt.Errorf("%s", err)
		}
	}

	if !isValidRAM(fvd.Hw.Ram) {
		return fmt.Errorf("invalid ram: %q", fvd.Hw.Ram)
	}
	if !isValidArch(fvd.Hw.Arch) {
		return fmt.Errorf("invalid arch: %q", fvd.Hw.Arch)
	}
	if !isValidMAC(fvd.Hw.Mac) {
		return fmt.Errorf("invalid MAC address: %q", fvd.Hw.Mac)
	}
	return nil
}

// ResolveImage resolves the path to the image of the given name and type,
// checking overrides first, then the product bundle.
func ResolveImage(pb *productbundle.ProductBundle, overrides ImageOverrides, name, typ string) (string, error) {
	if path, ok := overrides[ImageKey{Name: name, Type: typ}]; ok {
		return path, nil
	}
	if name == "zircon-r" {
		for _, image := range pb.SystemR {
			if image.Type == typ {
				return pb.ImagePath(image), nil
			}
		}
	}
	for _, image := range pb.SystemA {
		if image.Name == name && image.Type == typ {
			return pb.ImagePath(image), nil
		}
	}
	return "", fmt.Errorf("could not find %s of type %s", name, typ)
}

func isValidRAM(ram string) bool {
	return ramRe.MatchString(ram)
}

func isValidArch(arch string) bool {
	return arch == "x64" || arch == "arm64"
}

func isValidMAC(mac string) bool {
	return macRe.MatchString(mac)
}

func parseRAMBytes(ram string) (int, error) {
	if !isValidRAM(ram) {
		return -1, fmt.Errorf("invalid ram: %q", ram)
	}

	matches := ramRe.FindStringSubmatch(ram)
	size, err := strconv.ParseInt(matches[1], 10, 64)
	if err != nil {
		return -1, err
	}
	unit := strings.ToLower(matches[2])
	power := map[string]float64{"m": 0, "g": 1}[unit]
	bytes := int(size * int64(math.Pow(1024, power)))
	return bytes, nil
}
