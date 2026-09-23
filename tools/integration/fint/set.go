// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package fint

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
	"strings"

	fintpb "go.fuchsia.dev/fuchsia/tools/integration/fint/proto"
	"go.fuchsia.dev/fuchsia/tools/lib/hostplatform"
	"go.fuchsia.dev/fuchsia/tools/lib/isatty"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
	"go.fuchsia.dev/fuchsia/tools/lib/osmisc"
	"go.fuchsia.dev/fuchsia/tools/lib/subprocess"
)

var (
	// Path within a checkout to script which will run a hermetic Python interpreter.
	vendoredPythonScriptPath = []string{"scripts", "fuchsia-vendored-python"}

	// Path within a checkout to script which will clobber a build when new fences appear.
	forceCleanScript = []string{"build", "force_clean", "force_clean_if_needed.py"}

	// Known build event services (for set artifacts).
	// GN args may name configurations like "sponge_infra", but as far as recipes
	// are concerned, they only need to distinguish between the name prefixes
	// "sponge" and "resultstore".
	validBuildEventServices = []string{"sponge", "resultstore"} // constant
)

const (
	CompilationModeGNArg    string = "compilation_mode"
	CompilationModeDebug    string = "debug"
	CompilationModeRelease  string = "release"
	CompilationModeBalanced string = "balanced"
)

// canonicalizeBuildEventService translates a GN configuration name for
// build event services into a canonical name to be used in recipes.
// For example, "resultstore_infra" maps to "resultstore".
func canonicalizeBuildEventService(configName string) string {
	for _, s := range validBuildEventServices {
		if strings.HasPrefix(configName, s) {
			return s
		}
	}
	return configName
}

// Set runs `gn gen` given a static and context spec. It's intended to be
// consumed as a library function.
func Set(
	ctx context.Context,
	staticSpec *fintpb.Static,
	contextSpec *fintpb.Context,
	skipLocalArgs bool,
	assemblyOverridesStrings []string,
) (*fintpb.SetArtifacts, error) {
	platform, err := hostplatform.Name()
	if err != nil {
		return nil, err
	}

	// TODO move to setImpl, add unit tests
	if err := forceCleanIfNeeded(ctx, contextSpec, platform); err != nil {
		return nil, err
	}

	artifacts, err := setImpl(ctx, newRunner(contextSpec), staticSpec, contextSpec, platform, skipLocalArgs, assemblyOverridesStrings)
	if err != nil && artifacts != nil && artifacts.FailureSummary == "" {
		// Fall back to using the error text as the failure summary if the
		// failure summary is unset. It's better than failing without emitting
		// any information.
		artifacts.FailureSummary = err.Error()
	}
	return artifacts, err
}

// setImpl runs `gn gen` along with any post-processing steps, and returns a
// SetArtifacts object containing metadata produced by GN and post-processing.
func setImpl(
	ctx context.Context,
	runner subprocessRunner,
	staticSpec *fintpb.Static,
	contextSpec *fintpb.Context,
	platform string,
	skipLocalArgs bool,
	assemblyOverridesStrings []string,
) (*fintpb.SetArtifacts, error) {
	if contextSpec.CheckoutDir == "" {
		return nil, fmt.Errorf("checkout_dir must be set")
	}
	if contextSpec.BuildDir == "" {
		return nil, fmt.Errorf("build_dir must be set")
	}

	genArgs, err := genArgs(ctx, staticSpec, contextSpec, skipLocalArgs, assemblyOverridesStrings)
	if err != nil {
		return nil, err
	}

	artifacts := &fintpb.SetArtifacts{
		Metadata: &fintpb.SetArtifacts_Metadata{
			Board: staticSpec.Board,
			CompilationMode: strings.ToLower(
				strings.TrimPrefix(staticSpec.CompilationMode.String(), "COMPILATION_MODE_")),
			Product:    staticSpec.Product,
			TargetArch: strings.ToLower(staticSpec.TargetArch.String()),
			Variants:   staticSpec.Variants,
		},
		BuildEventService:      canonicalizeBuildEventService(staticSpec.BuildEventService),
		BuildEventServiceNinja: canonicalizeBuildEventService(staticSpec.BuildEventServiceNinja),
		// True if any toolchain is using RBE and needs reproxy to run.
		// Note: bazel+RBE doesn't require reproxy.
		EnableRbe: staticSpec.RustRbeEnable || staticSpec.CxxRbeEnable || staticSpec.LinkRbeEnable,
	}

	if contextSpec.ArtifactDir != "" {
		artifacts.GnTracePath = filepath.Join(contextSpec.ArtifactDir, "gn_trace.json")
	}
	genStdout, err := runGen(ctx, runner, staticSpec, contextSpec, platform, artifacts.GnTracePath, genArgs)
	if err != nil {
		artifacts.FailureSummary = genStdout
		return artifacts, err
	}

	// Only run build graph analysis if the result will be emitted via
	// artifacts, and if we actually care about checking the result.
	if contextSpec.ArtifactDir != "" && staticSpec.SkipIfUnaffected {
		var changedFiles []string
		for _, f := range contextSpec.ChangedFiles {
			changedFiles = append(changedFiles, f.Path)
		}
		sb, err := shouldBuild(ctx, runner, contextSpec.BuildDir, contextSpec.CheckoutDir, platform, changedFiles)
		if err != nil {
			return artifacts, err
		}
		artifacts.SkipBuild = !sb
	}
	return artifacts, err
}

// forceCleanIfNeeded clobbers the build dir if new clean build fences are found, see
// //build/force_clean/README.md for details.
func forceCleanIfNeeded(ctx context.Context, contextSpec *fintpb.Context, platform string) (err error) {
	if _, err := os.Stat(contextSpec.BuildDir); os.IsNotExist(err) {
		// no need to clean anything if there's nothing there
		return nil
	}
	scriptRunner := newRunner(contextSpec)
	scriptRunner.Dir = contextSpec.CheckoutDir
	return scriptRunner.Run(ctx, []string{
		filepath.Join(append([]string{contextSpec.CheckoutDir}, vendoredPythonScriptPath...)...),
		filepath.Join(append([]string{contextSpec.CheckoutDir}, forceCleanScript...)...),
		"--gn-bin",
		thirdPartyPrebuilt(contextSpec.CheckoutDir, platform, "gn"),
		"--checkout-dir",
		contextSpec.CheckoutDir,
		"--build-dir",
		contextSpec.BuildDir,
	}, subprocess.RunOptions{})
}

func runGen(
	ctx context.Context,
	runner subprocessRunner,
	staticSpec *fintpb.Static,
	contextSpec *fintpb.Context,
	platform string,
	gnTracePath string,
	args []string,
) (genStdout string, err error) {
	gn := thirdPartyPrebuilt(contextSpec.CheckoutDir, platform, "gn")

	formattedArgs := gnFormat(ctx, gn, runner, args)
	logger.Infof(ctx, "GN args:\n%s\n", formattedArgs)

	// gn will return an error if the argument list is too long, so write the
	// args directly to the build dir instead of using the --args flag.
	if f, err := osmisc.CreateFile(filepath.Join(contextSpec.BuildDir, "args.gn")); err != nil {
		return "", fmt.Errorf("failed to create args.gn: %w", err)
	} else if _, err := io.WriteString(f, formattedArgs); err != nil {
		return "", fmt.Errorf("failed to write args.gn: %w", err)
	}

	regenCmd := []string{
		filepath.Join(contextSpec.CheckoutDir, "build", "regenerator"),
		"--fuchsia-dir",
		contextSpec.CheckoutDir,
		"--fuchsia-build-dir",
		contextSpec.BuildDir,
	}
	if isatty.IsTerminal() {
		regenCmd = append(regenCmd, "--color")
	}
	if gnTracePath != "" {
		regenCmd = append(regenCmd, fmt.Sprintf("--gn-tracelog=%s", gnTracePath))
	}
	if !staticSpec.ExportRustProject {
		regenCmd = append(regenCmd, "--no-export-rust-project")
	}
	for _, f := range staticSpec.IdeFiles {
		regenCmd = append(regenCmd, fmt.Sprintf("--gn-ide=%s", f))
	}
	for _, s := range staticSpec.JsonIdeScripts {
		regenCmd = append(regenCmd, fmt.Sprintf("--gn-json-ide-script=%s", s))
	}

	// When `gn gen` fails, it outputs a brief helpful error message to stdout.
	var stdoutBuf bytes.Buffer
	if err := runner.Run(ctx, regenCmd, subprocess.RunOptions{Stdout: io.MultiWriter(&stdoutBuf, os.Stdout)}); err != nil {
		return stdoutBuf.String(), fmt.Errorf("error running %q: %w", regenCmd[0], err)
	}
	return stdoutBuf.String(), nil
}

func genArgs(
	ctx context.Context,
	staticSpec *fintpb.Static,
	contextSpec *fintpb.Context,
	skipLocalArgs bool,
	assemblyOverridesStrings []string,
) ([]string, error) {
	// GN variables to set via args (mapping from variable name to value).
	vars := make(map[string]any)
	// GN list variables that could historically be set by products, and should go
	// in their own block in args.gn.  Append or assign is controlled when these
	// are formatted as lines.
	targetLists := make(map[string][]string)
	// GN targets to import.
	var imports []string

	if staticSpec.TargetArch == fintpb.Static_ARCH_UNSPECIFIED {
		// Board files declare `target_cpu` so it's not necessary to set
		// `target_cpu` as long as we have a board file.
		if staticSpec.Board == "" {
			return nil, fmt.Errorf("target_arch must be set if board is not")
		}
	} else {
		vars["target_cpu"] = strings.ToLower(staticSpec.TargetArch.String())
	}

	if contextSpec.ClangToolchainDir != "" {
		vars["clang_prefix"] = filepath.Join(contextSpec.ClangToolchainDir, "bin")
	}
	if contextSpec.GccToolchainDir != "" {
		vars["gcc_tool_dir"] = filepath.Join(contextSpec.GccToolchainDir, "bin")
	}
	if contextSpec.RustToolchainDir != "" {
		vars["rustc_prefix"] = filepath.Join(contextSpec.RustToolchainDir)
	}

	// Only set these GN variables if they are != the default value
	// (which is false).
	if staticSpec.RustRbeEnable {
		vars["rust_rbe_enable"] = staticSpec.RustRbeEnable
	}
	if staticSpec.CxxRbeEnable {
		vars["cxx_rbe_enable"] = staticSpec.CxxRbeEnable
	}
	if staticSpec.LinkRbeEnable {
		vars["link_rbe_enable"] = staticSpec.LinkRbeEnable
	}
	if staticSpec.BazelRbeEnable {
		vars["enable_bazel_remote_rbe"] = staticSpec.BazelRbeEnable
	}
	if staticSpec.DisableXattrForRbe {
		vars["disable_xattr_for_rbe"] = staticSpec.DisableXattrForRbe
	}

	if staticSpec.BuildEventService != "" {
		vars["bazel_upload_build_events"] = staticSpec.BuildEventService
	}
	if staticSpec.BuildEventServiceNinja != "" {
		vars["ninja_upload_build_events"] = staticSpec.BuildEventServiceNinja
	}

	if staticSpec.Product != "" {
		basename := filepath.Base(staticSpec.Product)
		vars["build_info_product"] = strings.Split(basename, ".")[0]
		imports = append(imports, staticSpec.Product)
	}

	if staticSpec.Board != "" {
		basename := filepath.Base(staticSpec.Board)
		vars["build_info_board"] = strings.Split(basename, ".")[0]
		imports = append(imports, staticSpec.Board)
	}

	if staticSpec.MainPbLabel != "" {
		vars["main_pb_label"] = staticSpec.MainPbLabel
	}

	if contextSpec.SdkId != "" {
		vars["sdk_id"] = contextSpec.SdkId
	}

	if contextSpec.ReleaseVersion != "" {
		vars["build_info_version"] = contextSpec.ReleaseVersion
	}

	if staticSpec.TestDurationsFile != "" {
		testDurationsFile := staticSpec.TestDurationsFile
		exists, err := osmisc.FileExists(filepath.Join(contextSpec.CheckoutDir, testDurationsFile))
		if err != nil {
			return nil, fmt.Errorf("failed to check if TestDurationsFile exists: %w", err)
		}
		if !exists {
			testDurationsFile = staticSpec.DefaultTestDurationsFile
		}
		vars["test_durations_file"] = testDurationsFile
	}

	// The following spec vars are all combined into a single set of target labels:
	var targetLabels []string
	targetLabels = append(targetLabels, staticSpec.TargetLabels...)
	targetLabels = append(targetLabels, staticSpec.UniversePackages...)
	targetLabels = append(targetLabels, staticSpec.HermeticTestPackages...)
	targetLabels = append(targetLabels, staticSpec.TestPackages...)
	targetLabels = append(targetLabels, staticSpec.E2ETestLabels...)
	targetLabels = append(targetLabels, staticSpec.DeveloperTestLabels...)

	for varName, values := range map[string][]string{
		"host_labels":       append(staticSpec.HostLabels, staticSpec.HostTestLabels...),
		"target_labels":     targetLabels,
		"build_only_labels": staticSpec.BuildOnlyLabels,
	} {
		targetLists[varName] = values
	}

	if len(staticSpec.Variants) != 0 {
		vars["select_variant"] = staticSpec.Variants
	}
	if contextSpec.CollectCoverage && len(contextSpec.ChangedFiles) > 0 {
		var profileSourceFiles []string
		for _, file := range contextSpec.ChangedFiles {
			profileSourceFiles = append(profileSourceFiles, fmt.Sprintf("//%s", file.Path))
		}
		// Profile changed files, and only changed files.
		vars["profile_source_files"] = profileSourceFiles
		vars["dont_profile_source_files"] = []string{}
	}

	if contextSpec.PgoProfilePath != "" {
		vars["pgo_profile_path"] = filepath.Join(contextSpec.PgoProfilePath)
	}

	if staticSpec.UseTemporaryGoCache {
		// We wish to have the go cache directory be deterministic,
		// because the cache directory winds up in various ninja action
		// commandlines, so having the cache directory change between
		// builds means that those action's outputs are dirtied, and we
		// re-run actions on incremental builds that differ only in go
		// cache directory.
		// However, we do still wish to preserve the invariant that the
		// cache directory is empty when UseTemporaryGoCache is
		// requested.  Thus, instead of generating a random directory
		// with os.TempDir(), we generate a predictable, deterministic
		// path to serve as the go cache directory, and ensure that it
		// is empty at the time of `fint set`, which assuming no racing
		// work is occurring on the same machine (which is a safe
		// assumption in our build infra), is semantically equivalent
		// while allowing better build caching.
		dir := filepath.Join(os.TempDir(), "fuchsia_go_cache")
		if err := os.RemoveAll(dir); err != nil {
			return nil, err
		}
		if err := os.MkdirAll(dir, 0o700); err != nil {
			return nil, err
		}
		vars["gocache_dir"] = dir
	}

	var importArgs, varArgs, targetListArgs, localArgs, overridesArgs, compileArgs []string

	// Add comments to make args.gn more readable.
	compileArgs = append(compileArgs, "\n\n# Compilation args:")
	varArgs = append(varArgs, "\n\n# Basic args:")
	targetListArgs = append(targetListArgs, "\n\n# Target lists:")

	// compilation args are set in a single block
	if staticSpec.CompilationMode == fintpb.Static_COMPILATION_MODE_UNSPECIFIED {
		return nil, fmt.Errorf("compilation_mode is unspecified or invalid")
	}
	var compilationMode string
	switch staticSpec.CompilationMode {
	case fintpb.Static_COMPILATION_MODE_DEBUG:
		compilationMode = CompilationModeDebug
	case fintpb.Static_COMPILATION_MODE_BALANCED:
		compilationMode = CompilationModeBalanced
	case fintpb.Static_COMPILATION_MODE_RELEASE:
		compilationMode = CompilationModeRelease
	default:
		return nil, fmt.Errorf("unknown compilation_mode value: %s", staticSpec.CompilationMode.String())
	}
	compileArgs = appendGNArg(compileArgs, CompilationModeGNArg, compilationMode)

	// other vars are directly set in the "basic args" block
	for k, v := range vars {
		varArgs = appendGNArg(varArgs, k, v)
	}

	for _, arg := range staticSpec.GnArgs {
		if strings.HasPrefix(arg, "import(") {
			importArgs = append(importArgs, arg)
		} else if strings.Contains(arg, "+=") {
			// any list-append operation is almost always trying to change the target
			// lists set by a product or board, so add it to that block:
			targetListArgs = append(targetListArgs, arg)
		} else {
			// any directly-assigned args are more likely general build arguments.
			varArgs = append(varArgs, arg)
		}
	}
	sort.Strings(varArgs)

	for k, v := range targetLists {
		targetListArgs = appendGNArg(targetListArgs, k, v)
	}
	sort.Strings(targetListArgs)

	for _, p := range imports {
		importArgs = append(importArgs, fmt.Sprintf(`import("//%s")`, p))
	}
	sort.Strings(importArgs)

	if !skipLocalArgs {
		localArgsPath := filepath.Join(contextSpec.CheckoutDir, "local/args.gn")
		localArgsBytes, err := os.ReadFile(localArgsPath)
		localArgsContents := string(localArgsBytes)
		if err == nil {
			logger.Infof(ctx, "Including local args from %s.", localArgsPath)
			localArgs = append(localArgs, fmt.Sprintf("\n\n# Local args from %s:", localArgsPath))
			localArgs = append(localArgs, localArgsContents)
		} else if errors.Is(err, os.ErrNotExist) {
			logger.Infof(ctx, "No local args available.")
		} else {
			return nil, err
		}
	}

	assemblyOverrides := make(map[string]string)
	// Since map iteration order isn't stable, a separate list is needed to keep a consistent ordering
	// of the entries (to match user expectations and to allow tests to properly validate which
	// override target is used by each assembly target).
	var assemblyTargetOrder []string
	var mainTargetOverridesLabel string
	for _, overrideString := range assemblyOverridesStrings {
		pair := strings.Split(overrideString, "=")
		if len(pair) == 1 {
			if mainTargetOverridesLabel != "" {
				return nil, fmt.Errorf("Only one overrides target can be set for the main assembly.  Found '%s' and '%s'.", mainTargetOverridesLabel, overrideString)
			}
			mainTargetOverridesLabel = overrideString
		} else if len(pair) != 2 {
			return nil, fmt.Errorf("assembly overrides must be a single OVERRIDE_TARGET label or in ASSEMBLY_TARGET=OVERRIDE_TARGET pairs")
		} else {
			_, duplicate := assemblyOverrides[pair[0]]
			if duplicate {
				return nil, fmt.Errorf("Only one override target can be provided for each assembly target: %s", pair[0])
			}
			assemblyOverrides[pair[0]] = pair[1]
			assemblyTargetOrder = append(assemblyTargetOrder, pair[0])
		}
	}
	if mainTargetOverridesLabel != "" {
		overridesArgs = append(overridesArgs, fmt.Sprintf("\n\nproduct_assembly_overrides_label = %s", toGNValue(mainTargetOverridesLabel)))
	}
	overridesArgs = append(overridesArgs, "\n\nproduct_assembly_overrides = [")
	for _, assembly_target := range assemblyTargetOrder {
		override_target := assemblyOverrides[assembly_target]
		overridesArgs = append(overridesArgs, "{")
		overridesArgs = append(overridesArgs, fmt.Sprintf("assembly = %s", toGNValue(assembly_target)))
		overridesArgs = append(overridesArgs, fmt.Sprintf("overrides = %s", toGNValue(override_target)))
		overridesArgs = append(overridesArgs, "},")
	}
	overridesArgs = append(overridesArgs, "]")

	// Ensure that imports come before args that set or modify variables, as
	// otherwise the imported files might blindly redefine variables set or
	// modified by other arguments.
	var finalArgs []string
	finalArgs = append(finalArgs, importArgs...)
	finalArgs = append(finalArgs, compileArgs...)
	finalArgs = append(finalArgs, varArgs...)
	finalArgs = append(finalArgs, targetListArgs...)
	finalArgs = append(finalArgs, overridesArgs...)
	finalArgs = append(finalArgs, localArgs...)
	finalArgs = append(finalArgs, "\n")
	return finalArgs, nil
}

// gnFormat makes a best-effort attempt to format the input arguments using `gn
// format` so that the output will be more readable in case of any errors like
// unknown variables. If formatting fails (e.g. due to a syntax error) we'll
// just return the unformatted args and let `gn gen` return an error; otherwise
// we'd need duplicated error handling code to handle both syntax errors from
// `gn format` and non-syntax errors from `gn gen`.
func gnFormat(ctx context.Context, gn string, runner subprocessRunner, args []string) string {
	unformatted := strings.Join(args, "\n")
	var output bytes.Buffer
	opts := subprocess.RunOptions{
		Stdout: &output,
		Stderr: io.Discard,
		Stdin:  strings.NewReader(unformatted),
	}
	if err := runner.Run(ctx, []string{gn, "format", "--stdin"}, opts); err != nil {
		return unformatted
	}
	return output.String()
}

// toGNValue converts a Go value to a string representation of the corresponding
// GN value by inspecting the Go value's type. This makes the logic to set GN
// args more readable and less error-prone, with no need for patterns like
// fmt.Sprintf(`"%s"`) repeated everywhere.
//
// For example:
// - toGNValue(true) => `true`
// - toGNValue("foo") => `"foo"` (a string containing literal double-quotes)
// - toGNValue([]string{"foo", "bar"}) => `["foo","bar"]`
func toGNValue(x any) string {
	switch val := x.(type) {
	case bool:
		return fmt.Sprintf("%v", val)
	case string:
		// Apply double-quotes to strings, but not to GN scopes like
		// {variant="asan-fuzzer" target_type=["fuzzed_executable"]}
		if strings.HasPrefix(val, "{") && strings.HasSuffix(val, "}") {
			return val
		}
		return fmt.Sprintf(`"%s"`, val)
	case []string:
		var values []string
		for _, element := range val {
			values = append(values, toGNValue(element))
		}
		return fmt.Sprintf("[%s]", strings.Join(values, ","))
	default:
		panic(fmt.Sprintf("unsupported arg value type %T", val))
	}
}

// appendGNArg appends an argument to a list of GN arguments, which is one per
// line.  The value is converted from a Go value to a string representation
// using toGNValue.
func appendGNArg(varlist []string, name string, value any) []string {
	return append(varlist, fmt.Sprintf("%s=%s", name, toGNValue(value)))
}
