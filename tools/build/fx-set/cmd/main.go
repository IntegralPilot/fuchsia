// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"syscall"

	flag "github.com/spf13/pflag"

	"encoding/json"

	"go.fuchsia.dev/fuchsia/tools/integration/fint"
	fintpb "go.fuchsia.dev/fuchsia/tools/integration/fint/proto"
	"go.fuchsia.dev/fuchsia/tools/lib/color"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
	"go.fuchsia.dev/fuchsia/tools/lib/osmisc"
	"go.fuchsia.dev/fuchsia/tools/lib/subprocess"
)

const (
	// Optional env var set by the user, pointing to the directory in which
	// ccache artifacts should be cached between builds.
	ccacheDirEnvVar = "CCACHE_DIR"

	// fx ensures that this env var is set.
	checkoutDirEnvVar = "FUCHSIA_DIR"

	// Populated when fx's top-level `--dir` flag is set.
	buildDirEnvVar = "_FX_BUILD_DIR"

	// out/default was the historical "default" build directory for Fuchsia.
	// Many manual tests have hard-coded it. We preserve it as a symlink to the
	// actual build directory, and now create directories by board to preserve
	// local build artifacts between arch changes.
	symlinkBuildDir = "out/default"

	// When unspecified, this is used for --rbe-mode.
	defaultRbeMode = "auto"

	// When unspecified, this is used for --compilation-mode.
	defaultCompilationMode = "balanced"

	productBundlesFile = "product_bundles.json"
)

type productBundle struct {
	Name  string `json:"name"`
	Label string `json:"label"`
}

type subprocessRunner interface {
	Run(ctx context.Context, cmd []string, options subprocess.RunOptions) error
}

// fxRunner is a utility for running fx commands as subprocesses.
type fxRunner struct {
	sr          subprocessRunner
	checkoutDir string
}

func (r *fxRunner) constructCommand(command string, args []string) []string {
	fxPath := filepath.Join(r.checkoutDir, "scripts", "fx-reentry")
	cmd := []string{fxPath, command}
	return append(cmd, args...)
}

// run runs the given fx command with optional args.
func (r *fxRunner) run(ctx context.Context, command string, args ...string) error {
	return r.sr.Run(ctx, r.constructCommand(command, args), subprocess.RunOptions{
		// Subcommands may run interactive logins, so give them access to stdin by default.
		Stdin: os.Stdin,
	})
}

// runWithNoStdio is the same as run, but discards any stdout and stderr and
// doesn't forward stdin to the subprocess.
func (r *fxRunner) runWithNoStdio(ctx context.Context, command string, args ...string) error {
	return r.sr.Run(ctx, r.constructCommand(command, args), subprocess.RunOptions{
		Stdout: io.Discard, Stderr: io.Discard,
	})
}

func main() {
	l := logger.NewLogger(logger.ErrorLevel, color.NewColor(color.ColorAuto), os.Stdout, os.Stderr, "")
	// Don't include timestamps or other metadata in logs, since this tool is
	// only intended to be run on developer workstations.
	l.SetFlags(0)
	ctx := logger.WithLogger(context.Background(), l)
	ctx, cancel := signal.NotifyContext(ctx, syscall.SIGTERM, syscall.SIGINT)
	defer cancel()

	if err := mainImpl(ctx); err != nil {
		if ctx.Err() == nil {
			logger.Errorf(ctx, err.Error())
		}
		os.Exit(1)
	}
}

func mainImpl(ctx context.Context) error {
	args, err := parseArgsAndEnv(os.Args[1:], allEnvVars())
	if err != nil {
		return err
	}

	if args.verbose {
		if l := logger.LoggerFromContext(ctx); l != nil {
			l.LoggerLevel = logger.DebugLevel
		}
	}

	fx := fxRunner{
		sr:          &subprocess.Runner{},
		checkoutDir: args.checkoutDir,
	}

	var staticSpec *fintpb.Static
	canUseRbe, err := canAccessRbe(ctx, args.checkoutDir)
	if err != nil {
		fmt.Println("Unable to determine RBE access, assuming False.")
		canUseRbe = false
	}

	originalMainPb := args.mainPbLabel
	isMainPbName := originalMainPb != "" && !strings.HasPrefix(originalMainPb, "//")
	if isMainPbName {
		// Clear MainPbLabel for the first run if it's a name, it will be resolved after gn gen.
		args.mainPbLabel = ""
	}

	if args.fintParamsPath == "" {
		staticSpec, err = constructStaticSpec(args.checkoutDir, args, canUseRbe)
		if err != nil {
			return err
		}
	} else {
		path := args.fintParamsPath
		if !filepath.IsAbs(path) {
			path = filepath.Join(args.checkoutDir, path)
		}
		staticSpec, err = fint.ReadStatic(path)
		if err != nil {
			return err
		}
		staticSpec.GnArgs = append(staticSpec.GnArgs, args.gnArgs...)
		staticSpec, err = applyRbeSettings(staticSpec, args, canUseRbe)
		if err != nil {
			return err
		}
	}

	if args.checkoutDir == "" {
		return fmt.Errorf("got unexpected empty checkoutDir")
	}
	if args.buildDir == "" {
		return fmt.Errorf("got unexpected empty buildDir")
	}

	contextSpec := &fintpb.Context{
		CheckoutDir: args.checkoutDir,
		BuildDir:    filepath.Join(args.checkoutDir, args.buildDir),
	}

	if args.noChangeEnv {
		// Set an environment variable so downstream tools like build/regenerator.py
		// know to avoid natively overwriting global symlinks across the workspace.
		if err := os.Setenv("FX_NO_ENV_SYMLINK", "1"); err != nil {
			return fmt.Errorf("failed to set FX_NO_ENV_SYMLINK: %w", err)
		}
	}

	if !isMainPbName {
		// For the "normal" case where --main-pb is a label (or not set), we can run fint.Set once.
		_, err = fint.Set(ctx, staticSpec, contextSpec, args.skipLocalArgs, args.assemblyOverrideStrings)
		if err != nil {
			return err
		}
	} else {
		// if the --main-pb is specified as a name, we need to run GN twice, with slightly different
		// arguments.

		// First, run without any overrides so that we can generate the list of product bundles.
		_, err = fint.Set(ctx, staticSpec, contextSpec, args.skipLocalArgs, []string{})
		if err != nil {
			return err
		}

		// Resolve the product bundle name now that we can do so.
		label, err := resolveProductBundleName(args.checkoutDir, args.buildDir, originalMainPb)
		if err != nil {
			return err
		}
		staticSpec.MainPbLabel = label

		// Re-run fint.Set to update args.gn with the correct main_pb_label, and now pass the assembly
		// overrides if they were provided.
		_, err = fint.Set(ctx, staticSpec, contextSpec, args.skipLocalArgs, args.assemblyOverrideStrings)
		if err != nil {
			return err
		}

		fmt.Printf("\n[Tip]: Pass the full label (e.g. `--main-pb %s`) to skip the extra gn gen pass, and speed up `fx set`.\n", label)
	}

	// Set the build dir used by subsequent fx commands.
	buildDir := contextSpec.BuildDir
	if relBuildDir, err := filepath.Rel(contextSpec.CheckoutDir, contextSpec.BuildDir); err == nil {
		buildDir = relBuildDir
	}
	if !args.noChangeEnv {
		if err := fx.run(ctx, "use", buildDir); err != nil {
			return fmt.Errorf("failed to set build directory: %w", err)
		}
	}
	return nil
}

type setArgs struct {
	verbose        bool
	fintParamsPath string

	checkoutDir   string
	buildDir      string
	skipLocalArgs bool
	noChangeEnv   bool

	// Flags passed to GN.
	board     string
	product   string
	useCcache bool
	noCcache  bool
	ccacheDir string

	// rbeMode selects a preset of RBE configurations.
	// see build/toolchain/rbe_modes.gni.
	rbeMode string

	enableRustRbe bool

	enableLinkRbe  bool
	enableBazelRbe bool

	enableCxxRbe  bool
	disableCxxRbe bool

	buildEventService string

	mainPbLabel string

	includeClippy bool

	compilationMode string
	netboot         bool
	cargoTOMLGen    bool
	jsonIDEScripts  []string
	targetLabels    []string
	hostLabels      []string
	testLabels      []string
	variants        []string
	fuzzSanitizers  []string
	ideFiles        []string
	gnArgs          []string

	assemblyOverrideStrings []string
}

func parseArgsAndEnv(args []string, env map[string]string) (*setArgs, error) {
	cmd := &setArgs{}

	cmd.checkoutDir = env[checkoutDirEnvVar]
	if cmd.checkoutDir == "" {
		return nil, fmt.Errorf("%s env var must be set", checkoutDirEnvVar)
	}
	cmd.ccacheDir = env[ccacheDirEnvVar] // Not required.

	cmd.buildDir = env[buildDirEnvVar] // Not required.

	flagSet := flag.NewFlagSet("fx set", flag.ExitOnError)
	// TODO(olivernewman): Decide whether to have this tool print usage or
	// to let //tools/devshell/set handle usage.
	flagSet.Usage = func() {}
	// We log a final error to stderr, so no need to have pflag print
	// intermediate errors.
	flagSet.SetOutput(io.Discard)

	flagSet.BoolVar(&cmd.skipLocalArgs, "skip-local-args", false, "")
	flagSet.BoolVar(&cmd.noChangeEnv, "no-change-env", false, "Do not configure global .fx-build-dir or symlink")
	flagSet.StringVar(&cmd.buildDir, "dir", cmd.buildDir, "")

	var autoDir = true // default to automatically creating a named build directory

	// Help strings don't matter because `fx set -h` uses the help text from
	// //tools/devshell/set, which should be kept up to date with these flags.
	flagSet.BoolVar(&cmd.verbose, "verbose", false, "")
	flagSet.StringVar(&cmd.fintParamsPath, "fint-params-path", "", "")
	flagSet.BoolVar(&cmd.useCcache, "ccache", false, "")
	flagSet.BoolVar(&cmd.noCcache, "no-ccache", false, "")
	flagSet.BoolVar(&cmd.includeClippy, "include-clippy", true, "")

	flagSet.StringVar(&cmd.rbeMode, "rbe-mode", defaultRbeMode, "")
	flagSet.BoolVar(&cmd.enableRustRbe, "rust-rbe", false, "")
	flagSet.BoolVar(&cmd.enableCxxRbe, "cxx-rbe", false, "")
	flagSet.BoolVar(&cmd.disableCxxRbe, "no-cxx-rbe", false, "")
	flagSet.BoolVar(&cmd.enableLinkRbe, "link-rbe", false, "")
	flagSet.BoolVar(&cmd.enableBazelRbe, "bazel-rbe", false, "")

	flagSet.StringVar(&cmd.buildEventService, "bes", "", "")

	flagSet.StringVar(&cmd.mainPbLabel, "main-pb", "", "")

	flagSet.StringVar(&cmd.compilationMode, "compilation-mode", defaultCompilationMode, "")

	flagSet.StringVar(&cmd.compilationMode, "release", defaultCompilationMode, "")
	flagSet.Lookup("release").NoOptDefVal = "release"

	flagSet.StringVar(&cmd.compilationMode, "debug", defaultCompilationMode, "")
	flagSet.Lookup("debug").NoOptDefVal = "debug"

	flagSet.StringVar(&cmd.compilationMode, "balanced", defaultCompilationMode, "")
	flagSet.Lookup("balanced").NoOptDefVal = "balanced"
	flagSet.BoolVar(&cmd.cargoTOMLGen, "cargo-toml-gen", false, "")
	flagSet.StringSliceVar(&cmd.jsonIDEScripts, "json-ide-script", []string{}, "")
	flagSet.StringSliceVar(&cmd.targetLabels, "with", []string{}, "")
	flagSet.StringSliceVar(&cmd.hostLabels, "with-host", []string{}, "")
	flagSet.StringSliceVar(&cmd.testLabels, "with-test", []string{}, "")
	flagSet.StringSliceVar(&cmd.variants, "variant", []string{}, "")
	flagSet.StringSliceVar(&cmd.fuzzSanitizers, "fuzz-with", []string{}, "")
	flagSet.StringSliceVar(&cmd.ideFiles, "ide", []string{}, "")
	// Unlike StringSliceVar, StringArrayVar doesn't split flag values at
	// commas. Commas are syntactically significant in GN, so they should be
	// preserved rather than interpreting them as value separators.
	flagSet.StringArrayVar(&cmd.gnArgs, "args", []string{}, "")

	flagSet.StringSliceVar(&cmd.assemblyOverrideStrings, "assembly-override", []string{}, "")

	if err := flagSet.Parse(args); err != nil {
		return nil, err
	}

	if env["FUCHSIA_BUILD_DIR_FROM_FX"] != "" && flagSet.Changed("dir") {
		return nil, fmt.Errorf("cannot specify --dir both as global flag and as subcommand flag")
	}

	// Check and rebase if buildDir is an absolute path.
	if filepath.IsAbs(cmd.buildDir) {
		if !strings.HasPrefix(cmd.buildDir, cmd.checkoutDir+"/") {
			return nil, fmt.Errorf("build dir %q is not under checkout dir %q", cmd.buildDir, cmd.checkoutDir)
		}
		var err error
		cmd.buildDir, err = filepath.Rel(cmd.checkoutDir, cmd.buildDir)
		if err != nil {
			return nil, fmt.Errorf("rebasing build dir to check out dir: %v", err)
		}
	}

	modesSet := 0
	for _, f := range []string{"release", "debug", "balanced", "compilation-mode"} {
		if flagSet.Changed(f) {
			modesSet++
		}
	}
	if modesSet > 1 {
		return nil, fmt.Errorf("Only one of --release, --debug, --balanced, or --compilation-mode can be specified.")
	}

	switch cmd.compilationMode {
	case "release", "debug", "balanced":
	default:
		return nil, fmt.Errorf("Invalid --compilation-mode: %q. Valid values are 'release', 'balanced', 'debug'.", cmd.compilationMode)
	}

	if cmd.buildDir != "" {
		autoDir = false
	}

	if cmd.buildDir == symlinkBuildDir {
		// If the developer wants to use out/default as the out dir, and we had been using `symlinkBuildDir` as a symlink, unlink it.
		userSpecifiedPath := filepath.Join(cmd.checkoutDir, cmd.buildDir)
		symlink, err := isSymlink(userSpecifiedPath)
		if err != nil {
			return nil, err
		}
		if symlink {
			if err := os.Remove(userSpecifiedPath); err != nil {
				return nil, fmt.Errorf("failed to unlink: %+v", err)
			}
		}
	}

	// If a fint params file was specified then no other arguments are required,
	// so no need to validate them.
	if cmd.fintParamsPath != "" {
		if cmd.buildDir == "" {
			return nil, fmt.Errorf("build directory must be set (e.g. through --dir) when --fint-params-path is set")
		}
		return cmd, nil
	}

	if cmd.useCcache && cmd.noCcache {
		return nil, fmt.Errorf("--ccache and --no-ccache are mutually exclusive")
	}

	if cmd.enableCxxRbe && cmd.useCcache {
		return nil, fmt.Errorf("--cxx-rbe and --use-ccache are mutually exclusive")
	}
	if cmd.enableCxxRbe && cmd.disableCxxRbe {
		return nil, fmt.Errorf("--cxx-rbe and --no-cxx-rbe are mutually exclusive")
	}

	if flagSet.NArg() == 0 {
		return nil, fmt.Errorf("missing a PRODUCT.BOARD argument")
	} else if flagSet.NArg() > 1 {
		return nil, fmt.Errorf("only one positional PRODUCT.BOARD argument allowed")
	}

	productDotBoard := flagSet.Arg(0)
	productAndBoard := strings.Split(productDotBoard, ".")
	if len(productAndBoard) != 2 {
		return nil, fmt.Errorf("unable to parse PRODUCT.BOARD: %q", productDotBoard)
	}
	cmd.product, cmd.board = productAndBoard[0], productAndBoard[1]

	if autoDir {
		for _, variant := range cmd.variants {
			if strings.Contains(variant, "/") {
				return nil, fmt.Errorf("This variant builds cannot be automatically named. Please specify a directory name with fx --dir")
			}
		}
		nameComponents := []string{productDotBoard}
		nameComponents = append(nameComponents, cmd.variants...)
		nameComponents = append(nameComponents, cmd.compilationMode)
		cmd.buildDir = filepath.Join("out", strings.Join(nameComponents, "-"))
	}
	message := "The build directory for this build is " + cmd.buildDir + "\n"

	// TODO(https://fxbug.dev/396658029): Clippy actually does not play nicely
	// with variants at the moment; defaulting to true in that case results in
	// `gen` error.
	if !flagSet.Changed("include-clippy") && len(cmd.variants) > 0 {
		message += "Warning: auto-disabling Clippy due to variant selection (see https://fxbug.dev/396658029)\n"
		cmd.includeClippy = false
	}

	hostname, _ := os.Hostname()
	isVirtual := strings.HasSuffix(hostname, "c.googlers.com")
	isCorp := strings.HasSuffix(hostname, ".corp.google.com")
	if isVirtual || isCorp {
		cores := runtime.NumCPU()
		cpuNudge, _ := isNudgeOn(env, "cpu_cores")
		if cores < 96 && cpuNudge {
			if isVirtual {
				message += "[Nudge] For faster builds, resize your machine to at least 96 cores (go/launch-specialist)\n"
			} else {
				message += "[Nudge] For faster builds, upgrade your machine to at least 96 cores (go/launch-specialist)\n"
			}
			message += "(Silence nudge with `ffx config set ffx.ui.nudges.cpu_cores false`)\n"
		}
	}

	balancedNudge, _ := isNudgeOn(env, "balanced")
	if cmd.compilationMode == "release" && balancedNudge {
		message += "[Nudge] You have set --release, consider --balanced: https://fuchsia.dev/fuchsia-src/development/build/overview#quick_comparison\n"
		message += "(Silence nudge with `ffx config set ffx.ui.nudges.balanced false`)\n"
	}
	fmt.Printf(message)
	return cmd, nil
}

// rbeIsSupported returns true if the RBE is supported on the current platform.
func rbeIsSupported() bool {
	return (runtime.GOOS == "linux") && (runtime.GOARCH == "amd64")
}

// rbeHostType returns the type of host that this appears to be, defaulting to
// "workstation".
func rbeHostType() string {
	cmd := exec.Command("vendor/google/scripts/devshell/detect-build-host-class")
	out, err := cmd.Output()
	if err != nil {
		return "workstation"
	} else {
		return strings.Split(string(out), "\n")[0]
	}
}

func constructStaticSpec(checkoutDir string, args *setArgs, canUseRbe bool) (*fintpb.Static, error) {
	productPath, err := findGNIFile(checkoutDir, "products", args.product)
	if err != nil {
		productPath, err = findGNIFile(checkoutDir, filepath.Join("products", "tests"), args.product)
	}
	if err != nil {
		return nil, fmt.Errorf("no such product %q", args.product)
	}
	boardPath, err := findGNIFile(checkoutDir, "boards", args.board)
	if err != nil {
		return nil, fmt.Errorf("no such board: %q", args.board)
	}

	compilationMode := fintpb.Static_COMPILATION_MODE_BALANCED
	switch args.compilationMode {
	case "release":
		compilationMode = fintpb.Static_COMPILATION_MODE_RELEASE
	case "debug":
		compilationMode = fintpb.Static_COMPILATION_MODE_DEBUG
	}

	variants := args.variants
	for _, sanitizer := range args.fuzzSanitizers {
		variants = append(variants, fuzzerVariants(sanitizer)...)
	}

	gnArgs := args.gnArgs

	// fint already translates the *_rbe_enable variables into GN args.

	if args.buildEventService != "" {
		gnArgs = append(gnArgs, fmt.Sprintf("bazel_upload_build_events = \"%s\"", args.buildEventService))
	}

	if args.includeClippy {
		gnArgs = append(gnArgs, "include_clippy=true")
	}

	hostLabels := args.hostLabels
	if args.cargoTOMLGen {
		hostLabels = append(hostLabels, "//build/rust:cargo_toml_gen")
	}

	targetLabels := append(append([]string{}, args.targetLabels...), args.testLabels...)

	static := &fintpb.Static{
		Board:             boardPath,
		Product:           productPath,
		MainPbLabel:       args.mainPbLabel,
		CompilationMode:   compilationMode,
		TargetLabels:      targetLabels,
		HostLabels:        hostLabels,
		Variants:          variants,
		GnArgs:            gnArgs,
		RustRbeEnable:     args.enableRustRbe,
		LinkRbeEnable:     args.enableLinkRbe,
		BazelRbeEnable:    args.enableBazelRbe,
		BuildEventService: args.buildEventService,
		IdeFiles:          args.ideFiles,
		JsonIdeScripts:    args.jsonIDEScripts,
		ExportRustProject: true,
	}
	return applyRbeSettings(static, args, canUseRbe)
}

// Used for mocking in tests.
var probeXattr = probeXattrSupport

func applyRbeSettings(static *fintpb.Static, args *setArgs, canUseRbe bool) (*fintpb.Static, error) {
	rbeSupported := rbeIsSupported()
	rbeMode := args.rbeMode
	if rbeMode == "auto" {
		if rbeSupported && canUseRbe {
			rbeMode = rbeHostType()
		} else {
			rbeMode = "off"
		}
	}

	// Check for RBE eligibility.
	requestedAnyRbe := rbeMode != "off" || args.enableCxxRbe || args.enableRustRbe || args.enableLinkRbe || args.enableBazelRbe
	if requestedAnyRbe {
		if !rbeSupported {
			return nil, fmt.Errorf("Sorry, RBE is only supported on linux-x64 at this time.")
		}
		if !canUseRbe {
			fmt.Println("Note: RBE is not publicly accessible at this time.")
		}
	}

	var (
		// These variables eventually represent our final decisions of whether
		// to use a compiler prefix, since the logic is somewhat convoluted.
		useCxxRbeFinal bool
		useCcacheFinal bool
	)

	// Check CCACHE_DIR if it is specified.
	if !(args.useCcache || args.noCcache) {
		if args.ccacheDir != "" {
			isDir, err := osmisc.IsDir(args.ccacheDir)
			if err != nil {
				return nil, fmt.Errorf("failed to check existence of $%s: %w", ccacheDirEnvVar, err)
			}
			if !isDir {
				return nil, fmt.Errorf("$%s=%s does not exist or is a regular file", ccacheDirEnvVar, args.ccacheDir)
			}
			useCcacheFinal = true
		}
	}

	if args.enableCxxRbe {
		useCxxRbeFinal = true
	}

	if args.useCcache {
		useCcacheFinal = true
	} else if args.noCcache {
		useCcacheFinal = false
	}

	gnArgs := static.GnArgs
	if useCcacheFinal {
		gnArgs = append(gnArgs, "use_ccache=true")
	}

	// Always write out rbe_mode, even if it is the default "off".
	// This makes it easier for users to `fx args` and edit.
	gnArgs = append(gnArgs, fmt.Sprintf("rbe_mode=\"%s\"", rbeMode))

	static.GnArgs = gnArgs
	static.CxxRbeEnable = useCxxRbeFinal

	buildDirAbs := filepath.Join(args.checkoutDir, args.buildDir)
	if supported, err := probeXattr(buildDirAbs); err == nil && !supported {
		static.DisableXattrForRbe = true
	}

	return static, nil
}

// fuzzerVariants produces the variants for enabling a sanitizer on fuzzers.
func fuzzerVariants(sanitizer string) []string {
	return []string{
		fmt.Sprintf(`{variant="%s-fuzzer" target_type=["fuzzer_engine"]}`, sanitizer),
		fmt.Sprintf(`{variant="%s-fuzzer" target_type=["executable"]}`, sanitizer),
		// TODO(https://fxbug.dev/42113953): Fuzzers need a version of libfdio.so that is sanitized,
		// but doesn't collect coverage data.
		fmt.Sprintf(`{variant="%s" label=["//sdk/lib/fdio"]}`, sanitizer),
	}
}

// findGNIFile returns the relative path to a board or product file in a
// checkout, given a basename. It checks the root of the checkout as well as
// each vendor/* directory for a file matching "<dirname>/<basename>.gni", e.g.
// "boards/core.gni".
func findGNIFile(checkoutDir, dirname, basename string) (string, error) {
	dirs, err := filepath.Glob(filepath.Join(checkoutDir, "vendor", "*", dirname))
	if err != nil {
		return "", err
	}
	// Prefer vendor products in alphabetical order.
	sort.Strings(dirs)
	dirs = append(dirs, filepath.Join(checkoutDir, dirname))

	for _, dir := range dirs {
		path := filepath.Join(dir, fmt.Sprintf("%s.gni", basename))
		exists, err := osmisc.FileExists(path)
		if err != nil {
			return "", err
		}
		if exists {
			return filepath.Rel(checkoutDir, path)
		}
	}

	return "", fmt.Errorf("no such file %s.gni", basename)
}

func allEnvVars() map[string]string {
	env := make(map[string]string)
	for _, keyAndValue := range os.Environ() {
		parts := strings.SplitN(keyAndValue, "=", 2)
		key, val := parts[0], parts[1]
		env[key] = val
	}
	return env
}

// canAccessRbe returns true if there is evidence from the user's environment
// and source checkout that suggests they have RBE access privileges.
// Note: This is not perfect because it does not actually check against ACL
// but it avoids the problem of external developers accidentally
// configuring use of RBE.
// TODO(b/356896318): distinguish between cache-reading and remote execution
// privileges.
func canAccessRbe(ctx context.Context, checkoutDir string) (bool, error) {
	cmd := exec.CommandContext(ctx, "git", "remote", "-v")
	cmd.Dir = filepath.Join(checkoutDir, "integration")
	out, err := cmd.Output()
	if err != nil {
		return false, err
	}
	lines := strings.Split(string(out), "\n")
	// Check all remotes.  If any have SSO or RPC access, then assume user
	// can access RBE.
	for _, line := range lines {
		fields := strings.Fields(line)
		// Expect lines like:
		//   "origin	sso://.../integration (fetch)"
		// or
		//   "origin	rpc://.../integration (fetch)"
		// or
		//   "origin	https://.../integration (fetch)"
		if len(fields) >= 2 && (strings.HasPrefix(fields[1], "sso://") || strings.HasPrefix(fields[1], "rpc://")) {
			return true, nil
		}
	}
	return false, nil
}

// isSymlink returns true if the path is a symlink,
// it returns false if the path doesn't exist.
func isSymlink(path string) (bool, error) {
	fileInfo, err := os.Lstat(path)
	if err != nil {
		if os.IsNotExist(err) {
			return false, nil
		}
		return false, fmt.Errorf("error checking path %s: %w", path, err)
	}
	isLink := fileInfo.Mode()&os.ModeSymlink != 0
	return isLink, nil
}

func resolveProductBundleName(checkoutDir, buildDir, name string) (string, error) {
	pbPath := filepath.Join(checkoutDir, buildDir, productBundlesFile)
	data, err := os.ReadFile(pbPath)
	if err != nil {
		return "", fmt.Errorf("failed to read %s: %w", pbPath, err)
	}

	var bundles []productBundle
	if err := json.Unmarshal(data, &bundles); err != nil {
		return "", fmt.Errorf("failed to parse %s: %w", pbPath, err)
	}

	for _, b := range bundles {
		if b.Name == name {
			label := strings.Split(b.Label, "(")[0]
			return label, nil
		}
	}

	var availableNames []string
	for _, b := range bundles {
		availableNames = append(availableNames, b.Name)
	}
	sort.Strings(availableNames)
	return "", fmt.Errorf("product bundle %q not found in %s\nAvailable product bundles:\n  %s", name, pbPath, strings.Join(availableNames, "\n  "))
}

func isNudgeOn(env map[string]string, nudge string) (bool, error) {
	nudge_string := "ffx.ui.nudges." + nudge
	cmd := exec.Command("ffx", "config", "get", nudge_string)
	cmd.Dir = env[checkoutDirEnvVar]
	out, err := cmd.Output()
	if err == nil {
		return strconv.ParseBool(strings.TrimSpace(string(out)))
	} else {
		return true, err
	}
}
