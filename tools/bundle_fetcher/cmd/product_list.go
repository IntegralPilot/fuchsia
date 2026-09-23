// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"os"
	"path"
	"slices"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/bundle_fetcher/bundler"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"

	"github.com/google/subcommands"
)

type Configuration struct {
	// Board is the board name. e.g. "x64", "vim3".
	Board string `json:"board"`

	// Product is the product name. e.g. "minimal", "core".
	Product string `json:"product"`
}

// BuildInfo represent the build info build-api.
type BuildInfo struct {
	// Conguration is the array of product/board dictionary.
	Configurations []Configuration `json:"configurations"`

	// A unique version of this build.
	Version string `json:"version"`
}

type productListCmd struct {
	gcsBucket                 string
	buildIDs                  string
	productBundles            string
	outDir                    string
	outputProductListFileName string
}

const (
	buildsDirName          = "builds"
	buildApiDirName        = "build_api"
	buildInfoJSONName      = "build_info.json"
	productBundlesDirName  = "product_bundles"
	productBundlesJSONName = "product_bundles.json"
	transferJSONName       = "transfer.json"
)

func (*productListCmd) Name() string { return "product-list" }

func (*productListCmd) Synopsis() string {
	return "Create a product list to index urls to transfer manifest files."
}

func (*productListCmd) Usage() string {
	return "bundle_fetcher product-list -bucket <GCS_BUCKET> -build_ids <build_ids> [-product_bundles <product_bundles>]\n"
}

func (cmd *productListCmd) SetFlags(f *flag.FlagSet) {
	f.StringVar(&cmd.gcsBucket, "bucket", "", "GCS bucket from which to read the files from.")
	f.StringVar(&cmd.buildIDs, "build_ids", "", "Comma separated list of build_ids.")
	f.StringVar(&cmd.productBundles, "product_bundles", "", "Comma separated list of product bundles.")
	f.StringVar(&cmd.outDir, "out_dir", "", "Directory to write out_file_name to.")
	f.StringVar(&cmd.outputProductListFileName, "out_file_name", "product_bundles.json", "Name of the output file containing the product bundle to transfer lookup information.")
}

func (cmd *productListCmd) parseFlags() error {
	if cmd.buildIDs == "" {
		return fmt.Errorf("-build_ids is required")
	}

	if cmd.gcsBucket == "" {
		return fmt.Errorf("-bucket is required")
	}

	if cmd.outDir == "" {
		return fmt.Errorf("-out_dir is required")
	}
	info, err := os.Stat(cmd.outDir)
	if os.IsNotExist(err) {
		return fmt.Errorf("out directory path %s does not exist", cmd.outDir)
	}
	if err != nil {
		return err
	}
	if !info.IsDir() {
		return fmt.Errorf("out directory path %s is not a directory", cmd.outDir)
	}
	return nil
}

func (cmd *productListCmd) Execute(ctx context.Context, f *flag.FlagSet, _ ...any) subcommands.ExitStatus {
	if err := cmd.execute(ctx); err != nil {
		logger.Errorf(ctx, "%s", err)
		return subcommands.ExitFailure
	}
	return subcommands.ExitSuccess
}

func (cmd *productListCmd) execute(ctx context.Context) error {
	if err := cmd.parseFlags(); err != nil {
		return err
	}

	sink, err := bundler.NewCloudSink(ctx, cmd.gcsBucket)
	if err != nil {
		return err
	}
	defer sink.Close()

	return cmd.executeWithSink(ctx, sink)
}

func (cmd *productListCmd) executeWithSink(ctx context.Context, sink bundler.DataSink) error {
	productList := build.ProductBundlesManifest{}

	var flagProductNames []string
	if cmd.productBundles != "" {
		for _, p := range strings.Split(cmd.productBundles, ",") {
			if trimmed := strings.TrimSpace(p); trimmed != "" {
				flagProductNames = append(flagProductNames, trimmed)
			}
		}
	}

	buildIDsList := strings.Split(cmd.buildIDs, ",")
	for _, buildID := range buildIDsList {
		buildID = strings.TrimSpace(buildID)
		productBundlePath := path.Join(buildsDirName, buildID, buildApiDirName, productBundlesJSONName)
		logger.Debugf(ctx, "Build %s contains the product bundles in path %s", buildID, productBundlePath)
		productBundles, err := getProductListFromJSON(ctx, sink, productBundlePath)
		if err != nil {
			return fmt.Errorf("unable to read product bundle metdadata for build_id %s: %s %w", buildID, productBundlePath, err)
		}

		buildInfoPath := path.Join(buildsDirName, buildID, buildApiDirName, buildInfoJSONName)
		productName, err := getProductNameFromJSON(ctx, sink, buildInfoPath)
		if err != nil {
			return fmt.Errorf("unable to read build info for build_id %s: %s %w", buildID, buildInfoPath, err)
		}
		productNames := slices.Clone(flagProductNames)
		if !slices.Contains(productNames, productName) {
			productNames = append(productNames, productName)
		}

		for _, productBundle := range *productBundles {
			if !slices.Contains(productNames, productBundle.Name) {
				continue
			}

			transferManifestUrl := getTransferManifestPath(ctx, cmd.gcsBucket, buildsDirName, buildID, productBundle.Name)

			productEntry := build.ProductBundle{
				// Entries for Label and TransferManifestPath are not needed.
				Name:                productBundle.Name,
				ProductVersion:      productBundle.ProductVersion,
				TransferManifestUrl: transferManifestUrl,
			}
			productList = append(productList, productEntry)
		}
	}
	outputFilePath := path.Join(cmd.outDir, cmd.outputProductListFileName)
	logger.Debugf(ctx, "writing final product list file to: %s", outputFilePath)
	f, err := os.Create(outputFilePath)
	if err != nil {
		return fmt.Errorf("Creating '%s' %w", outputFilePath, err)
	}
	var errs error
	enc := json.NewEncoder(f)
	enc.SetIndent("", "  ")
	if err := enc.Encode(&productList); err != nil {
		errs = errors.Join(errs, err)
	}
	if err := f.Close(); err != nil {
		errs = errors.Join(errs, err)
	}
	return errs
}

// getTransferManifestPath will build the transfer manifest path. This path will
// be builds/<buildid>/product_bundles/<product_name>/transfer.json.
func getTransferManifestPath(ctx context.Context, gcsBucket, buildsDirName, buildID, productName string) string {
	transferManifestPath := path.Join(buildsDirName, buildID, productBundlesDirName, productName, transferJSONName)
	return fmt.Sprintf("gs://%s/%s", gcsBucket, transferManifestPath)
}

// Read the product_bundles.json from GCS and returns them as a
// ProductBundlesManifest list.
func getProductListFromJSON(ctx context.Context, sink bundler.DataSink, productListJSONPath string) (*build.ProductBundlesManifest, error) {
	data, err := sink.ReadFromGCS(ctx, productListJSONPath)
	if err != nil {
		return nil, err
	}
	listData := &build.ProductBundlesManifest{}
	err = json.Unmarshal(data, listData)
	if err != nil {
		return nil, err
	}
	if len(*listData) == 0 {
		return nil, fmt.Errorf("error, the product list is empty")
	}
	for _, product := range *listData {
		// Only needed values (by this tool) are validated.
		if product.Name == "" {
			return nil, fmt.Errorf("error, the product name is empty")
		}
		if product.ProductVersion == "" {
			return nil, fmt.Errorf("error, the product version is empty")
		}
	}
	return listData, nil
}

// Read the build_info.json from GCS and returns produt.board string.
func getProductNameFromJSON(ctx context.Context, sink bundler.DataSink, buildInfoJSONPath string) (string, error) {
	data, err := sink.ReadFromGCS(ctx, buildInfoJSONPath)
	if err != nil {
		return "", err
	}
	var buildInfo BuildInfo
	err = json.Unmarshal(data, &buildInfo)
	if err != nil {
		return "", err
	}
	config := buildInfo.Configurations[0]
	return fmt.Sprintf("%s.%s", config.Product, config.Board), nil
}
