// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"encoding/json"
	"reflect"
	"testing"

	"go.fuchsia.dev/fuchsia/tools/orchestrate"
)

func TestRunInputJSONMerge(t *testing.T) {
	base := []byte(`{
		"emulator": {
			"ffx_path": "base_path",
			"package_archives": ["base.far"],
			"cipd": {"path1": "version1"}
		}
	}`)
	override := []byte(`{
		"emulator": {
			"package_archives": ["override.far"],
			"cipd": {"path2": "version2"}
		}
	}`)

	var input orchestrate.RunInput
	if err := json.Unmarshal(base, &input); err != nil {
		t.Fatalf("Failed to unmarshal base: %v", err)
	}
	if err := json.Unmarshal(override, &input); err != nil {
		t.Fatalf("Failed to unmarshal override: %v", err)
	}

	want := &orchestrate.RunInput{
		Emulator: orchestrate.EmulatorRunInput{
			TargetRunInput: orchestrate.TargetRunInput{
				FfxPath:         "base_path",
				PackageArchives: []string{"override.far"},                                    // Replaced
				Cipd:            map[string]string{"path1": "version1", "path2": "version2"}, // Merged
			},
		},
	}

	if !reflect.DeepEqual(want, &input) {
		t.Errorf("JSON Merge failed:\nwant: %+v\ngot:  %+v", want, &input)
	}
}

func TestRunInputOverridesValidate(t *testing.T) {
	testCases := []struct {
		name     string
		base     string
		override string
		wantErr  bool
	}{
		{
			name: "Valid Base, Valid Override",
			base: `{
				"emulator": {
					"ffx_path": "base_path",
					"transfer_url": "gs://foo/bar.json"
				}
			}`,
			override: `{
				"emulator": {
					"package_archives": ["override.far"]
				}
			}`,
			wantErr: false,
		},
		{
			name: "Override Makes TransferURL and LocalPB Mutually Exclusive",
			base: `{
				"emulator": {
					"ffx_path": "base_path",
					"transfer_url": "gs://foo/bar.json"
				}
			}`,
			override: `{
				"emulator": {
					"local_pb": "foo/bar"
				}
			}`,
			wantErr: true,
		},
		{
			name: "Override Adds Hardware Target Making Them Mutually Exclusive",
			base: `{
				"emulator": {
					"ffx_path": "base_path",
					"transfer_url": "gs://foo/bar.json"
				}
			}`,
			override: `{
				"hardware": {
					"ffx_path": "hw_path",
					"transfer_url": "gs://foo/bar.json"
				}
			}`,
			wantErr: true,
		},
		{
			name: "Override Clears TransferURL Resulting In Neither TransferURL Nor LocalPB Set",
			base: `{
				"emulator": {
					"ffx_path": "base_path",
					"transfer_url": "gs://foo/bar.json"
				}
			}`,
			override: `{
				"emulator": {
					"transfer_url": ""
				}
			}`,
			wantErr: true,
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			var input orchestrate.RunInput
			if err := json.Unmarshal([]byte(tc.base), &input); err != nil {
				t.Fatalf("Failed to unmarshal base: %v", err)
			}
			if err := json.Unmarshal([]byte(tc.override), &input); err != nil {
				t.Fatalf("Failed to unmarshal override: %v", err)
			}
			err := input.Validate()
			if tc.wantErr && err == nil {
				t.Error("Validate() succeeded, want error")
			} else if !tc.wantErr && err != nil {
				t.Errorf("Validate() failed: %v, want no error", err)
			}
		})
	}
}
