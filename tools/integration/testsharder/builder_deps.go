// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package testsharder

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/build"
	fintpb "go.fuchsia.dev/fuchsia/tools/integration/fint/proto"
	"go.fuchsia.dev/fuchsia/tools/lib/hostplatform"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

func testBotPlatform(cpu string) (string, error) {
	// The test bot platform should use the same OS as the host where testsharder is run
	// so just get the current host platform and replace the cpu.
	platform, err := hostplatform.Name()
	if err != nil {
		return "", err
	}
	parts := strings.Split(platform, "-")
	return fmt.Sprintf("%s-%s", parts[0], cpu), nil
}

// AddShardDeps adds tools from the build that are required to run on the shard.
func AddShardDeps(ctx context.Context, shards []*Shard, args build.Args, tools build.Tools) error {
	// First get the build metadata from the args.json to add to the shard and to help
	// determine what deps need to be added to the shard.
	// TODO(ihuh): Dedupe this with fint so only fint needs to know how to generate
	// this metadata.
	var board, product, targetArch, compilationMode string
	var variants []string
	if err := args.Get("build_info_product", &product); err != nil {
		return err
	}
	if err := args.Get("build_info_board", &board); err != nil {
		return err
	}
	if err := args.Get("target_cpu", &targetArch); err != nil {
		return err
	}
	if err := args.Get("compilation_mode", &compilationMode); err != nil {
		return err
	}
	if err := args.Get("select_variant", &variants); err != nil {
		// variants may not be set, which is ok.
		logger.Debugf(ctx, "%s", err)
	}
	buildMetadata := fintpb.SetArtifacts_Metadata{
		Board:           board,
		CompilationMode: compilationMode,
		Product:         product,
		TargetArch:      targetArch,
		Variants:        variants,
	}

	// Add tools to run on host on shard.
	for _, s := range shards {
		s.BuildMetadata = buildMetadata
		// Ignore shards that are skipped. A skipped shard will have a Summary attached to it.
		if len(s.Summary.Tests) > 0 {
			continue
		}
		platform, err := testBotPlatform(s.HostCPU)
		if err != nil {
			return err
		}
		toolDeps := []string{"ssh"}
		if s.HostCPU == "x64" {
			// Relevant for automatic symbolization of things running on
			// host. Only the x64 variation is available in the checkout and
			// we have nothing that runs on an arm host that needs
			// symbolizing.
			toolDeps = append(toolDeps, "llvm-symbolizer")
		}
		toolPaths := []string{}
		for _, toolName := range toolDeps {
			tool, err := tools.LookupTool(platform, toolName)
			if err != nil {
				return err
			}
			toolPaths = append(toolPaths, tool.Path)
		}
		s.AddDeps(toolPaths)
	}
	return nil
}

// GetBuilderDeps returns all the deps required from the checkout/build that a builder needs
// as file paths relative to the checkout root, including all shard deps and deps required by
// the orchestrator builder.
func GetBuilderDeps(shards []*Shard, buildDir, checkoutDir string, tools build.Tools, hostPlatform string, extraDeps []string) ([]string, error) {
	allDepsMap := make(map[string]string)
	for _, s := range shards {
		for _, dep := range s.Deps {
			if _, ok := allDepsMap[dep]; !ok {
				absPath := filepath.Join(buildDir, dep)
				relToCheckout, err := filepath.Rel(checkoutDir, absPath)
				if err != nil {
					return nil, fmt.Errorf("failed to get rel path of %s to checkout: %w", absPath, err)
				}
				allDepsMap[dep] = relToCheckout
			}
		}
	}

	// Add tools to deps file that will be run on the orchestrator which runs on an x64 host.
	toolsToAdd := []string{"llvm-symbolizer", "symbolizer", "tefmocheck", "triage", "resultdb", "perfcompare", "test_filterer"}
	for _, tool := range toolsToAdd {
		t, err := tools.LookupTool(hostPlatform, tool)
		if err != nil {
			return nil, err
		}
		absPath := filepath.Join(buildDir, t.Path)
		relToCheckout, err := filepath.Rel(checkoutDir, absPath)
		if err != nil {
			return nil, fmt.Errorf("failed to get rel path of %s to checkout: %w", absPath, err)
		}
		allDepsMap[t.Path] = relToCheckout
	}
	// Add other files required by the orchestrator.
	for _, file := range extraDeps {
		relToCheckout, err := filepath.Rel(checkoutDir, file)
		if err != nil {
			return nil, fmt.Errorf("failed to get rel path of %s to checkout: %w", file, err)
		}
		if strings.HasPrefix(relToCheckout, "..") {
			return nil, fmt.Errorf("%s must be within the checkout root at %s", file, checkoutDir)
		}
		allDepsMap[file] = relToCheckout
	}
	var allDeps []string
	for _, relPath := range allDepsMap {
		allDeps = append(allDeps, relPath)
	}
	sort.Strings(allDeps)
	return allDeps, nil
}

// WriteCASPathsJSONFile writes all the builder deps to a file in the format
// expected from `cas archive -paths-json`.
func WriteCASPathsJSONFile(allDeps []string, buildDir, checkoutDir, depsFile string) error {
	casPaths := [][2]string{}
	for _, relPath := range allDeps {
		casPaths = append(casPaths, [2]string{checkoutDir, relPath})
	}
	f, err := os.Create(depsFile)
	if err != nil {
		return fmt.Errorf("unable to create %s: %w", depsFile, err)
	}
	encoder := json.NewEncoder(f)
	encoder.SetIndent("", "    ")
	if err := encoder.Encode(&casPaths); err != nil {
		return fmt.Errorf("failed to encode deps: %w", err)
	}
	if err := f.Close(); err != nil {
		return fmt.Errorf("failed to close: %w", err)
	}
	return nil
}
