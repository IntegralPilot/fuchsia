// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package targets

import (
	"fmt"
)

// Auxiliary target contains the properties of a target auxiliary device.
type AuxiliaryConfig map[string]any

type Auxiliary struct {
	config AuxiliaryConfig
}

// TestConfig returns fields describing the target to be provided to tests.
func (t *Auxiliary) TestConfig(expectsSSH bool) (any, error) {
	return t.config, nil
}

// NewAuxiliary returns a new Auxiliary target with a given configuration.
func NewAuxiliary(config AuxiliaryConfig) (*Auxiliary, error) {
	device, ok := config["device"].(string)
	if !ok {
		return nil, fmt.Errorf("auxiliary config requires target to be a string.")
	}
	if device == "" {
		return nil, fmt.Errorf("device cannot be an empty string.")
	}

	config["type"] = device

	delete(config, "device")

	return &Auxiliary{
		config: config,
	}, nil
}
