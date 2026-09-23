// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package build

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
)

type Blob struct {
	// Merkle is the merkle associated with a blob.
	Merkle string `json:"merkle"`
}

type DeliveryBlobConfig struct {
	Type int `json:"type"`
}

func GetDeliveryBlobType(configPath string) (*int, error) {
	data, err := os.ReadFile(configPath)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, nil
		}
		return nil, fmt.Errorf("failed to read delivery blob config: %w", err)
	}

	var config DeliveryBlobConfig
	err = json.Unmarshal(data, &config)
	if err != nil {
		return nil, fmt.Errorf("unable to unmarshal delivery blob config: %w", err)
	}
	return &config.Type, nil
}

// Get the dir relative to repo that store blobs, if delivery blobs are enabled,
// this will return the sub directory that contains delivery blobs with the
// correct type.
func GetBlobsDir(deliveryBlobConfigPath string) (string, error) {
	blobType, err := GetDeliveryBlobType(deliveryBlobConfigPath)
	if err != nil {
		return "", fmt.Errorf("unable to get delivery blob type: %w", err)
	}
	blobsDir := "blobs"
	if blobType == nil {
		return blobsDir, nil
	}
	return filepath.Join(blobsDir, fmt.Sprint(*blobType)), nil
}
