// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package testsharder

import (
	"encoding/json"
	"fmt"
	"io"
	"net/url"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"golang.org/x/exp/maps"
	"golang.org/x/exp/slices"

	"go.fuchsia.dev/fuchsia/tools/build"
	fintpb "go.fuchsia.dev/fuchsia/tools/integration/fint/proto"
	"go.fuchsia.dev/fuchsia/tools/integration/testsharder/metadata"
	"go.fuchsia.dev/fuchsia/tools/lib/jsonutil"
	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
)

const (
	// The name of the metadata directory within a package repository.
	metadataDirName = "repository"
	// The delivery blob config.
	deliveryBlobConfigName = "delivery_blob_config.json"
)

// Shard represents a set of tests with a common execution environment.
type Shard struct {
	// Name is the identifier for the shard.
	Name string `json:"name"`

	// Whether this shard can be skipped. If this shard has tests that should be
	// run, this will be false.
	Skippable bool `json:"skippable"`

	// The device type (or os for host tests) and target CPU architecture.
	EnvName string `json:"env_name"`

	// Tests is the set of tests to be executed in this shard.
	Tests []Test `json:"tests"`

	// Env is a generalized notion of the execution environment for the shard.
	Env build.Environment `json:"environment"`

	// The host CPU to run the shard on.
	HostCPU string `json:"host_cpu"`

	// Deps is the list of runtime dependencies required to be present on the host
	// at shard execution time. It is a list of paths relative to the fuchsia
	// build directory.
	Deps []string `json:"deps,omitempty"`

	// PkgRepo is the path to the shard-specific package repository. It is
	// relative to the fuchsia build directory, and is a directory itself.
	PkgRepo string `json:"pkg_repo,omitempty"`

	// TimeoutSecs is the execution timeout, in seconds, that should be set for
	// the task that runs the shard. It's computed dynamically based on the
	// expected runtime of the tests.
	TimeoutSecs int `json:"timeout_secs"`

	// TODO(b/450062147): remove this field once recipes no longer depend on it.
	// Summary is a TestSummary that is populated if the shard is skipped.
	Summary runtests.TestSummary `json:"summary,omitempty"`

	// Summary is a TestSummary that is populated in case this shard is skipped.
	SummaryIfSkipped runtests.TestSummary `json:"summary_if_skipped,omitempty"`

	// ProductBundle is the name of the product bundle describing the system
	// against which the test should be run.
	ProductBundle string `json:"product_bundle,omitempty"`

	// IsBootTest specifies whether the test is a boot test.
	IsBootTest bool `json:"is_boot_test,omitempty"`

	// BootupTimeoutSecs is the timeout in seconds that the provided product
	// bundle/environment is expected to take to boot up the target.
	BootupTimeoutSecs int `json:"bootup_timeout_secs,omitempty"`

	// ExpectsSSH specifies whether the test is expected to run against
	// a product bundle that supports SSH.
	ExpectsSSH bool `json:"expects_ssh,omitempty"`

	// CIPDPackages specifies the CIPD packages to install on the task that runs this shard.
	CIPDPackages []CIPDPackage `json:"cipd_packages,omitempty"`

	// BotDimensions specifies the swarming bot dimensions to use for this shard.
	BotDimensions map[string]string `json:"bot_dimensions,omitmepty"`

	// BotanistConfig is the path to the botanist config to pass to the swarming command.
	BotanistConfig string `json:"botanist_config,omitempty"`

	// The test manifest passed to botanist containing the tests to run.
	TestsJSON string `json:"tests_json,omitempty"`

	// The base swarming command to run for this shard.
	BaseCommand []string `json:"base_command,omitempty"`

	// The experiments from the testsharder params.
	ExperimentSpecs []string `json:"experiment_specs,omitempty"`

	// The actual experiments enabled for this shard based on the experiment percentage.
	// The same experiments should be enabled for every shard created from a testsharder
	// invocation.
	EnabledExperiments []string `json:"enabled_experiments,omitempty"`

	// The relative cwd for the swarming task. All paths should be relative to this
	// directory.
	RelativeCWD string `json:"relative_cwd,omitempty"`

	// The env prefixes to pass to the task request.
	EnvPrefixes map[string][]string `json:"env_prefixes,omitempty"`

	// BuildMetadata provides the fint set artifacts metadata needed to construct
	// swarming task requests from the shards. This will only be populated if the
	// `-deps-file` flag is provided meaning that local artifacts will be used and
	// thus the builder itself won't have the fint set artifacts available.
	BuildMetadata fintpb.SetArtifacts_Metadata `json:"build_metadata,omitempty"`
}

// CIPDPackage describes the CIPD package, version and subdir to download the package to
// within the working directory of the shard.
//
// This should only be used instead of regular deps for packages where the required platform
// differs from the platform of the locally checked-out package, and size constraints make it
// infeasible to include multiple platforms in the checkout.
//
// For example, x64 build hosts may launch arm64 emulator tests that require arm64 emulator
// prebuilts, but emulator prebuilts are large enough that including multiple versions of
// them in the checkout would slow down checkout times.
type CIPDPackage struct {
	// Name is the name of the package.
	Name string `json:"name"`

	// Version is the instance_id, ref, or unique tag that describes
	// the version of the package to use.
	Version string `json:"version"`

	// Subdir is the directory on the Swarming bot at which the package
	// should be installed.
	Subdir string `json:"subdir"`
}

// TargetCPU returns the CPU architecture of the target this shard will run against.
// Environments should be populated with the default cpu if missing before running MakeShards().
func (s *Shard) TargetCPU() string {
	return s.Env.Dimensions.CPU()
}

// HostCPU returns the host CPU architecture this shard will run on.
func GetHostCPU(env build.Environment, useTCG bool) string {
	// Determine what system architecture we want this test task to run on.
	// First, pure host tests should always run on the architecture of the
	// host test.  We can identify this case by the absence of a device_type
	// in the shard.
	if env.Dimensions.DeviceType() == "" {
		return env.Dimensions.CPU()
	}

	// Second, we use x64 controllers for all the physical hardware
	// that runs in labs, so those should always be x64.  We also use x64
	// cloud instances to drive testing against GCE, regardless of whether
	// the GCE instance is x64 or arm64.  So all non-emulator shards with
	// device_type targets set should run on x64.
	if !env.TargetsEmulator() {
		return "x64"
	}

	// For emulator shards, we want to take advantage of KVM where possible,
	// so we want to use the build's target arch, unless we're using TCG to
	// fully emulate hardware, in which case we'd rather that run on big x64
	// machines (since we don't have any riscv64 hardware capable of running
	// that workload).
	if useTCG {
		return "x64"
	}
	return env.Dimensions.CPU()
}

// CreatePackageRepo creates a package repository for the given shard.
func (s *Shard) CreatePackageRepo(buildDir string, globalRepoMetadata string, cacheTestPackages bool) error {
	globalRepoMetadata = filepath.Join(buildDir, globalRepoMetadata)

	// The path to the package repository should be unique so as to not
	// conflict with other shards' repositories.
	localRepoRel := fmt.Sprintf("repo_%s", url.PathEscape(s.Name))
	localRepo := filepath.Join(buildDir, localRepoRel)
	// Remove the localRepo if it exists in the incremental build cache.
	if err := os.RemoveAll(localRepo); err != nil {
		return err
	}

	// Copy over all repository metadata (encoded in JSON files).
	localRepoMetadata := filepath.Join(localRepo, metadataDirName)
	if err := os.MkdirAll(localRepoMetadata, os.ModePerm); err != nil {
		return err
	}
	entries, err := os.ReadDir(globalRepoMetadata)
	if err != nil {
		return err
	}
	for _, e := range entries {
		filename := e.Name()
		if filepath.Ext(filename) == ".json" {
			src := filepath.Join(globalRepoMetadata, filename)
			dst := filepath.Join(localRepoMetadata, filename)
			if err := os.Link(src, dst); err != nil {
				return err
			}
		}
	}
	// Add the blobs we expect the shard to access if the caller wants us to
	// set up a local package cache.
	if cacheTestPackages {
		pkgManifestsPerTest := make(map[string][]string)
		for _, t := range s.Tests {
			pkgManifests := t.PackageManifests
			if t.PackageManifestDepsFile != "" {
				var pkgManifestDeps []string
				if err := jsonutil.ReadFromFile(filepath.Join(buildDir, t.PackageManifestDepsFile), &pkgManifestDeps); err != nil {
					return err
				} else {
					pkgManifests = append(pkgManifests, pkgManifestDeps...)
				}
			}
			pkgManifestsPerTest[t.Name] = pkgManifests
		}

		// Use delivery blobs if the config exists.
		blobsDirRel, err := build.GetBlobsDir(filepath.Join(buildDir, deliveryBlobConfigName))
		if err != nil {
			return fmt.Errorf("failed to get blobs dir: %w", err)
		}
		blobsDir := filepath.Join(localRepoMetadata, blobsDirRel)
		addedBlobs := make(map[string]struct{})
		if err := os.MkdirAll(blobsDir, os.ModePerm); err != nil {
			return err
		}
		for testName, pkgManifests := range pkgManifestsPerTest {
			for _, p := range pkgManifests {
				if err := prepareBlobsForPackage(p, testName, addedBlobs, buildDir, globalRepoMetadata, localRepoMetadata, blobsDirRel); err != nil {
					return err
				}
			}
		}
	}

	s.PkgRepo = localRepoRel
	s.AddDeps([]string{localRepoRel})
	return nil
}

// prepareBlobsForPackage loads the given manifest path and ensures that all
// blobs it references, either directly or via subpackages, are copied or
// linked from globalRepoMetadata/blobsDirRel into blobsDir and enumerated in
// addedBlobs.
func prepareBlobsForPackage(
	manifestPath string,
	testName string,
	addedBlobs map[string]struct{},
	buildDir string,
	globalRepoMetadata string,
	localRepoMetadata string,
	blobsDirRel string,
) error {
	manifestAbsPath := manifestPath
	if !filepath.IsAbs(manifestAbsPath) {
		manifestAbsPath = filepath.Join(buildDir, manifestPath)
	}
	manifest, err := build.LoadPackageManifest(manifestAbsPath)
	if err != nil {
		return err
	}

	// Ensure all blobs directly referenced are added
	for _, blob := range manifest.Blobs {
		if _, exists := addedBlobs[blob.Merkle.String()]; !exists {
			// Use the blobs from the blobs dir instead of blob.SourcePath
			// since SourcePath only points to uncompressed blobs.
			src := filepath.Join(globalRepoMetadata, blobsDirRel, blob.Merkle.String())
			dst := filepath.Join(localRepoMetadata, blobsDirRel, blob.Merkle.String())
			if err := linkOrCopy(src, dst); err != nil {
				return fmt.Errorf("failed to copy blob %s from %s for %s: %w", blob.SourcePath, manifestPath, testName, err)
			}
			addedBlobs[blob.Merkle.String()] = struct{}{}
		}
	}

	// Walk all subpackages and ensure their blobs are added too.
	for _, subpackage := range manifest.Subpackages {
		if err := prepareBlobsForPackage(subpackage.ManifestPath, testName, addedBlobs, buildDir, globalRepoMetadata, localRepoMetadata, blobsDirRel); err != nil {
			return err
		}
	}

	return nil
}

// AddDeps adds a set of runtime dependencies to the shard. It ensures no
// duplicates and a stable ordering.
func (s *Shard) AddDeps(deps []string) {
	s.Deps = append(s.Deps, deps...)
	s.Deps = dedupe(s.Deps)
	sort.Strings(s.Deps)
}

func dedupe(l []string) []string {
	var deduped []string
	m := make(map[string]struct{})
	for _, s := range l {
		m[s] = struct{}{}
	}
	for s := range m {
		deduped = append(deduped, s)
	}
	return deduped
}

// ShardOptions parametrize sharding behavior.
type ShardOptions struct {
	// Tags is the list of tags that the sharded Environments must match; those
	// that don't match all tags will be ignored.
	Tags []string

	// Whether to use TCG with this shard if running against an emulator.
	UseTCG bool

	// The default CPU to apply to host or emulator envs.
	DefaultCPU string
}

// populateEnvDefaults returns an env with certain defaults populated.
// This helps shard environments with those values explicitly provided together
// with those that would have the same values using the defaults.
func populateEnvDefaults(env build.Environment, defaultCPU string, useTCG bool) build.Environment {
	dt := env.Dimensions.DeviceType()
	// The default CPU only applies to host and emulator tests.
	if dt == "" || env.TargetsEmulator() {
		if _, ok := env.Dimensions["cpu"]; !ok {
			env.Dimensions["cpu"] = defaultCPU
		}
	}
	// Accel mode only applies to emulators.
	if env.TargetsEmulator() && env.Emulator.Accel == "" {
		if useTCG {
			env.Emulator.Accel = build.AccelNone
		} else {
			env.Emulator.Accel = build.AccelHyper
		}
	}
	return env
}

// MakeShards returns the list of shards associated with a given build.
// A single output shard will contain only tests that have the same environment.
func MakeShards(specs []build.TestSpec, testListEntries map[string]build.TestListEntry, opts *ShardOptions, metadataMap map[string]metadata.TestMetadata) ([]*Shard, error) {
	// We don't want to crash if we've passed a nil testListEntries map.
	if testListEntries == nil {
		testListEntries = make(map[string]build.TestListEntry)
	}

	slices.Sort(opts.Tags)

	envs := make(map[string]build.Environment)
	envToSuites := make(map[string][]build.TestSpec)
	for _, spec := range specs {
		for _, e := range spec.Envs {
			e = populateEnvDefaults(e, opts.DefaultCPU, opts.UseTCG)
			// Tags should not differ by ordering.
			slices.Sort(e.Tags)
			if !slices.Equal(opts.Tags, e.Tags) {
				continue
			}

			key := environmentKey(e)
			envs[key] = e
			envToSuites[key] = append(envToSuites[key], spec)
		}
	}

	shards := []*Shard{}
	for envKey, e := range envs {
		deviceType := e.Dimensions.DeviceType()
		os := e.Dimensions.OS()
		if deviceType == "" && os == "" {
			return nil, fmt.Errorf("both the device_type and os fields should not be empty")
		}
		if opts.DefaultCPU == "" {
			return nil, fmt.Errorf("the DefaultCPU field (target_arch) in the ShardOptions should not be empty")
		}
		envName := makeEnvName(deviceType, os, opts.DefaultCPU)
		specs := envToSuites[envKey]

		sort.Slice(specs, func(i, j int) bool {
			return specs[i].Test.Name < specs[j].Test.Name
		})

		shardForProductBundle := make(map[string]*Shard)
		for _, spec := range specs {
			shard, ok := shardForProductBundle[spec.ProductBundle]
			if !ok {
				name := environmentName(e)
				if spec.ProductBundle != "" {
					name = fmt.Sprintf("%s-%s", name, spec.ProductBundle)
				}
				shard = &Shard{
					Name:              name,
					Tests:             []Test{},
					ProductBundle:     spec.ProductBundle,
					BootupTimeoutSecs: spec.BootupTimeoutSecs,
					ExpectsSSH:        spec.ExpectsSSH,
					Env:               e,
					HostCPU:           GetHostCPU(e, opts.UseTCG || e.Emulator.Accel == build.AccelNone),
					EnvName:           envName,
				}
			}
			test := Test{Test: spec.Test, Runs: 1}
			testListEntry, exists := testListEntries[spec.Test.Name]
			if exists {
				test.updateFromTestList(testListEntry)
			}
			testMetadata, exists := metadataMap[spec.Test.Name]
			if exists {
				test.Metadata = testMetadata
			}
			if spec.Test.Isolated || spec.IsBootTest {
				name := fmt.Sprintf("%s-%s", environmentName(e), normalizeTestName(spec.Test.Name))
				shards = append(shards, &Shard{
					Name:              name,
					Tests:             []Test{test},
					ProductBundle:     spec.ProductBundle,
					IsBootTest:        spec.IsBootTest,
					BootupTimeoutSecs: spec.BootupTimeoutSecs,
					ExpectsSSH:        spec.ExpectsSSH,
					Env:               e,
					HostCPU:           GetHostCPU(e, opts.UseTCG || e.Emulator.Accel == build.AccelNone),
					EnvName:           envName,
				})
			} else {
				shard.Tests = append(shard.Tests, test)
				shardForProductBundle[spec.ProductBundle] = shard
			}
		}
		for _, shard := range shardForProductBundle {
			if len(shard.Tests) > 0 {
				shards = append(shards, shard)
			}
		}
	}

	makeShardNamesUnique(shards)

	return shards, nil
}

func makeEnvName(deviceType string, os string, targetArch string) string {
	var envName string
	if deviceType != "" {
		envName = fmt.Sprintf("%s-%s", deviceType, targetArch)
	} else {
		envName = fmt.Sprintf("%s-%s", os, targetArch)
	}
	return envName
}

// makeShardNamesUnique updates `shards` in-place to ensure that no two shards
// have the same name, by grouping together shards with the same name and
// appending a suffix to the name of each duplicate-named shard using any
// environment dimensions that distinguish it from the others.
//
// This assumes that `Env.Dimensions` is always sufficient to distinguish two
// shards with the same name.
func makeShardNamesUnique(shards []*Shard) {
	type shardDims struct {
		shard *Shard
		dims  map[string]any
	}
	sameNameShards := make(map[string][]shardDims)
	for _, s := range shards {
		dims := make(map[string]any)
		for k, v := range s.Env.Dimensions {
			dims[k] = v
		}
		sameNameShards[s.Name] = append(sameNameShards[s.Name], shardDims{s, dims})
	}

	for _, dupes := range sameNameShards {
		if len(dupes) < 2 {
			continue
		}
		duplicateEnvs := []map[string]any{}
		for _, shard := range dupes {
			duplicateEnvs = append(duplicateEnvs, shard.dims)
		}
		common := commonDimensions(duplicateEnvs)
		for _, shard := range dupes {
			var tokens []string
			dims := maps.Keys(shard.dims)
			slices.Sort(dims)
			for _, dim := range dims {
				if _, ok := common[dim]; !ok {
					tokens = append(tokens, fmt.Sprintf("%s:%v", dim, shard.dims[dim]))
				}
			}
			if len(tokens) > 0 {
				shard.shard.Name += "-" + strings.Join(tokens, "-")
			}
		}
	}
}

// commonDimensions calculates the intersection of the environment dimensions of
// a set of shards.
func commonDimensions(dims []map[string]any) map[string]any {
	res := make(map[string]any)
	if len(dims) == 0 {
		return res
	}

	maps.Copy(res, dims[0])

	for i := 1; i < len(dims); i++ {
		maps.DeleteFunc(res, func(k string, v any) bool {
			v2, ok := dims[i][k]
			return !ok || v != v2
		})
	}
	return res
}

func environmentKey(env build.Environment) string {
	b, err := json.Marshal(env)
	if err != nil {
		panic(err)
	}
	return string(b)
}

// EnvironmentName returns a human-readable name for an environment.
func environmentName(env build.Environment) string {
	tokens := []string{}
	addToken := func(s string) {
		if s != "" {
			// s/-/_, so there is no ambiguity among the tokens
			// making up a name.
			s = strings.Replace(s, "-", "_", -1)
			tokens = append(tokens, s)
		}
	}

	addToken(env.Dimensions.DeviceType())
	addToken(env.Dimensions.OS())
	addToken(env.Dimensions.Testbed())
	addToken(env.Dimensions.Pool())
	addToken(env.Dimensions.HostDeviceType())
	if env.ServiceAccount != "" {
		addToken(strings.Split(env.ServiceAccount, "@")[0])
	}
	if env.Netboot {
		addToken("netboot")
	}
	if env.Emulator.Name != "" {
		addToken(env.Emulator.Name)
	}

	return strings.Join(tokens, "-")
}

func stringSlicesEq(s []string, t []string) bool {
	if len(s) != len(t) {
		return false
	}
	seen := make(map[string]int)
	for i := range s {
		seen[s[i]]++
		seen[t[i]]--
	}
	for _, v := range seen {
		if v != 0 {
			return false
		}
	}
	return true
}

// linkOrCopy hardlinks src to dst if src is not a symlink. If the source is a
// symlink, then it copies it. There are several blobs in the build directory
// that are symlinks to CIPD packages, and we don't want to include that
// symlink in the final package repository, so we copy instead.
func linkOrCopy(src string, dst string) error {
	info, err := os.Lstat(src)
	if err != nil {
		return err
	}
	if info.Mode()&os.ModeSymlink != os.ModeSymlink {
		return os.Link(src, dst)
	}
	s, err := os.Open(src)
	if err != nil {
		return err
	}
	defer s.Close()
	d, err := os.Create(dst)
	if err != nil {
		return err
	}
	defer d.Close()
	_, err = io.Copy(d, s)
	return err
}
