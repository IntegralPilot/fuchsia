// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package artifactory

import (
	"path/filepath"
	"reflect"
	"testing"

	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/lib/jsonutil"
)

// Implements productBundlesModules.
type mockProductBundlesModules struct {
	productBundles []build.ProductBundle
	buildDir       string
}

func (m mockProductBundlesModules) BuildDir() string {
	return m.buildDir
}

func (m mockProductBundlesModules) ProductBundles() []build.ProductBundle {
	return m.productBundles
}

func TestProductBundle2Uploads(t *testing.T) {
	transferManifest := TransferManifest{
		Version: "1",
		Entries: []TransferManifestEntry{
			{
				Type:   "files",
				Local:  "",
				Remote: "",
				Entries: []ArtifactEntry{
					{
						Name: "123",
					},
					{
						Name: "sub/456",
					},
					{
						Name: "sub/sub/789",
					},
				},
			},
			{
				Type:   "blobs",
				Local:  "blobs/1",
				Remote: "1",
				Entries: []ArtifactEntry{
					{
						Name: "abc",
					},
					{
						Name: "def",
					},
					{
						Name: "ghi",
					},
				},
			},
		},
	}
	dir := t.TempDir()
	transferManifestName := "transfer.json"
	if err := jsonutil.WriteToFile(filepath.Join(dir, transferManifestName), transferManifest); err != nil {
		t.Fatalf("failed to write to fake transfer.json file: %v", err)
	}

	m := &mockProductBundlesModules{
		productBundles: []build.ProductBundle{
			{
				Name:                 "my-product-bundle",
				Path:                 dir,
				TransferManifestPath: transferManifestName,
			},
		},
		buildDir: dir,
	}

	expectedTransferManifest := []byte(`{
  "version": "1",
  "entries": [
    {
      "type": "files",
      "local": "",
      "remote": "PRODUCT_BUNDLES/my-product-bundle",
      "entries": [
        {
          "name": "123"
        },
        {
          "name": "sub/456"
        },
        {
          "name": "sub/sub/789"
        }
      ]
    },
    {
      "type": "blobs",
      "local": "blobs/1",
      "remote": "BLOBS/1",
      "entries": [
        {
          "name": "abc"
        },
        {
          "name": "def"
        },
        {
          "name": "ghi"
        }
      ]
    }
  ]
}`)
	expected := []Upload{
		{
			Source:      filepath.Join(dir, "123"),
			Destination: filepath.Join("PRODUCT_BUNDLES", "my-product-bundle", "123"),
			Signed:      true,
		},
		{
			Source:      filepath.Join(dir, "sub", "456"),
			Destination: filepath.Join("PRODUCT_BUNDLES", "my-product-bundle", "sub", "456"),
			Signed:      true,
		},
		{
			Source:      filepath.Join(dir, "sub", "sub", "789"),
			Destination: filepath.Join("PRODUCT_BUNDLES", "my-product-bundle", "sub", "sub", "789"),
			Signed:      true,
		},
		{
			Source:      filepath.Join(dir, "blobs/1"),
			Destination: filepath.Join("BLOBS/1"),
			Deduplicate: true,
			Signed:      true,
		},
		{
			Compress:    true,
			Contents:    []byte(expectedTransferManifest),
			Destination: filepath.Join("PRODUCT_BUNDLES", "my-product-bundle", "transfer.json"),
			Signed:      true,
		},
	}

	actual, err := productBundle2Uploads(m, "BLOBS", "PRODUCT_BUNDLES")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !reflect.DeepEqual(actual, expected) {
		t.Fatalf("unexpected product bundle uploads:\nexpected: %v\nactual: %v\n", expected, actual)
	}
}
