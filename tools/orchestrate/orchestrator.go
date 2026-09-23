// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package orchestrate

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	ffx "go.fuchsia.dev/fuchsia/tools/orchestrate/ffx"
	utils "go.fuchsia.dev/fuchsia/tools/orchestrate/utils"
)

// FFXClient defines the interface for interacting with ffx commands.
type FFXClient interface {
	Close() error
	ApplyEnv(env []string) ([]string, error)
	SetDefaultTarget(target *string)
	Flash(ctx context.Context, fastbootSerial, productDir, pubKeyPath string) error
	IsPackageServerRunning(ctx context.Context, repoName string) (bool, error)

	// High-level provisioning operations
	SetupFfx(ctx context.Context, repoName string) error
	ProductDownload(ctx context.Context, transferURL, outDir, authPath string) error
	EmuStart(ctx context.Context, productDir, name, engine, device string) error
	EmuStop(ctx context.Context) error
	RepositoryCreate(ctx context.Context, repoDir string) error
	RepositoryPublish(ctx context.Context, repoDir, productDir string, packageArchives []string) error
	SymbolIndexAdd(ctx context.Context, buildID string) error
	RepositoryServerStart(ctx context.Context, repoName, repoDir, address string) error
	RepositoryServerStop(ctx context.Context, repoName string) error
	RepositoryServerList(ctx context.Context) (string, error)
	TargetAdd(ctx context.Context, addr string) error
	TargetList(ctx context.Context) (string, error)
	TargetWait(ctx context.Context) error
	TargetShow(ctx context.Context) (string, error)
	TargetRepositoryRegister(ctx context.Context, repoName string, aliases []string) error
	TargetSnapshot(ctx context.Context, dir string) error
	Symbolize(ctx context.Context, input io.Reader, output io.Writer) error
	TargetLogStart(ctx context.Context, output io.Writer) (io.Closer, error)
}

// TestOrchestrator uses FFX to run Fuchsia component tests.
type TestOrchestrator struct {
	ffx             FFXClient
	deviceConfig    *DeviceConfig
	targetLogCloser io.Closer
	targetLogFile   *os.File
	repoName        string
}

var (
	ffxConfigDump = filepath.Join(utils.GetOutputsDir(), "ffx_config.txt")
	subrunnerLog  = filepath.Join(utils.GetOutputsDir(), "subrunner.log")
	targetLog     = filepath.Join(utils.GetOutputsDir(), "target.log")
	targetSymLog  = filepath.Join(utils.GetOutputsDir(), "target.symbolized.log")
	summaryPath   = filepath.Join(utils.GetOutputsDir(), "summary.json")
)

// NewTestOrchestrator creates a TestOrchestrator with default dependencies.
func NewTestOrchestrator(deviceConfig *DeviceConfig) *TestOrchestrator {
	return &TestOrchestrator{
		deviceConfig: deviceConfig,
		repoName:     fmt.Sprintf("repo-%d", os.Getpid()),
	}
}

func (r *TestOrchestrator) instantiateFfx(ctx context.Context, in *RunInput) error {
	if r.ffx != nil {
		return nil
	}
	ffxPath := in.Target().FfxPath
	if ffxPath == "" {
		return fmt.Errorf("ffx path is empty")
	}
	var err error
	ffxPath, err = filepath.Abs(ffxPath)
	if err != nil {
		return fmt.Errorf("resolving ffx path: %w", err)
	}

	outputsDir := utils.GetOutputsDir()
	if in.HasExperiment("orchestrate-ffx-strict") {
		client, err := NewFFXStrictClient(ctx, ffxPath, outputsDir, r.repoName, in.Target().FfxConfig)
		if err != nil {
			return fmt.Errorf("NewFFXStrictClient: %w", err)
		}
		r.ffx = client
		if r.deviceConfig != nil && r.deviceConfig.Network.IPv4 != "" {
			r.ffx.SetDefaultTarget(&r.deviceConfig.Network.IPv4)
		}
		return nil
	}

	ffxOpt := &ffx.Option{
		ExePath:   ffxPath,
		LogDir:    outputsDir,
		FfxConfig: in.Target().FfxConfig,
	}
	f, err := ffx.New(ctx, ffxOpt)
	if err != nil {
		return fmt.Errorf("ffx.New: %w", err)
	}
	r.ffx = f
	return nil
}

// Run executes tests.
func (r *TestOrchestrator) Run(ctx context.Context, in *RunInput, testCmd []string) error {
	if len(in.Cipd()) > 0 {
		fmt.Println("=== orchestrate - Downloading CIPD packages (0/6) ===")
		if err := r.cipdEnsure(ctx, in); err != nil {
			return fmt.Errorf("cipdEnsure: %w", err)
		}
	}
	if in.IsTarget() {
		fmt.Println("=== orchestrate - Setting up ffx (1/6) ===")
		if err := r.instantiateFfx(ctx, in); err != nil {
			return fmt.Errorf("instantiateFfx: %w", err)
		}
		defer func() {
			if err := r.ffx.Close(); err != nil {
				fmt.Printf("ffx.Close: %v\n", err)
			}
		}()
		if err := r.ffx.SetupFfx(ctx, r.repoName); err != nil {
			return fmt.Errorf("SetupFfx: %w", err)
		}
		productDir := ""
		if in.Target().TransferURL != "" {
			fmt.Println("=== orchestrate - Downloading Product Bundle (2/6) ===")
			var err error
			productDir, err = r.downloadProductBundle(ctx, in)
			if err != nil {
				return fmt.Errorf("downloadProductBundle: %w", err)
			}
		} else if in.Target().LocalPB != "" {
			fmt.Println("=== orchestrate - Local Product Bundle (2/6) ===")
			var err error
			productDir, err = filepath.Abs(in.Target().LocalPB)
			if err != nil {
				return fmt.Errorf("resolving local_pb path: %w", err)
			}
		}
		if in.IsHardware() {
			fmt.Println("=== orchestrate - Flashing Device (3/6) ===")
			if err := r.flashDevice(ctx, productDir); err != nil {
				return fmt.Errorf("flashDevice: %w", err)
			}
		} else if in.IsEmulator() {
			fmt.Println("=== orchestrate - Starting Emulator (3/6) ===")
			if err := r.startEmulator(ctx, productDir, in.Emulator.Engine, in.Emulator.Device); err != nil {
				return fmt.Errorf("startEmulator: %w", err)
			}
			defer func() {
				if err := r.ffx.EmuStop(context.Background()); err != nil {
					fmt.Printf("EmuStop: %v\n", err)
				}
			}()
		}
		fmt.Println("=== orchestrate - Serving Packages (4/6) ===")
		if err := r.servePackages(ctx, in, productDir); err != nil {
			return fmt.Errorf("servePackages: %w", err)
		}
		defer r.stopPackageServer()
		fmt.Println("=== orchestrate - Reach Device (5/6) ===")
		if err := r.reachDevice(ctx); err != nil {
			return fmt.Errorf("reachDevice: %w", err)
		}
		defer r.stopFfxLog()
	} else {
		fmt.Println("=== orchestrate - Skipped Target Provisioning (1-5/6) ===")
	}
	fmt.Println("=== orchestrate - Test (6/6) ===")
	if err := r.test(ctx, testCmd, in); err != nil {
		return fmt.Errorf("test: %w", err)
	}
	return nil
}

/* Step 0 - Downloading CIPD packages. */
func (r *TestOrchestrator) cipdEnsure(ctx context.Context, in *RunInput) error {
	wd, err := os.Getwd()
	if err != nil {
		return fmt.Errorf("os.Getwd: %w", err)
	}
	for destPath, cipdSpec := range in.Cipd() {
		split := strings.SplitN(cipdSpec, ":", 2)
		ensureLine := fmt.Sprintf("%s\t%s\n", split[0], split[1])
		cipdCmd := []string{
			"cipd",
			"ensure",
			"-ensure-file",
			"-",
			"-root",
			filepath.Join(wd, destPath),
			"-service-account-json",
			":gce",
		}
		fmt.Printf("Running command: %+v stdin: %s", cipdCmd, ensureLine)
		cmd := exec.CommandContext(ctx, cipdCmd[0], cipdCmd[1:]...)
		cmd.Stdout = os.Stdout
		cmd.Stderr = os.Stderr
		cmd.Stdin = strings.NewReader(ensureLine)
		cmd.Env = os.Environ()
		if err := cmd.Run(); err != nil {
			return fmt.Errorf("cmd.Run: %w", err)
		}
	}
	return nil
}

/* Step 2 - Downloading product bundle. */
func (r *TestOrchestrator) downloadProductBundle(ctx context.Context, in *RunInput) (string, error) {
	wd, err := os.Getwd()
	if err != nil {
		return "", fmt.Errorf("os.Getwd: %w", err)
	}
	dir := filepath.Join(wd, "ffx-product-bundle")

	if err := r.ffx.ProductDownload(ctx, in.Target().TransferURL, dir, in.Target().FfxluciauthPath); err != nil {
		return "", fmt.Errorf("ffx product download: %w", err)
	}
	return dir, nil
}

/* Step 3 - Flashing device OR Starting emulator. */
func (r *TestOrchestrator) flashDevice(ctx context.Context, productDir string) error {
	if err := r.ffx.Flash(ctx, r.deviceConfig.FastbootSerial, productDir, ""); err != nil {
		return fmt.Errorf("ffx flash: %w", err)
	}
	return nil
}

func (r *TestOrchestrator) startEmulator(ctx context.Context, productDir, engine, device string) error {
	emu_name := fmt.Sprintf("fuchsia-emulator-%d", os.Getpid())

	if err := r.ffx.EmuStart(ctx, productDir, emu_name, engine, device); err != nil {
		return fmt.Errorf("ffx emu start: %w", err)
	}

	// Set the emulator as the default
	r.ffx.SetDefaultTarget(&emu_name)

	return nil
}

/* Step 4 - Serving packages. */
/*
Serving packages requires:
* Creating a new package repository
* Publishing all packages from the product bundle into the repository.
* Publshing any user provided packages, which may replace the packages from the product bundle.
* Starting the package server process
* Registering the package server on the target device.
* Package servers are managed by name.

*/
func (r *TestOrchestrator) servePackages(ctx context.Context, in *RunInput, productDir string) error {
	wd, err := os.Getwd()
	if err != nil {
		return fmt.Errorf("os.Getwd: %w", err)
	}
	repoDir := filepath.Join(wd, "repo")
	if err := r.ffx.RepositoryCreate(ctx, repoDir); err != nil {
		return fmt.Errorf("ffx repository create: %w", err)
	}
	if err := r.ffx.RepositoryPublish(ctx, repoDir, productDir, in.Target().PackageArchives); err != nil {
		return fmt.Errorf("ffx repository publish: %w", err)
	}
	for _, buildID := range in.Target().BuildIds {
		if err := r.ffx.SymbolIndexAdd(ctx, buildID); err != nil {
			return fmt.Errorf("ffx debug symbol-index add %s: %w", buildID, err)
		}
	}

	if err := r.serveAndWait(ctx, repoDir); err != nil {
		return fmt.Errorf("serveAndWait: %w", err)
	}

	if _, err := r.ffx.RepositoryServerList(ctx); err != nil {
		return fmt.Errorf("ffx repository server list: %w", err)
	}
	return nil
}

func (r *TestOrchestrator) serveAndWait(ctx context.Context, repoDir string) error {
	port := os.Getenv("FUCHSIA_PACKAGE_SERVER_PORT")
	if port == "" {
		// Use a dynamic port unless the environment is specific.
		port = "0"
	}
	addr := fmt.Sprintf("[::]:%s", port)
	if err := r.ffx.RepositoryServerStart(ctx, r.repoName, repoDir, addr); err != nil {
		return fmt.Errorf("ffx repository server start: %w", err)
	}

	// The server start command when using `--background` waits for the server
	// to actually start before exiting, so this check is a double check.
	running, err := r.ffx.IsPackageServerRunning(ctx, r.repoName)
	if err != nil {
		return fmt.Errorf("ffx isPackageServerRunning: %w", err)
	}
	if !running {
		return fmt.Errorf("repository %s is not running", r.repoName)
	}
	return nil
}

/* Step 5 - Reach Device */
func (r *TestOrchestrator) reachDevice(ctx context.Context) error {
	if r.deviceConfig != nil {
		addr := r.deviceConfig.Network.IPv4
		if err := r.ffx.TargetAdd(ctx, addr); err != nil {
			return fmt.Errorf("ffx target add: %w", err)
		}
		r.ffx.SetDefaultTarget(&addr)
	}

	if _, err := r.ffx.TargetList(ctx); err != nil {
		return fmt.Errorf("ffx target list: %w", err)
	}

	if err := r.ffx.TargetWait(ctx); err != nil {
		return fmt.Errorf("ffx target wait: %w", err)
	}
	if _, err := r.ffx.TargetShow(ctx); err != nil {
		return fmt.Errorf("ffx target show: %w", err)
	}
	if err := r.dumpFfxLog(ctx); err != nil {
		return fmt.Errorf("dumpFfxLog: %w", err)
	}

	// Register the repo server using the aliases configured with the running server.
	if err := r.ffx.TargetRepositoryRegister(ctx, r.repoName, []string{"fuchsia.com", "chromium.org"}); err != nil {
		return fmt.Errorf("ffx target repository register: %w", err)
	}
	return nil
}

func (r *TestOrchestrator) dumpFfxLog(ctx context.Context) error {
	logFile, err := os.Create(targetLog)
	if err != nil {
		return fmt.Errorf("os.Create: %w", err)
	}
	r.targetLogFile = logFile
	closer, err := r.ffx.TargetLogStart(ctx, logFile)
	if err != nil {
		return fmt.Errorf("TargetLogStart: %w", err)
	}
	r.targetLogCloser = closer
	return nil
}

/* Step 6 - Test */
func (r *TestOrchestrator) test(ctx context.Context, testCmd []string, in *RunInput) error {
	wd, err := os.Getwd()
	if err != nil {
		return fmt.Errorf("os.Getwd: %w", err)
	}
	logFile, err := os.Create(subrunnerLog)
	if err != nil {
		return fmt.Errorf("os.Create: %w", err)
	}
	defer func() {
		if err := logFile.Close(); err != nil {
			fmt.Printf("logFile.Close: %v\n", err)
		}
	}()

	// Prepare the env for target tests:
	//  1. Applies default ffx cmd environment variables
	//     (eg: isolation, disabling analytics).
	//  2. Adds ffx so that downstream can call "ffx" without having to leak its
	//     full path.
	//  3. Add openssh to PATH.
	env := os.Environ()
	if in.IsTarget() {
		env, err = r.ffx.ApplyEnv(env)
		if err != nil {
			return fmt.Errorf("ffx.ApplyEnv: %v", err)
		}
		ffxDir := filepath.Dir(filepath.Join(wd, in.Target().FfxPath))
		if err = os.Setenv("PATH", fmt.Sprintf("%s:%s", ffxDir, os.Getenv("PATH"))); err != nil {
			return fmt.Errorf("os.Setenv: %w", err)
		}
		env = utils.AppendPath(env, ffxDir)
	}

	// Create cmd AFTER setting the PATH so that it will correctly resolve testCmd[0]
	cmd := exec.CommandContext(ctx, testCmd[0], testCmd[1:]...)
	cmd.Env = env

	// Setup pipes to forward subcmd stdout and stderr to logFile and os.Stdout.
	pipeOut := io.MultiWriter(logFile, os.Stdout)
	cmd.Stdout = pipeOut
	cmd.Stderr = pipeOut

	fmt.Printf("Running test: %+v\n", cmd.Args)
	testErr := cmd.Run()
	fmt.Printf("Pausing 10 seconds for log flush...\n")
	time.Sleep(10 * time.Second)
	if in.IsTarget() {
		snapshotCtx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
		defer cancel()
		if err := r.ffx.TargetSnapshot(snapshotCtx, utils.GetOutputsDir()); err != nil {
			fmt.Printf("target snapshot: %v\n", err)
		}
	}
	if err := r.writeTestSummary(testErr); err != nil {
		return fmt.Errorf("writeTestSummary: %w", err)
	}
	// TODO(b/322928092): Disable and remove this once `orchestrate` is the
	// entrypoint for all bazel_build_test_upload invocations.
	if in.HasExperiment("orchestrate-error-on-test-failure") && testErr != nil {
		return fmt.Errorf("Test Failures: %w", testErr)
	}
	return nil
}

// testSummary determines the data for out/summary.json
type testSummary struct {
	Success bool `json:"success"`
}

func (r *TestOrchestrator) writeTestSummary(testErr error) error {
	if testErr != nil {
		fmt.Printf("Tests failed: %v\n", testErr)
	}
	summary := &testSummary{
		Success: testErr == nil,
	}
	if err := os.MkdirAll(filepath.Dir(summaryPath), 0755); err != nil {
		return fmt.Errorf("os.MkdirAll: %w", err)
	}
	if err := writeJSON(summaryPath, summary); err != nil {
		return fmt.Errorf("writeJSON: %w", err)
	}
	return nil
}

func writeJSON(filename string, data any) error {
	rawData, err := json.MarshalIndent(data, "", "  ")
	if err != nil {
		return fmt.Errorf("json.MarshalIndent: %w", err)
	}
	if err = os.WriteFile(filename, rawData, 0644); err != nil {
		return fmt.Errorf("os.WriteFile: %w", err)
	}
	return nil
}

/* Cleanup */
func (r *TestOrchestrator) stopPackageServer() {
	if err := r.ffx.RepositoryServerStop(context.Background(), r.repoName); err != nil {
		fmt.Printf("ffx repository server stop: %v\n", err)
	}
}

func (r *TestOrchestrator) stopFfxLog() {
	if r.targetLogCloser == nil {
		return
	}
	if err := r.targetLogCloser.Close(); err != nil {
		fmt.Printf("targetLogCloser.Close: %v\n", err)
	}
	if err := r.targetLogFile.Close(); err != nil {
		fmt.Printf("targetLogFile.Close: %v\n", err)
	}
	// Symbolize logs
	if err := r.Symbolize(targetLog, targetSymLog); err != nil {
		fmt.Printf("Symbolize: %v\n", err)
	}
}

// Symbolize uses ffx to symbolize the log output.
func (r *TestOrchestrator) Symbolize(input, output string) error {
	logFile, err := os.Open(input)
	if err != nil {
		return fmt.Errorf("os.Open(%q): %w", input, err)
	}
	defer func() {
		if err := logFile.Close(); err != nil {
			fmt.Printf("logFile.Close: %v\n", err)
		}
	}()
	symbolizedFile, err := os.Create(output)
	if err != nil {
		return fmt.Errorf("os.Create(%q): %w", output, err)
	}
	defer func() {
		if err := symbolizedFile.Close(); err != nil {
			fmt.Printf("symbolizedFile.Close: %v\n", err)
		}
	}()
	return r.ffx.Symbolize(context.Background(), logFile, symbolizedFile)
}
