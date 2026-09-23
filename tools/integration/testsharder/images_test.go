// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package testsharder

import (
	"context"
	"path/filepath"
	"sort"
	"strings"
	"testing"

	"github.com/google/go-cmp/cmp"

	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/lib/ffxutil"
)

type mockFFX struct {
	ffxutil.MockFFXInstance
}

func (m *mockFFX) GetPBArtifacts(ctx context.Context, pbPath, group string) ([]string, error) {
	var artifacts []string
	if group == "bootloader" {
		if strings.Contains(pbPath, "efi") {
			artifacts = append(artifacts, "firmware_efi-shell:efi")
		}
	} else {
		artifacts = append(artifacts, "zbi")
		if group == "emu" {
			artifacts = append(artifacts, "qemu-kernel")
		}
	}
	return artifacts, nil
}

func TestAddImageDeps(t *testing.T) {
	imgDir := t.TempDir()
	defaultDeps := func(pbPath string, emu bool) []string {
		deps := []string{"product_bundles.json"}
		if pbPath == "" {
			pbPath = "obj/build/images/fuchsia/product_bundle"
		}
		deps = append(deps, filepath.Join(pbPath, "zbi"))
		if emu {
			deps = append(deps, filepath.Join(pbPath, "qemu-kernel"))
		}
		return deps
	}
	testCases := []struct {
		name       string
		deviceType string
		pbPath     string
		want       []string
	}{
		{
			name:       "emulator image deps",
			deviceType: "AEMU",
			want:       defaultDeps("", true),
		},
		{
			name:       "device image deps",
			deviceType: "NUC",
			want:       defaultDeps("", false),
		},
		{
			name:       "emulator env with efi",
			deviceType: "AEMU",
			pbPath:     "efi-boot-test/product_bundle",
			want:       append(defaultDeps("efi-boot-test/product_bundle", true), "efi-boot-test/product_bundle/efi"),
		},
		{
			name:       "GCE image deps",
			deviceType: "GCE",
		},
		{
			name: "host-test only shard image deps",
		},
	}
	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			s := &Shard{
				Env: build.Environment{
					Dimensions: build.DimensionSet{
						"device_type": tc.deviceType,
					},
				},
			}
			origGetFFX := GetFFX
			defer func() {
				GetFFX = origGetFFX
			}()
			GetFFX = func(ctx context.Context, ffxPath, outputsDir string) (FFXInterface, error) {
				return &mockFFX{}, nil
			}
			pbPath := "obj/build/images/fuchsia/product_bundle"
			if tc.pbPath != "" {
				pbPath = tc.pbPath
			}
			AddImageDeps(context.Background(), s, imgDir, pbPath, "path/to/ffx")
			sort.Strings(tc.want)
			sort.Strings(s.Deps)
			if diff := cmp.Diff(tc.want, s.Deps); diff != "" {
				t.Errorf("AddImageDeps(%v) failed: (-want +got): \n%s", s, diff)
			}
		})
	}
}
