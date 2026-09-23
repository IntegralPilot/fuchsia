// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Package productbundle defines Go structs representing the schema of the
// product_bundle.json file produced by Fuchsia assembly.
//
// These structs are a subset of the ProductBundleV2 schema defined in Rust at
// //src/lib/assembly/product_bundle/src/v2.rs, containing only the fields
// required by host-side tools written in Go.
package productbundle

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
)

type BootloaderPartition struct {
	Type  string `json:"type"`
	Name  string `json:"name"`
	Image string `json:"image"`
}

type BootstrapCondition struct {
	Variable string `json:"variable"`
	Value    string `json:"value"`
}

type BootstrapPartition struct {
	Name      string              `json:"name"`
	Image     string              `json:"image"`
	Condition *BootstrapCondition `json:"condition,omitempty"`
}

type Partition struct {
	Type string  `json:"type"`
	Name string  `json:"name"`
	Slot string  `json:"slot,omitempty"`
	Size *uint64 `json:"size,omitempty"`
}

type SystemImage struct {
	Type string `json:"type"`
	Name string `json:"name"`
	// Path is the relative path to the image.
	Path     string          `json:"path"`
	Signed   *bool           `json:"signed,omitempty"`
	Contents json.RawMessage `json:"contents,omitempty"`
}

type Repository struct {
	Name                    string `json:"name"`
	MetadataPath            string `json:"metadata_path"`
	BlobsPath               string `json:"blobs_path"`
	DeliveryBlobType        uint32 `json:"delivery_blob_type"`
	RootPrivateKeyPath      string `json:"root_private_key_path,omitempty"`
	TargetsPrivateKeyPath   string `json:"targets_private_key_path,omitempty"`
	SnapshotPrivateKeyPath  string `json:"snapshot_private_key_path,omitempty"`
	TimestampPrivateKeyPath string `json:"timestamp_private_key_path,omitempty"`
}

type PartitionsConfig struct {
	BootstrapPartitions  []BootstrapPartition  `json:"bootstrap_partitions,omitempty"`
	BootloaderPartitions []BootloaderPartition `json:"bootloader_partitions,omitempty"`
	Partitions           []Partition           `json:"partitions,omitempty"`
	HardwareRevision     string                `json:"hardware_revision"`
	ProductMatches       []string              `json:"product_matches,omitempty"`
	UnlockCredentials    []string              `json:"unlock_credentials,omitempty"`
}

type ProductBundle struct {
	Version            string           `json:"version"`
	ProductName        string           `json:"product_name"`
	ProductVersion     string           `json:"product_version"`
	Partitions         PartitionsConfig `json:"partitions"`
	SdkVersion         string           `json:"sdk_version"`
	SystemA            []SystemImage    `json:"system_a,omitempty"`
	SystemB            []SystemImage    `json:"system_b,omitempty"`
	SystemR            []SystemImage    `json:"system_r,omitempty"`
	Repositories       []Repository     `json:"repositories,omitempty"`
	UpdatePackageHash  *string          `json:"update_package_hash,omitempty"`
	VirtualDevicesPath *string          `json:"virtual_devices_path,omitempty"`
	// BaseDir is the directory path containing product_bundle.json, used to
	// resolve relative paths. It is only set when loaded via Load().
	BaseDir string `json:"-"`
}

// GetSystemAImage returns the absolute path to the image of the given type and name.
func (pb *ProductBundle) GetSystemAImage(imageType string, imageName string) (string, error) {
	for _, image := range pb.SystemA {
		if image.Type == imageType && image.Name == imageName {
			return pb.ImagePath(image), nil
		}
	}

	return "", fmt.Errorf("failed to find system_a %s %s", imageType, imageName)
}

// Load parses product_bundle.json from the given directory and returns the ProductBundle.
//
// It sets the BaseDir field to the given pbDir.
func Load(pbDir string) (*ProductBundle, error) {
	pbJSONPath := filepath.Join(pbDir, "product_bundle.json")
	data, err := os.ReadFile(pbJSONPath)
	if err != nil {
		return nil, fmt.Errorf("failed to read %s: %w", pbJSONPath, err)
	}
	var pb ProductBundle
	if err := json.Unmarshal(data, &pb); err != nil {
		return nil, fmt.Errorf("failed to unmarshal %s: %w", pbJSONPath, err)
	}
	pb.BaseDir = pbDir
	return &pb, nil
}

// ImagePath returns the absolute path to the given image.
func (pb *ProductBundle) ImagePath(image SystemImage) string {
	if image.Path == "" {
		return ""
	}
	if filepath.IsAbs(image.Path) || pb.BaseDir == "" {
		return image.Path
	}
	return filepath.Join(pb.BaseDir, image.Path)
}
