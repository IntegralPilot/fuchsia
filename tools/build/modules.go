// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package build

import (
	"path/filepath"

	"golang.org/x/exp/maps"
	"golang.org/x/exp/slices"
)

const (
	imageManifestName = "images"
)

// Modules is a convenience interface for accessing the various build API
// modules associated with a build.
//
// For information about each build API module, see the corresponding
// `build_api_module` target in //BUILD.gn.
type Modules struct {
	buildDir string
	// keep-sorted start
	apis                     []string
	args                     Args
	assemblyInputArchives    []AssemblyInputArchive
	assemblyManifests        []AssemblyManifest
	binaries                 []Binary
	checkoutArtifacts        []CheckoutArtifact
	clippyTargets            []ClippyTarget
	generatedSources         []string
	images                   []Image
	licenses                 []License
	packageRepositories      []PackageRepo
	platforms                []DimensionSet
	prebuiltBinarySets       []PrebuiltBinarySet
	prebuiltVersionsLocation PrebuiltVersionsLocation
	productBundles           []ProductBundle
	productSizeCheckerOutput []ProductSizeCheckerOutput
	sdkArchives              []SDKArchive
	tefmocheckRulesManifest  []string
	testDurations            []TestDuration
	testListLocation         []string
	testSpecs                []TestSpec
	tools                    Tools
	triageSources            []string
	// keep-sorted end
}

// NewModules returns a Modules associated with a given build directory.
func NewModules(buildDir string) (*Modules, error) {
	m := &Modules{buildDir: buildDir}

	buildApiClient, err := NewBuildAPIClient(buildDir)
	if err != nil {
		return nil, err
	}

	manifests := map[string]any{
		// keep-sorted start ignore_prefixes="
		"api":                         &m.apis,
		"args":                        &m.args,
		"assembly_input_archives":     &m.assemblyInputArchives,
		"assembly_manifests":          &m.assemblyManifests,
		"binaries":                    &m.binaries,
		"checkout_artifacts":          &m.checkoutArtifacts,
		"generated_sources":           &m.generatedSources,
		imageManifestName:             &m.images,
		"licenses":                    &m.licenses,
		"package-repositories":        &m.packageRepositories,
		"platforms":                   &m.platforms,
		"prebuilt_binaries":           &m.prebuiltBinarySets,
		"prebuilt_versions_location":  &m.prebuiltVersionsLocation,
		"product_bundles":             &m.productBundles,
		"product_size_checker_output": &m.productSizeCheckerOutput,
		"rust_target_mapping":         &m.clippyTargets,
		"sdk_archives":                &m.sdkArchives,
		"tefmocheck_rules_manifest":   &m.tefmocheckRulesManifest,
		"test_durations":              &m.testDurations,
		"test_list_location":          &m.testListLocation,
		"tests":                       &m.testSpecs,
		"tool_paths":                  &m.tools,
		"triage_sources":              &m.triageSources,
		// keep-sorted end
	}
	// Ensure we read the manifests in order, so that if multiple manifests are
	// missing we always get the same error.
	manifestNames := maps.Keys(manifests)
	slices.Sort(manifestNames)
	for _, manifest := range manifestNames {
		if err := buildApiClient.GetJSON(manifest, manifests[manifest]); err != nil {
			return nil, err
		}
	}
	return m, nil
}

// BuildDir returns the fuchsia build directory root.
func (m Modules) BuildDir() string {
	return m.buildDir
}

// ImageManifest returns the path to the images manifest.
func (m Modules) ImageManifest() string {
	return filepath.Join(m.BuildDir(), imageManifestName+".json")
}

// APIs returns the build API module of available build API modules.
func (m Modules) APIs() []string {
	return m.apis
}

func (m Modules) ModulePaths() ([]string, error) {
	buildApiClient, err := NewBuildAPIClient(m.buildDir)
	if err != nil {
		return nil, err
	}
	return buildApiClient.GetModulePaths()
}

func (m Modules) Args() Args {
	return m.args
}

func (m Modules) AssemblyInputArchives() []AssemblyInputArchive {
	return m.assemblyInputArchives
}

func (m Modules) AssemblyManifests() []AssemblyManifest {
	return m.assemblyManifests
}

func (m Modules) Binaries() []Binary {
	return m.binaries
}

func (m Modules) CheckoutArtifacts() []CheckoutArtifact {
	return m.checkoutArtifacts
}

func (m Modules) ClippyTargets() []ClippyTarget {
	return m.clippyTargets
}

func (m Modules) GeneratedSources() []string {
	return m.generatedSources
}

func (m Modules) Images() []Image {
	return m.images
}

func (m Modules) Licenses() []License {
	return m.licenses
}

func (m Modules) PackageRepositories() []PackageRepo {
	return m.packageRepositories
}

// Platforms returns the build API module of available platforms to test on.
func (m Modules) Platforms() []DimensionSet {
	return m.platforms
}

// PrebuiltBinarySets returns the build API module of prebuilt packages
// registered in the build.
func (m Modules) PrebuiltBinarySets() []PrebuiltBinarySet {
	return m.prebuiltBinarySets
}

func (m Modules) PrebuiltVersionsLocation() PrebuiltVersionsLocation {
	return m.prebuiltVersionsLocation
}

func (m Modules) PrebuiltVersions() ([]PrebuiltVersion, error) {
	return LoadPrebuiltVersions(filepath.Join(m.BuildDir(), m.PrebuiltVersionsLocation().Location))
}

func (m Modules) ProductSizeCheckerOutput() []ProductSizeCheckerOutput {
	return m.productSizeCheckerOutput
}

func (m Modules) ProductBundles() []ProductBundle {
	return m.productBundles
}

func (m Modules) SDKArchives() []SDKArchive {
	return m.sdkArchives
}

func (m Modules) TefmocheckRulesManifest() []string {
	return m.tefmocheckRulesManifest
}

func (m Modules) TestDurations() []TestDuration {
	return m.testDurations
}

func (m Modules) TestListLocation() []string {
	return m.testListLocation
}

func (m Modules) TestSpecs() []TestSpec {
	return m.testSpecs
}

func (m Modules) Tools() Tools {
	return m.tools
}

func (m Modules) TriageSources() []string {
	return m.triageSources
}
