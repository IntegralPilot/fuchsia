// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package targets

import (
	"context"
	"fmt"
	"testing"
)

func TestNewEmulator(t *testing.T) {
	for _, tc := range []struct {
		name       string
		emuType    string
		wantBinary string
		wantErr    bool
	}{{
		name:       "qemu",
		emuType:    "qemu",
		wantBinary: fmt.Sprintf("%s-%s", qemuSystemPrefix, qemuTargetMapping[TargetX64]),
	}, {
		name:       "aemu",
		emuType:    "aemu",
		wantBinary: aemuBinaryName,
	}, {
		name:       "crosvm",
		emuType:    "crosvm",
		wantBinary: "crosvm",
	}, {
		name:    "unknown",
		emuType: "unknown",
		wantErr: true,
	}} {
		t.Run(tc.name, func(t *testing.T) {
			ctx := context.Background()
			emu, err := NewEmulator(
				ctx,
				EmulatorConfig{
					Target: "x64",
				},
				Options{},
				tc.emuType,
			)
			if err != nil && !tc.wantErr {
				t.Fatalf("Unable to create NewEmulator: %s", err)
			} else if err == nil && tc.wantErr {
				t.Fatalf("expected err but got none")
			}

			if !tc.wantErr && emu.binary != tc.wantBinary {
				t.Errorf("Unexpected emu binary %s, expected %s", emu.binary, tc.wantBinary)
			}
		})
	}
}
