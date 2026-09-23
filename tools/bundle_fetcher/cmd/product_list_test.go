// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path"
	"reflect"
	"testing"

	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/bundle_fetcher/bundler"
)

func TestProductListParseFlags(t *testing.T) {
	dir, err := os.MkdirTemp("", "bundle_fetcher_dir")
	if err != nil {
		t.Fatalf("unable to create temp dir")
	}
	tmpfn := path.Join(dir, "tmpfile")
	if err := os.WriteFile(tmpfn, []byte("hello world"), 0644); err != nil {
		t.Fatalf("unable to create temp file")
	}
	defer os.RemoveAll(dir)

	var tests = []struct {
		productListCmd *productListCmd
		expectedErr    string
	}{
		{
			productListCmd: &productListCmd{
				buildIDs:  "123456",
				gcsBucket: "orange",
				outDir:    dir,
			},
		},
		{
			productListCmd: &productListCmd{
				gcsBucket: "orange",
				outDir:    dir,
			},
			expectedErr: "-build_ids is required",
		},
		{
			productListCmd: &productListCmd{
				buildIDs: "123456",
				outDir:   dir,
			},
			expectedErr: "-bucket is required",
		},
		{
			productListCmd: &productListCmd{
				buildIDs:  "123456",
				gcsBucket: "orange",
			},
			expectedErr: "-out_dir is required",
		},
		{
			productListCmd: &productListCmd{
				buildIDs:  "123456",
				gcsBucket: "orange",
				outDir:    tmpfn,
			},
			expectedErr: fmt.Sprintf("out directory path %v is not a directory", tmpfn),
		},
	}

	for _, test := range tests {
		if err := test.productListCmd.parseFlags(); err != nil && err.Error() != test.expectedErr {
			t.Errorf("Got error: %s, want: %s", err.Error(), test.expectedErr)
		}
	}
}

func TestGetProductListFromJSON(t *testing.T) {
	contents := map[string][]byte{
		"some/valid/product_bundles.json": []byte(`[{
			"label": "//build/images/fuchsia:product_bundle(//build/toolchain/fuchsia:x64)",
			"name": "fake_product.x64",
			"path": "obj/build/images/fuchsia/product_bundle",
			"product_version": "fake_version",
			"transfer_manifest_path": "obj/build/images/fuchsia/transfer.json",
			"transfer_manifest_url": "file://obj/build/images/fuchsia/transfer.json"
		  }]`),
		"invalid/product_bundles.json": []byte(`[{
			"label": "//build/images/fuchsia:product_bundle(//build/toolchain/fuchsia:x64)",
			"path": "obj/build/images/fuchsia/product_bundle",
			"product_version": "fake_version"
		  }]`),
		"non-list/product_bundles.json": []byte(`{
			"label": "//build/images/fuchsia:product_bundle(//build/toolchain/fuchsia:x64)",
			"name": "fake_product.x64",
			"path": "obj/build/images/fuchsia/product_bundle",
			"product_version": "fake_version"
		  }`),
	}
	ctx := context.Background()
	var tests = []struct {
		name                string
		productListJSONPath string
		dataSinkErr         error
		expectedOutput      *build.ProductBundlesManifest
		expectedErrMessage  string
	}{
		{
			name:                "valid product_bundles.json",
			productListJSONPath: "some/valid/product_bundles.json",
			expectedOutput: &build.ProductBundlesManifest{
				build.ProductBundle{
					Label:                "//build/images/fuchsia:product_bundle(//build/toolchain/fuchsia:x64)",
					Name:                 "fake_product.x64",
					Path:                 "obj/build/images/fuchsia/product_bundle",
					ProductVersion:       "fake_version",
					TransferManifestPath: "obj/build/images/fuchsia/transfer.json",
					TransferManifestUrl:  "file://obj/build/images/fuchsia/transfer.json",
				},
			},
		},
		{
			name:                "not a list product_bundles.json",
			productListJSONPath: "non-list/product_bundles.json",
			expectedErrMessage:  "json: cannot unmarshal object into Go value of type []build.ProductBundle",
		},
		{
			name:                "missing something product_bundles.json",
			productListJSONPath: "invalid/product_bundles.json",
			expectedErrMessage:  "error, the product name is empty",
		},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			sink := bundler.NewMemSink(contents, test.dataSinkErr, "")
			output, err := getProductListFromJSON(ctx, sink, test.productListJSONPath)
			if !reflect.DeepEqual(output, test.expectedOutput) {
				t.Errorf("Got output for '%s': %v, want: %v", test.name, output, test.expectedOutput)
			}
			if err != nil {
				if err.Error() != test.expectedErrMessage {
					t.Errorf("Got error for '%s': %s, want: %s", test.name, err.Error(), test.expectedErrMessage)
				}
				// else the expected error message was given.
			} else if test.expectedErrMessage != "" {
				t.Errorf("Got no error for '%s', want: %s", test.name, test.expectedErrMessage)
			}
		})
	}
}

func TestExecuteWithSink(t *testing.T) {
	contents := map[string][]byte{
		"builds/123/build_api/product_bundles.json": []byte(`[{
			"label": "//build/images/fuchsia:product_bundle(//build/toolchain/fuchsia:x64)",
			"name": "fake_product.x64",
			"path": "obj/build/images/fuchsia/product_bundle",
			"product_version": "fake_version",
			"transfer_manifest_path": "obj/build/images/fuchsia/transfer.json",
			"transfer_manifest_url": "file://obj/build/images/fuchsia/transfer.json"
		  }]`),
		"builds/123/build_api/build_info.json": []byte(`{
			"configurations": [
			  {
				"board": "x64",
				"product": "fake_product"
			  }
			],
			"version": "fake_version"
		  }`),
		"builds/123/product_bundles/fake_product.x64/transfer.json": []byte(`{
			"content": "fake"
		  }`),
		"builds/456/build_api/product_bundles.json": []byte(`[{
			"label": "//build/images/fuchsia:product_bundle(//build/toolchain/fuchsia:x64)",
			"path": "obj/build/images/fuchsia/product_bundle",
			"product_version": "fake_version"
		  }]`),
		"builds/789/build_api/product_bundles.json": []byte(`{
			"label": "//build/images/fuchsia:product_bundle(//build/toolchain/fuchsia:x64)",
			"name": "fake_product.x64",
			"path": "obj/build/images/fuchsia/product_bundle",
			"product_version": "fake_version"
		  }`),
		"builds/999/build_api/product_bundles.json": []byte(`[{
			"label": "//build/images/fuchsia:product_bundle(//build/toolchain/fuchsia:x64)",
			"name": "custom_product.x64",
			"path": "obj/build/images/fuchsia/product_bundle",
			"product_version": "fake_version",
			"transfer_manifest_path": "obj/build/images/fuchsia/transfer.json",
			"transfer_manifest_url": "file://obj/build/images/fuchsia/transfer.json"
		  },
		  {
			"label": "//build/images/fuchsia:product_bundle(//build/toolchain/fuchsia:x64)",
			"name": "another_product.x64",
			"path": "obj/build/images/fuchsia/product_bundle",
			"product_version": "fake_version",
			"transfer_manifest_path": "obj/build/images/fuchsia/transfer.json",
			"transfer_manifest_url": "file://obj/build/images/fuchsia/transfer.json"
		  }]`),
		"builds/999/product_bundles/custom_product.x64/transfer.json": []byte(`{
			"content": "fake"
		  }`),
		"builds/999/product_bundles/another_product.x64/transfer.json": []byte(`{
			"content": "fake"
		  }`),
		"builds/999/build_api/build_info.json": []byte(`{
			"configurations": [
			  {
				"board": "x64",
				"product": "another_product"
			  }
			],
			"version": "fake_version"
		  }`),
	}
	ctx := context.Background()
	var tests = []struct {
		name               string
		buildIDs           string
		productBundles     string
		dataSinkErr        error
		expectedOutput     *build.ProductBundlesManifest
		expectedErrMessage string
	}{
		{
			name:     "valid_product_bundles.json",
			buildIDs: "123",
			expectedOutput: &build.ProductBundlesManifest{
				build.ProductBundle{
					Name:                "fake_product.x64",
					ProductVersion:      "fake_version",
					TransferManifestUrl: "gs://orange/builds/123/product_bundles/fake_product.x64/transfer.json",
				},
			},
		},
		{
			name:               "missing_name_product_bundles.json",
			buildIDs:           "456",
			expectedErrMessage: "unable to read product bundle metdadata for build_id 456: builds/456/build_api/product_bundles.json error, the product name is empty",
		},
		{
			name:               "not_a_list_product_bundles.json",
			buildIDs:           "789",
			expectedErrMessage: "unable to read product bundle metdadata for build_id 789: builds/789/build_api/product_bundles.json json: cannot unmarshal object into Go value of type []build.ProductBundle",
		},
		{
			name:           "valid_product_bundles_with_arg",
			buildIDs:       "999",
			productBundles: "custom_product.x64",
			expectedOutput: &build.ProductBundlesManifest{
				build.ProductBundle{
					Name:                "custom_product.x64",
					ProductVersion:      "fake_version",
					TransferManifestUrl: "gs://orange/builds/999/product_bundles/custom_product.x64/transfer.json",
				},
				build.ProductBundle{
					Name:                "another_product.x64",
					ProductVersion:      "fake_version",
					TransferManifestUrl: "gs://orange/builds/999/product_bundles/another_product.x64/transfer.json",
				},
			},
		},
	}

	dir, err := os.MkdirTemp("", "bundle_fetcher_dir")
	if err != nil {
		t.Fatalf("unable to create temp dir")
	}
	tmpfn := path.Join(dir, "tmpfile")
	if err := os.WriteFile(tmpfn, []byte("hello world"), 0644); err != nil {
		t.Fatalf("unable to create temp file")
	}
	defer os.RemoveAll(dir)
	temp_pb_output := path.Join(dir, productBundlesJSONName)

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			sink := bundler.NewMemSink(contents, test.dataSinkErr, "")
			cmd := productListCmd{
				buildIDs:                  test.buildIDs,
				productBundles:            test.productBundles,
				gcsBucket:                 "orange",
				outDir:                    dir,
				outputProductListFileName: productBundlesJSONName,
			}
			err := cmd.executeWithSink(ctx, sink)
			if err != nil {
				if err.Error() != test.expectedErrMessage {
					t.Fatalf("Got error for '%s': %s, want: %s", test.name, err.Error(), test.expectedErrMessage)
				}
				return
			}
			if test.expectedErrMessage != "" {
				t.Fatalf("Got no error for '%s', want: %s", test.name, test.expectedErrMessage)
			}

			data, err := os.ReadFile(temp_pb_output)
			if err != nil {
				t.Fatalf("Unable to read output: %v", err)
				return
			}
			defer os.Remove(temp_pb_output)
			listData := &build.ProductBundlesManifest{}
			err = json.Unmarshal(data, listData)
			if err != nil {
				t.Fatalf("Unmarshal failed '%s': %v", test.name, listData)
			}
			if !reflect.DeepEqual(listData, test.expectedOutput) {
				t.Fatalf("Got output for '%s': %v, want: %v", test.name, listData, test.expectedOutput)
			}
		})
	}
}
