// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package artifactory

import (
	"testing"

	"github.com/google/go-cmp/cmp"
	"go.fuchsia.dev/fuchsia/tools/build"
)

// Implements imgModules
type mockModules struct {
	imgs     []build.Image
	buildDir string
}

func (m mockModules) BuildDir() string {
	return m.buildDir
}

func (m mockModules) ImageManifest() string {
	return "BUILD_DIR/IMAGE_MANIFEST"
}

func (m mockModules) Images() []build.Image {
	return m.imgs
}

func TestImageUploads(t *testing.T) {
	m := &mockModules{
		buildDir: "BUILD_DIR",
		imgs: []build.Image{
			{
				Name: "some-other-image",
				Path: "image.bin",
				Type: "bin",
			},
		},
	}
	want := []Upload{
		{
			Source:      "BUILD_DIR/IMAGE_MANIFEST",
			Destination: "namespace/IMAGE_MANIFEST",
			Signed:      true,
		},
	}
	got, err := imageUploads(m, "namespace")
	if err != nil {
		t.Fatalf("imageUploads failed: %s", err)
	}
	if diff := cmp.Diff(want, got); diff != "" {
		t.Fatalf("unexpected image uploads (-want +got):\n%s", diff)
	}
}
