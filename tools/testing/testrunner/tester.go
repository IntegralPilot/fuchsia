// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSE file.

package testrunner

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"math"
	"net/url"
	"os"
	"os/exec"
	"os/user"
	"path/filepath"
	"regexp"
	"strings"
	"sync"
	"time"

	"go.fuchsia.dev/fuchsia/tools/botanist"
	botanistconstants "go.fuchsia.dev/fuchsia/tools/botanist/constants"
	"go.fuchsia.dev/fuchsia/tools/botanist/targets"
	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/debug/covargs"
	"go.fuchsia.dev/fuchsia/tools/integration/testsharder"
	"go.fuchsia.dev/fuchsia/tools/lib/clock"
	"go.fuchsia.dev/fuchsia/tools/lib/environment"
	"go.fuchsia.dev/fuchsia/tools/lib/ffxutil"
	ffxutilconstants "go.fuchsia.dev/fuchsia/tools/lib/ffxutil/constants"
	"go.fuchsia.dev/fuchsia/tools/lib/iomisc"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
	"go.fuchsia.dev/fuchsia/tools/lib/osmisc"
	"go.fuchsia.dev/fuchsia/tools/lib/retry"
	"go.fuchsia.dev/fuchsia/tools/lib/serial"
	"go.fuchsia.dev/fuchsia/tools/lib/subprocess"
	sshutilconstants "go.fuchsia.dev/fuchsia/tools/net/sshutil/constants"
	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
	"go.fuchsia.dev/fuchsia/tools/testing/testrunner/constants"
)

const (
	// Various tools for running tests.
	runtestsName     = "runtests"
	runTestSuiteName = "run-test-suite"

	// Returned by run-test-suite to indicate the test timed out.
	timeoutExitCode = 21

	// Printed to the serial console when ready to accept user input.
	serialConsoleCursor = "\n$"

	// Number of times to try running a test command over serial before giving
	// up. This value was somewhat arbitrarily chosen and can be adjusted higher
	// or lower if deemed appropriate.
	startSerialCommandMaxAttempts = 3

	llvmProfileEnvKey    = "LLVM_PROFILE_FILE"
	llvmProfileExtension = ".profraw"
	llvmProfileSinkType  = "llvm-profile"

	// This needs to be long enough to allow for significant serial RTT during
	// startup shortly after booting. We've seen a serial RTT ~8s, so maybe 15s
	// will be enough margin above ~8s.
	testStartedTimeout = 15 * time.Second

	// The name of the test to associate early boot data sinks with.
	earlyBootSinksTestName = "early_boot_sinks"

	// The max number of times to try reconnecting to the target.
	maxReconnectAttempts = 3

	// The default timeout for package resolution.
	defaultPackageResolutionTimeout = 2 * time.Minute
)

// Tester describes the interface for all different types of testers.
type Tester interface {
	SetupTest(context.Context, testsharder.Test) error
	Test(context.Context, testsharder.Test, io.Writer, io.Writer, string) (*runtests.TestDetails, error)
	ProcessResult(context.Context, testsharder.Test, string, *runtests.TestDetails, error) (*runtests.TestDetails, error)
	Close() error
	EnsureSinks(context.Context, []runtests.DataSinkMap, *TestOutputs) error
	RunSnapshot(context.Context, string) error
	Reconnect(context.Context) error
}

// For testability
type cmdRunner interface {
	Run(ctx context.Context, command []string, options subprocess.RunOptions) error
}

// For testability
var newRunner = func(dir string, env []string) cmdRunner {
	return &subprocess.Runner{Dir: dir, Env: env}
}

// For testability.
var newTempDir = func(dir, pattern string) (string, error) {
	return os.MkdirTemp(dir, pattern)
}

// For testability
type serialClient interface {
	runDiagnostics(ctx context.Context) error
}

// BaseTestResultFromTest returns a TestResult for a Tester.Test() to modify
// and return with some pre-filled values and a starting failure result which
// should be changed as needed within the tester's Test() method.
func BaseTestResultFromTest(test testsharder.Test) *runtests.TestDetails {
	return &runtests.TestDetails{
		Name:        test.Name,
		GNLabel:     test.Label,
		SourceLabel: test.SourceLabel,
		Status:      runtests.TestFailure,
		Tags:        test.Tags,
		Metadata:    test.Metadata,
	}
}

// SubprocessTester executes tests in local subprocesses.
type SubprocessTester struct {
	env            []string
	dir            string
	localOutputDir string
	sProps         *sandboxingProps
	testRuns       map[string]string
	target         targets.FuchsiaTarget
}

type sandboxingProps struct {
	nsjailPath    string
	nsjailRoot    string
	mountQEMU     bool
	mountUserHome bool
	cwd           string
}

type SubprocessTesterOptions struct {
	Dir        string
	Env        []string
	OutputDir  string
	NsjailPath string
	NsjailRoot string
	Target     targets.FuchsiaTarget
}

// NewSubprocessTester returns a SubprocessTester that can execute tests
// locally with a given working directory and environment.
func NewSubprocessTester(opts SubprocessTesterOptions) (Tester, error) {
	s := &SubprocessTester{
		dir:            opts.Dir,
		env:            opts.Env,
		localOutputDir: opts.OutputDir,
		testRuns:       make(map[string]string),
		target:         opts.Target,
	}
	// If the caller provided a path to NsJail, then intialize sandboxing properties.
	if opts.NsjailPath != "" {
		s.sProps = &sandboxingProps{
			nsjailPath: opts.NsjailPath,
			nsjailRoot: opts.NsjailRoot,
			// TODO(rudymathu): Remove this once ssh/ssh-keygen usage is removed.
			mountUserHome: true,
		}

		if _, err := os.Stat("/sys/class/net/qemu/"); err == nil {
			s.sProps.mountQEMU = true
		} else if !errors.Is(err, os.ErrNotExist) {
			return &SubprocessTester{}, nil
		}

		cwd, err := os.Getwd()
		if err != nil {
			return &SubprocessTester{}, err
		}
		s.sProps.cwd = cwd
	}
	return s, nil
}

func (t *SubprocessTester) SetupTest(_ context.Context, _ testsharder.Test) error {
	return nil
}

func (t *SubprocessTester) setTestRun(test testsharder.Test, profileRelDir string) {
	t.testRuns[test.Path] = profileRelDir
}

func (t *SubprocessTester) getTestRun(test testsharder.Test) string {
	profileRelDir, ok := t.testRuns[test.Path]
	if !ok {
		return ""
	}
	return profileRelDir
}

func (t *SubprocessTester) Test(ctx context.Context, test testsharder.Test, stdout io.Writer, stderr io.Writer, outDir string) (*runtests.TestDetails, error) {
	testResult := BaseTestResultFromTest(test)
	if test.Path == "" {
		testResult.FailureReason = runtests.FailureReasonFromMessage(fmt.Sprintf("test %q has no `path` set", test.Name))
		return testResult, nil
	}
	// Some tests read TestOutDirEnvKey so ensure they get their own output dir.
	if err := os.MkdirAll(outDir, 0o770); err != nil {
		testResult.FailureReason = runtests.FailureReasonFromMessage(err.Error())
		return testResult, nil
	}

	// Might as well emit any profiles directly to the output directory.
	// We'll set
	// LLVM_PROFILE_FILE=<output dir>/<test-specific namsepace>/%m.profraw
	// and then record any .profraw file written to that directory as an
	// emitted profile.
	profileRelDir := filepath.Join(llvmProfileSinkType, test.Path)
	profileAbsDir := filepath.Join(t.localOutputDir, profileRelDir)
	os.MkdirAll(profileAbsDir, os.ModePerm)

	testOutputSummaryDir := filepath.Join(t.localOutputDir, "test_output_summary", test.Path)
	if err := os.MkdirAll(testOutputSummaryDir, os.ModePerm); err != nil {
		testResult.FailureReason = runtests.FailureReasonFromMessage(err.Error())
		return testResult, nil
	}
	defer os.RemoveAll(filepath.Join(t.localOutputDir, "test_output_summary"))
	testOutputSummaryPath := filepath.Join(testOutputSummaryDir, "test_output_summary_path.json")

	ffxConfigs := []string{}
	sshControlMasterPath := ""
	if t.target != nil {
		sshControlMasterPath = t.target.SSHControlMasterPath()
		if _, err := os.Stat(sshControlMasterPath); err != nil {
			logger.Debugf(ctx, "not using ssh controlmaster because socket not created yet: %s", err)
			sshControlMasterPath = ""
		}
	}
	if sshControlMasterPath != "" {
		ffxConfigs = append(ffxConfigs, fmt.Sprintf("ssh.controlmaster.path=%s", sshControlMasterPath), "ssh.controlmaster.mode=explicit")
	}

	envOverrides := map[string]string{
		// Override FUCHSIA_TEST_OUTDIR with the outdir specific to this
		// test.
		constants.TestOutDirEnvKey: outDir,
		// When host-side tests are instrumented for profiling, executing
		// them will write a profile to the location under this environment variable.
		llvmProfileEnvKey:                     filepath.Join(profileAbsDir, "%m"+llvmProfileExtension),
		constants.TestOutputSummaryPathEnvKey: testOutputSummaryPath,
		constants.FFXConfigsEnvKey:            strings.Join(ffxConfigs, ","),
	}
	runnerEnv := t.env
	for key, value := range envOverrides {
		runnerEnv = append(runnerEnv, fmt.Sprintf("%s=%s", key, value))
	}
	r := newRunner(t.dir, runnerEnv)
	if test.Timeout > 0 {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, test.Timeout)
		defer cancel()
	}
	// './' is a package-level construct in os/exec whose use is recommended
	// when the provided invocation executes things relative to the local
	// working directory. In particular, unless it is in $PATH, executables in
	// the immediate working directory require this to be referenced
	// relatively as of Go v1.19.
	//
	// https://pkg.go.dev/os/exec#hdr-Executables_in_the_current_directory
	testCmd := []string{"./" + test.Path}
	if t.sProps != nil {
		testCmdBuilder := &NsJailCmdBuilder{
			Bin: t.sProps.nsjailPath,
			// TODO(rudymathu): Eventually, this should be a more fine grained
			// property that disables network isolation only on tests that explicitly
			// request it.
			IsolateNetwork: false,
			MountPoints: []*MountPt{
				{
					Src:      t.localOutputDir,
					Writable: true,
				},
				{
					Src:      outDir,
					Writable: true,
				},
				{
					// The fx_script_tests utilize this file.
					Src: "/usr/share/misc/magic.mgc",
				},
				{
					Src:      "/dev/shm",
					Writable: true,
				},
			},
			Symlinks: map[string]string{
				"/proc/self/fd": "/dev/fd",
			},
		}

		if sshControlMasterPath != "" {
			testCmdBuilder.MountPoints = append(
				testCmdBuilder.MountPoints,
				&MountPt{
					Src:      sshControlMasterPath,
					Writable: true,
				},
			)
		}

		if isolateDir, ok := os.LookupEnv(ffxutil.FFXIsolateDirEnvKey); ok {
			testCmdBuilder.MountPoints = append(
				testCmdBuilder.MountPoints,
				&MountPt{
					Src:      isolateDir,
					Writable: true,
				},
			)
		}

		if ffxSharedData, ok := os.LookupEnv(botanistconstants.FFXSharedDataEnvKey); ok {
			testCmdBuilder.MountPoints = append(
				testCmdBuilder.MountPoints,
				&MountPt{
					Src:      ffxSharedData,
					Writable: true,
				},
			)
		}

		// Mount the QEMU tun_flags if the qemu interface exists. This is used
		// by VDL to ascertain that the interface exists.
		if t.sProps.mountQEMU {
			testCmdBuilder.MountPoints = append(
				testCmdBuilder.MountPoints,
				&MountPt{
					Src: "/sys/class/net/qemu/",
				},
			)
		}

		// Some tests invoke the `ssh` command line tool, which always creates
		// a .ssh file in the home directory. Unfortunately, it prefers to read
		// the home directory from the /etc/passwd file, and only reads $HOME
		// if this doesn't work. Because we need to mount /etc/passwd for
		// ssh-keygen, we need to create the same home directory in
		// /etc/passwd. This is really quite a big hack, and we should remove
		// it ASAP.
		if t.sProps.mountUserHome {
			currentUser, err := user.Current()
			if err != nil {
				testResult.FailureReason = runtests.FailureReasonFromMessage(err.Error())
				return testResult, nil
			}
			pwdFile, err := os.Open("/etc/passwd")
			if err != nil {
				testResult.FailureReason = runtests.FailureReasonFromMessage(err.Error())
				return testResult, nil
			}
			defer pwdFile.Close()
			pwdScanner := bufio.NewScanner(pwdFile)
			for pwdScanner.Scan() {
				elems := strings.Split(pwdScanner.Text(), ":")
				if elems[0] == currentUser.Username {
					testCmdBuilder.MountPoints = append(
						testCmdBuilder.MountPoints,
						&MountPt{
							Dst:      elems[5],
							UseTmpfs: true,
						},
					)
					break
				}
			}
			if pwdScanner.Err() != nil {
				testResult.FailureReason = runtests.FailureReasonFromMessage(pwdScanner.Err().Error())
				return testResult, nil
			}
		}

		// Mount /tmp. Ideally, we would use a tmpfs mount, but we write quite a
		// lot of data to it, so we instead create a temp dir and mount it
		// instead.
		tmpDir, err := newTempDir("", "")
		if err != nil {
			testResult.FailureReason = runtests.FailureReasonFromMessage(err.Error())
			return testResult, nil
		}
		defer os.RemoveAll(tmpDir)
		testCmdBuilder.MountPoints = append(
			testCmdBuilder.MountPoints,
			&MountPt{
				Src:      tmpDir,
				Dst:      "/tmp",
				Writable: true,
			},
		)

		// Construct the sandbox's environment by forwarding the current env
		// but overriding the TempDirEnvVars with /tmp.
		envOverrides["TMPDIR"] = "/tmp"
		for _, key := range environment.TempDirEnvVars() {
			envOverrides[key] = "/tmp"
		}
		testCmdBuilder.ForwardEnv(envOverrides)

		// Set the root of the NsJail and the working directory.
		// The working directory is expected to be a subdirectory of the root.
		if t.sProps.nsjailRoot != "" {
			absRoot, err := filepath.Abs(t.sProps.nsjailRoot)
			if err != nil {
				testResult.FailureReason = runtests.FailureReasonFromMessage(err.Error())
				return testResult, nil
			}
			testCmdBuilder.MountPoints = append(
				testCmdBuilder.MountPoints,
				&MountPt{Src: absRoot, Writable: true},
			)
		}
		testCmdBuilder.Cwd = t.sProps.cwd

		// Mount the testbed config and any serial sockets.
		testbedConfigPath := os.Getenv(botanistconstants.TestbedConfigEnvKey)
		if testbedConfigPath != "" {
			// Mount the actual config.
			testCmdBuilder.MountPoints = append(testCmdBuilder.MountPoints, &MountPt{Src: testbedConfigPath})

			// Mount the SSH keys and serial sockets for each target in the testbed.
			type targetInfo struct {
				SerialSocket string `json:"serial_socket"`
				SSHKey       string `json:"ssh_key"`
			}
			b, err := os.ReadFile(testbedConfigPath)
			if err != nil {
				testResult.FailureReason = runtests.FailureReasonFromMessage(err.Error())
				return testResult, nil
			}
			var testbedConfig []targetInfo
			if err := json.Unmarshal(b, &testbedConfig); err != nil {
				testResult.FailureReason = runtests.FailureReasonFromMessage(err.Error())
				return testResult, nil
			}
			serialSockets := make(map[string]struct{})
			sshKeys := make(map[string]struct{})
			for _, config := range testbedConfig {
				if config.SSHKey != "" {
					sshKeys[config.SSHKey] = struct{}{}
				}
				if config.SerialSocket != "" {
					serialSockets[config.SerialSocket] = struct{}{}
				}
			}
			for socket := range serialSockets {
				absSocketPath, err := filepath.Abs(socket)
				if err != nil {
					testResult.FailureReason = runtests.FailureReasonFromMessage(err.Error())
					return testResult, nil
				}
				testCmdBuilder.MountPoints = append(testCmdBuilder.MountPoints, &MountPt{
					Src:      absSocketPath,
					Writable: true,
				})
			}
			for key := range sshKeys {
				absKeyPath, err := filepath.Abs(key)
				if err != nil {
					testResult.FailureReason = runtests.FailureReasonFromMessage(err.Error())
					return testResult, nil
				}
				testCmdBuilder.MountPoints = append(testCmdBuilder.MountPoints, &MountPt{
					Src: absKeyPath,
				})
			}
		}
		// The LUCI_CONTEXT is needed by OTA tests to run the artifacts tool
		// which requires authentication.
		if luciCtx := os.Getenv("LUCI_CONTEXT"); luciCtx != "" {
			absPath, err := filepath.Abs(luciCtx)
			if err != nil {
				testResult.FailureReason = runtests.FailureReasonFromMessage(err.Error())
				return testResult, nil
			}
			testCmdBuilder.MountPoints = append(testCmdBuilder.MountPoints, &MountPt{
				Src: absPath,
			})
		}
		testCmdBuilder.AddDefaultMounts()
		testCmd, err = testCmdBuilder.Build(testCmd)
		if err != nil {
			testResult.FailureReason = runtests.FailureReasonFromMessage(err.Error())
			return testResult, nil
		}
	}
	err := r.Run(ctx, testCmd, subprocess.RunOptions{Stdout: stdout, Stderr: stderr, Setpgid: true})
	t.setTestRun(test, profileRelDir)
	var exitErr *exec.ExitError
	if err == nil {
		testResult.Status = runtests.TestSuccess
	} else if errors.Is(err, context.DeadlineExceeded) {
		testResult.Status = runtests.TestAborted
	} else if againstDevice() && errors.As(err, &exitErr) {
		// Exit code 40 signals that the device is in a bad state and
		// needs to be power-cycled. See https://fxbug.dev/425675837.
		// A device in this state may still pass the generic health
		// check that's run in between tests so we explicitly check for
		// it here so that we can recover the device before the next test.
		if exitErr.ExitCode() == 40 {
			r := &subprocess.Runner{Env: os.Environ()}
			if err := setPowerState(ctx, r, "cycle"); err != nil {
				return testResult, fmt.Errorf("failed to power cycle target: %w", err)
			}
		}
		testResult.FailureReason = runtests.FailureReasonFromMessage(err.Error())
	} else {
		testResult.FailureReason = runtests.FailureReasonFromMessage(err.Error())
	}

	if bytes, err := os.ReadFile(testOutputSummaryPath); !os.IsNotExist(err) {
		if err != nil {
			return testResult, fmt.Errorf("failed to read test case summary: %s", err)
		} else {
			var resultDetails runtests.TestResult
			if err := json.Unmarshal(bytes, &resultDetails); err != nil {
				return testResult, fmt.Errorf("failed to unmarshal case results: %s", err)
			} else {
				if resultDetails.FailureReason == nil {
					resultDetails.FailureReason = testResult.FailureReason
				}
				testResult.TestResult = resultDetails
			}
		}
	}

	return testResult, nil
}

func (t *SubprocessTester) ProcessResult(ctx context.Context, test testsharder.Test, outDir string, testResult *runtests.TestDetails, err error) (*runtests.TestDetails, error) {
	profileRelDir := t.getTestRun(test)
	if profileRelDir == "" {
		return testResult, err
	}
	profileAbsDir := filepath.Join(t.localOutputDir, profileRelDir)
	var sinks []runtests.DataSink
	profileErr := filepath.WalkDir(profileAbsDir, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if !d.IsDir() {
			profileRel, err := filepath.Rel(profileAbsDir, path)
			if err != nil {
				return err
			}
			sinks = append(sinks, runtests.DataSink{
				Name: filepath.Base(path),
				File: filepath.Join(profileRelDir, profileRel),
			})
		}
		return nil
	})
	if profileErr != nil {
		logger.Errorf(ctx, "unable to determine whether profiles were emitted: %s", profileErr)
	}
	if len(sinks) > 0 {
		testResult.DataSinks = runtests.DataSinkMap{
			llvmProfileSinkType: sinks,
		}
	}
	return testResult, err
}

func (t *SubprocessTester) EnsureSinks(ctx context.Context, sinkRefs []runtests.DataSinkMap, _ *TestOutputs) error {
	// Nothing to actually copy; if any profiles were emitted, they would have
	// been written directly to the output directory. We verify here that all
	// recorded data sinks are actually present.
	numSinks := 0
	for _, ref := range sinkRefs {
		for _, sinks := range ref {
			for _, sink := range sinks {
				abs := filepath.Join(t.localOutputDir, sink.File)
				exists, err := osmisc.FileExists(abs)
				if err != nil {
					return fmt.Errorf("unable to determine if local data sink %q exists: %w", sink.File, err)
				} else if !exists {
					return fmt.Errorf("expected a local data sink %q, but no such file exists", sink.File)
				}
				numSinks++
			}
		}
	}
	if numSinks > 0 {
		logger.Debugf(ctx, "local data sinks present: %d", numSinks)
	}
	return nil
}

func (t *SubprocessTester) RunSnapshot(_ context.Context, _ string) error {
	return nil
}

func (t *SubprocessTester) Reconnect(_ context.Context) error {
	return nil
}

func (t *SubprocessTester) UpdateEnv(env []string) {
	t.env = env
}

func (t *SubprocessTester) Close() error {
	return nil
}

type serialSocket struct {
	socketPath string
}

func (s *serialSocket) runDiagnostics(ctx context.Context) error {
	if s.socketPath == "" {
		return fmt.Errorf("serialSocketPath not set")
	}
	socket, err := serial.NewSocket(ctx, s.socketPath)
	if err != nil {
		return fmt.Errorf("newSerialSocket failed: %w", err)
	}
	defer socket.Close()
	return serial.RunDiagnostics(ctx, socket)
}

// for testability
type FFXInstance interface {
	Doctor(ctx context.Context) error
	PackageResolve(ctx context.Context, packageURL string) error
	Run(ctx context.Context, args ...string) error
	RunWithTarget(ctx context.Context, args ...string) error
	TestEarlyBootProfile(ctx context.Context, outDir string) error
	SetTarget(target string)
	Stdout() io.Writer
	Stderr() io.Writer
	SetStdoutStderr(stdout, stderr io.Writer)
	TestRun(ctx context.Context, tests build.TestList, outDir string, args ...string) (*ffxutil.TestRunResult, error)
	Snapshot(ctx context.Context, outDir string, snapshotFilename string) error
	Stop() error
	TargetWait(ctx context.Context, args ...string) error
}

// FFXTester uses ffx to run tests and other enabled features.
type FFXTester struct {
	ffx            FFXInstance
	experiments    botanist.Experiments
	localOutputDir string
	target         targets.FuchsiaTarget
	testbedConfig  []any

	// The test output dirs from all the calls to Test().
	testOutDirs []string

	// A map of the test PackageURL to data needed for processing the test results.
	testRuns map[string]ffxTestRun

	llvmProfdata      string
	llvmVersion       string
	sinksPerTest      map[string]runtests.DataSinkMap
	debuginfodServers []string
	debuginfodCache   string

	// The following waitgroup and errs track the goroutines for merging profiles.
	wg     sync.WaitGroup
	errs   []error
	errsMu sync.Mutex
}

type ffxTestRun struct {
	result        *ffxutil.TestRunResult
	output        string
	totalDuration time.Duration
}

type FFXTesterOptions struct {
	Ffx           FFXInstance
	OutputDir     string
	Experiments   botanist.Experiments
	LLVMProfdata  string
	Target        targets.FuchsiaTarget
	TestbedConfig []any
}

// NewFFXTester returns an FFXTester.
func NewFFXTester(ctx context.Context, opts FFXTesterOptions) (*FFXTester, error) {
	err := opts.Ffx.TargetWait(ctx, "-t", "90")
	if err != nil {
		return nil, err
	}
	debuginfodServers := []string{}
	debuginfodCache := ""
	llvmVersion := ""
	if opts.LLVMProfdata != "" {
		llvmVersion, opts.LLVMProfdata = covargs.SplitVersion(opts.LLVMProfdata)
		opts.LLVMProfdata, err = filepath.Abs(opts.LLVMProfdata)
		if err != nil {
			return nil, err
		}
		debuginfodURLs := strings.TrimSpace(os.Getenv("DEBUGINFOD_URLS"))
		if debuginfodURLs != "" {
			debuginfodServers = strings.Split(debuginfodURLs, " ")
		}
		debuginfodCache = filepath.Join(opts.OutputDir, "debuginfod-cache")
	}
	return &FFXTester{
		ffx:               opts.Ffx,
		localOutputDir:    opts.OutputDir,
		experiments:       opts.Experiments,
		target:            opts.Target,
		testbedConfig:     opts.TestbedConfig,
		testRuns:          make(map[string]ffxTestRun),
		llvmProfdata:      opts.LLVMProfdata,
		llvmVersion:       llvmVersion,
		sinksPerTest:      make(map[string]runtests.DataSinkMap),
		debuginfodServers: debuginfodServers,
		debuginfodCache:   debuginfodCache,
		wg:                sync.WaitGroup{},
		errs:              []error{},
		errsMu:            sync.Mutex{},
	}, nil
}

func (t *FFXTester) SetupTest(ctx context.Context, test testsharder.Test) error {
	packageResolutionTimeout := defaultPackageResolutionTimeout
	if test.PackageResolutionTimeoutSecs > 0 {
		packageResolutionTimeout = time.Duration(test.PackageResolutionTimeoutSecs) * time.Second
	}
	packageResolutionCtx, cancel := context.WithTimeout(ctx, packageResolutionTimeout)
	defer cancel()
	if err := t.ffx.PackageResolve(packageResolutionCtx, strings.Split(test.PackageURL, "#")[0]); err != nil {
		if errors.Is(err, context.DeadlineExceeded) {
			// If we time out resolving the package, just log the error and continue to run the test.
			// Any blobs that haven't been resolved yet should be resolved as part of the test run.
			logger.Warningf(ctx, "failed to resolve package in %v: %s", packageResolutionTimeout, err)
		} else {
			return fmt.Errorf("failed to resolve package: %w", err)
		}
	}
	return nil
}

func (t *FFXTester) Test(ctx context.Context, test testsharder.Test, stdout, stderr io.Writer, outDir string) (*runtests.TestDetails, error) {
	return BaseTestResultFromTest(test), t.testWithFile(ctx, test, stdout, stderr, outDir)
}

// for testability
var ffxTesterReconnect = reconnect

func reconnect(t *FFXTester, ctx context.Context) error {
	return t.ffx.TargetWait(ctx, "-t", "90")
}

func (t *FFXTester) Reconnect(ctx context.Context) error {
	if err := ffxTesterReconnect(t, ctx); err == nil || againstDevice() {
		return err
	}
	if err := t.target.ResolveIP(); err != nil {
		return fmt.Errorf("failed to resolve IP: %w", err)
	}
	ipv6, err := t.target.IPv6()
	if err != nil {
		return fmt.Errorf("failed to get ipv6 addr: %w", err)
	}
	ipv4, err := t.target.IPv4()
	if err != nil {
		return fmt.Errorf("failed to get ipv4 addr: %w", err)
	}
	addr, err := targets.IPAddr(t.target)
	if err != nil {
		return fmt.Errorf("failed to get device addr: %w", err)
	}

	env := map[string]string{
		botanistconstants.DeviceAddrEnvKey: addr.String(),
		botanistconstants.IPv4AddrEnvKey:   ipv4.String(),
		botanistconstants.IPv6AddrEnvKey:   ipv6.String(),
	}
	for k, v := range env {
		if err := os.Setenv(k, v); err != nil {
			return fmt.Errorf("failed to reset env var %s to %s: %w", k, v, err)
		}
	}
	for i, info := range t.testbedConfig {
		targetInfo, ok := info.(targets.FuchsiaTestbedConfig)
		if !ok {
			continue
		}
		if targetInfo.Nodename == t.target.Nodename() {
			targetInfo.IPv4 = ""
			targetInfo.IPv6 = ""
			if ipv4 != nil {
				targetInfo.IPv4 = ipv4.String()
			}
			if ipv6 != nil {
				targetInfo.IPv6 = ipv6.String()
			}
			t.testbedConfig[i] = targetInfo
			testbedConfigPath := os.Getenv(botanistconstants.TestbedConfigEnvKey)
			if testbedConfigPath != "" {
				data, err := json.Marshal(t.testbedConfig)
				if err != nil {
					return err
				}
				logger.Debugf(ctx, "updating testbed config with new ip addresses")
				if err := os.WriteFile(testbedConfigPath, data, os.ModePerm); err != nil {
					return err
				}
			}
			break
		}
	}
	t.ffx.SetTarget(addr.String())
	return ffxTesterReconnect(t, ctx)
}

// testWithFile runs `ffx test` with -test-file and returns the test result.
func (t *FFXTester) testWithFile(ctx context.Context, test testsharder.Test, stdout, stderr io.Writer, outDir string) error {
	testDef := []build.TestListEntry{{
		Name:   test.PackageURL,
		Labels: []string{test.Label},
		Execution: build.ExecutionDef{
			Type:                     "fuchsia_component",
			ComponentURL:             test.PackageURL,
			TimeoutSeconds:           int(test.Timeout.Seconds()),
			Parallel:                 test.Parallel,
			MaxSeverityLogs:          test.LogSettings.MaxSeverity,
			MinSeverityLogs:          test.LogSettings.MinSeverity,
			TestFilters:              test.TestFilters,
			NoCasesEqualsSuccess:     test.NoCasesEqualsSuccess,
			Realm:                    test.Realm,
			CreateNoExceptionChannel: test.CreateNoExceptionChannel,
		},
		Tags: test.Tags,
	}}
	origStdout := t.ffx.Stdout()
	origStderr := t.ffx.Stderr()
	var buf bytes.Buffer
	stdout = io.MultiWriter(stdout, &buf)
	stderr = io.MultiWriter(stderr, &buf)
	t.ffx.SetStdoutStderr(stdout, stderr)
	defer t.ffx.SetStdoutStderr(origStdout, origStderr)

	extraArgs := []string{"--filter-ansi"}
	if t.experiments.Contains(botanist.UseFFXTestParallel) {
		extraArgs = append(extraArgs, "--experimental-parallel-execution", "8")
	}
	startTime := clock.Now(ctx)
	runResult, err := t.ffx.TestRun(ctx, build.TestList{Data: testDef, SchemaID: build.TestListSchemaIDExperimental}, outDir, extraArgs...)
	if runResult == nil && err == nil {
		err = fmt.Errorf("no test result was found")
	}
	t.testRuns[test.PackageURL] = ffxTestRun{runResult, buf.String(), clock.Now(ctx).Sub(startTime)}
	return err
}

func containsError(output string, errMsgs []string) (bool, string) {
	for _, msg := range errMsgs {
		if strings.Contains(output, msg) {
			return true, msg
		}
	}
	return false, ""
}

func (t *FFXTester) ProcessResult(ctx context.Context, test testsharder.Test, outDir string, testResult *runtests.TestDetails, err error) (*runtests.TestDetails, error) {
	finalTestResult := testResult
	testRun := t.testRuns[test.PackageURL]
	if testRun.result != nil {
		testOutDir := testRun.result.GetTestOutputDir()
		t.testOutDirs = append(t.testOutDirs, testOutDir)
		testResult, err = processTestResult(testRun.result, test, testRun.totalDuration, false)
		if !finalTestResult.IsMultipliedRun {
			t.wg.Add(1)
			go func() {
				defer t.wg.Done()
				// Merge profiles in the background. Use a separate context so that it doesn't get
				// canceled when the test's context gets canceled.
				l := logger.LoggerFromContext(ctx)
				mergeCtx := logger.WithLogger(context.Background(), l)
				mergeCtx, cancel := context.WithCancel(mergeCtx)
				defer cancel()
				t.errsMu.Lock()
				if err := t.getSinks(mergeCtx, testOutDir, t.sinksPerTest, true); err != nil {
					t.errs = append(t.errs, err)
				}
				t.errsMu.Unlock()
			}()
		}
	}
	if err != nil {
		finalTestResult.FailureReason = runtests.FailureReasonFromMessage(err.Error())
	} else if testResult == nil {
		finalTestResult.FailureReason = runtests.FailureReasonFromMessage("expected 1 test result, got none")
	} else {
		finalTestResult = testResult
	}
	ffxTag := build.TestTag{Key: "use_ffx", Value: "true"}
	finalTestResult.Tags = append(finalTestResult.Tags, ffxTag)
	for i, testCase := range finalTestResult.Cases {
		finalTestResult.Cases[i].Tags = append(testCase.Tags, ffxTag)
	}
	if finalTestResult.Status != runtests.TestSuccess {
		if ok, errMsg := containsError(testRun.output, []string{
			sshutilconstants.ProcessTerminatedMsg, ffxutilconstants.TimeoutReachingTargetMsg, ffxutilconstants.UnableToResolveAddressMsg}); ok {
			if err == nil {
				err = errors.New(errMsg)
			}
			return finalTestResult, connectionError{err}
		}
	}
	return finalTestResult, nil
}

func processTestResult(runResult *ffxutil.TestRunResult, test testsharder.Test, totalDuration time.Duration, removeProfiles bool) (*runtests.TestDetails, error) {
	testOutDir := runResult.GetTestOutputDir()
	suiteResults, err := runResult.GetSuiteResults()
	if err != nil {
		return nil, err
	}
	if len(suiteResults) != 1 {
		return nil, fmt.Errorf("expected 1 test result, got %d", len(suiteResults))
	}

	suiteResult := suiteResults[0]
	testResult := BaseTestResultFromTest(test)

	switch suiteResult.Outcome {
	case ffxutil.TestPassed:
		testResult.Status = runtests.TestSuccess
	case ffxutil.TestTimedOut:
		testResult.Status = runtests.TestAborted
	case ffxutil.TestNotStarted:
		testResult.Status = runtests.TestSkipped
	default:
		testResult.Status = runtests.TestFailure
	}
	testResult.Tags = append(testResult.Tags, build.TestTag{Key: "test_outcome", Value: suiteResult.Outcome})

	var suiteArtifacts []string
	var stdioPath string
	suiteArtifactDir := filepath.Join(testOutDir, suiteResult.ArtifactDir)
	for artifact, metadata := range suiteResult.Artifacts {
		if _, err := os.Stat(filepath.Join(suiteArtifactDir, artifact)); os.IsNotExist(err) {
			// Don't record artifacts that don't exist.
			continue
		}
		if metadata.ArtifactType == ffxutil.ReportType {
			// Copy the report log into the filename expected by infra.
			// TODO(https://fxbug.dev/42172530): Remove dependencies on this filename.
			absPath := filepath.Join(suiteArtifactDir, artifact)
			stdioPath = filepath.Join(suiteArtifactDir, runtests.TestOutputFilename)
			if err := os.Rename(absPath, stdioPath); err != nil {
				return testResult, err
			}
			suiteArtifacts = append(suiteArtifacts, runtests.TestOutputFilename)
		} else if metadata.ArtifactType != ffxutil.DebugType {
			suiteArtifacts = append(suiteArtifacts, artifact)
		}
	}
	testResult.OutputFiles = suiteArtifacts
	testResult.OutputDir = suiteArtifactDir

	var cases []runtests.TestCaseResult
	for _, testCase := range suiteResult.Cases {
		var status runtests.TestStatus
		switch testCase.Outcome {
		case ffxutil.TestPassed:
			status = runtests.TestSuccess
		case ffxutil.TestSkipped:
			status = runtests.TestSkipped
		default:
			status = runtests.TestFailure
		}

		var artifacts []string
		var failReason string
		testCaseArtifactDir := filepath.Join(testOutDir, testCase.ArtifactDir)
		for artifact, metadata := range testCase.Artifacts {
			if _, err := os.Stat(filepath.Join(testCaseArtifactDir, artifact)); os.IsNotExist(err) {
				// Don't record artifacts that don't exist.
				continue
			}
			// Get the failReason from the stderr log.
			// TODO(ihuh): The stderr log may contain unsymbolized logs.
			// Consider symbolizing them within ffx or testrunner.
			if metadata.ArtifactType == ffxutil.StderrType {
				stderrBytes, err := os.ReadFile(filepath.Join(testCaseArtifactDir, artifact))
				if err != nil {
					failReason = fmt.Sprintf("failed to read stderr for test case %s: %s", testCase.Name, err)
				} else {
					failReason = string(stderrBytes)
				}
			}
			artifacts = append(artifacts, artifact)
		}
		var failureReason *runtests.FailureReason
		if runtests.IsFailure(status) {
			failureReason = runtests.FailureReasonFromMessage(failReason)
		}
		cases = append(cases, runtests.TestCaseResult{
			DisplayName:   testCase.Name,
			CaseName:      testCase.Name,
			Status:        status,
			FailureReason: failureReason,
			Format:        "FTF",
			OutputFiles:   artifacts,
			OutputDir:     testCaseArtifactDir,
			Tags:          []build.TestTag{{Key: "test_outcome", Value: testCase.Outcome}},
		})
	}
	testResult.Cases = cases

	testResult.StartTime = time.UnixMilli(suiteResult.StartTime)
	testResult.EndTime = time.UnixMilli(suiteResult.StartTime + suiteResult.DurationMilliseconds)
	// Calculate overhead from running ffx test and add it to the recorded
	// test duration to more accurately capture the total duration of the test.
	overhead := totalDuration - testResult.Duration()
	testResult.EndTime = testResult.EndTime.Add(overhead)
	// The runResult's artifacts should contain a directory with the profiles from
	// component v2 tests along with a summary.json that lists the data sinks per test.
	// It should also contain a second directory with early boot data sinks.
	// TODO(https://fxbug.dev/42075455): Merge profiles on host when using ffx test. When using
	// run-test-suite, we can just remove the entire artifact directory because we'll
	// scp the profiles off the target at the end of the task instead.
	if removeProfiles {
		runArtifactDir := filepath.Join(testOutDir, runResult.ArtifactDir)
		if err := os.RemoveAll(runArtifactDir); err != nil {
			return testResult, err
		}
	}
	return testResult, nil
}

// UpdateOutputDir updates the output dir with the oldDir substring with the newDir.
// This should be called if the outputs are moved.
func (t *FFXTester) UpdateOutputDir(oldDir, newDir string) {
	for i, outDir := range t.testOutDirs {
		if strings.Contains(outDir, oldDir) {
			t.testOutDirs[i] = strings.ReplaceAll(outDir, oldDir, newDir)
			break
		}
	}
}

// RemoveAllEmptyOutputDirs cleans up the output dirs by removing all empty
// directories. This leaves the run_summary and suite_summaries for debugging.
func (t *FFXTester) RemoveAllEmptyOutputDirs() error {
	var errs []string
	for _, outDir := range t.testOutDirs {
		err := filepath.WalkDir(outDir, func(path string, d fs.DirEntry, err error) error {
			if err != nil {
				return err
			}
			if d.IsDir() {
				files, err := os.ReadDir(path)
				if err != nil {
					return err
				}
				if len(files) == 0 {
					if err := os.RemoveAll(path); err != nil {
						return fmt.Errorf("failed to remove %s: %s", path, err)
					}
					return filepath.SkipDir
				}
			}
			return nil
		})
		if err != nil {
			errs = append(errs, fmt.Sprintf("%v", err))
		}
	}
	return fmt.Errorf(strings.Join(errs, "; "))
}

func (t *FFXTester) Close() error {
	return nil
}

func (t *FFXTester) EnsureSinks(ctx context.Context, sinks []runtests.DataSinkMap, outputs *TestOutputs) error {
	defer func() {
		if err := os.RemoveAll(t.debuginfodCache); err != nil {
			logger.Debugf(ctx, "failed to remove debuginfod cache: %s", err)
		}
	}()
	// Wait for all goroutines that are merging profiles.
	t.wg.Wait()
	t.errsMu.Lock()
	if len(t.errs) > 0 {
		var finalErr error
		for _, err := range t.errs {
			finalErr = fmt.Errorf("%w; %w", finalErr, err)
		}
		return finalErr
	}
	sinksPerTest := t.sinksPerTest
	if err := t.getEarlyBootProfiles(ctx, sinksPerTest); err != nil {
		// OTA tests cause this command to fail, but aren't used for collecting
		// coverage anyway. If this fails, just log the error and continue
		// processing the rest of the data sinks.
		logger.Debugf(ctx, "failed to determine early boot data sinks: %s", err)
	}
	// If there were early boot sinks, record the "early_boot_sinks" test in the outputs
	// so that the test result can be updated with the early boot sinks.
	if _, ok := sinksPerTest[earlyBootSinksTestName]; ok {
		earlyBootSinksTest := &runtests.TestDetails{
			Name:   earlyBootSinksTestName,
			Status: runtests.TestSuccess,
		}
		outputs.Record(ctx, *earlyBootSinksTest)
	}
	if len(sinksPerTest) > 0 {
		outputs.updateDataSinks(sinksPerTest, "v2")
	}
	for _, sinkRef := range sinks {
		if len(sinkRef) > 0 {
			return fmt.Errorf("Found v1 sinks when there should be none: %v", sinks)
		}
	}
	return nil
}

func (t *FFXTester) getEarlyBootProfiles(ctx context.Context, sinksPerTest map[string]runtests.DataSinkMap) error {
	testOutDir := filepath.Join(t.localOutputDir, "early-boot-profiles")
	if err := os.MkdirAll(testOutDir, os.ModePerm); err != nil {
		return err
	}
	if err := t.ffx.TestEarlyBootProfile(ctx, testOutDir); err != nil {
		return err
	}
	return t.getSinks(ctx, testOutDir, sinksPerTest, false)
}

func (t *FFXTester) getSinks(ctx context.Context, testOutDir string, sinksPerTest map[string]runtests.DataSinkMap, ignoreEarlyBoot bool) error {
	runResult, err := ffxutil.GetRunResult(testOutDir)
	if err != nil {
		return err
	}
	runArtifactDir := filepath.Join(testOutDir, runResult.ArtifactDir)
	seen := make(map[string]struct{})
	startTime := clock.Now(ctx)

	// The new test_manager API moves the artifacts previously associated with the run to the
	// suite. Look for data sinks there.
	if len(runResult.Artifacts) == 0 && len(runResult.Suites) == 1 {
		suiteArtifactDir := filepath.Join(testOutDir, runResult.Suites[0].ArtifactDir)
		for artifact, meta := range runResult.Suites[0].Artifacts {
			artifactPath := filepath.Join(suiteArtifactDir, artifact)
			if meta.ArtifactType != ffxutil.DebugType {
				continue
			}
			if err := t.getSinksFromArtifactDir(ctx, artifactPath, sinksPerTest, seen, ignoreEarlyBoot); err != nil {
				return err
			}
		}
	}

	// The runResult's artifacts should contain a directory with the profiles from
	// component v2 tests along with a summary.json that lists the data sinks per test.
	// It should also contain a second directory with early boot data sinks.
	for artifact := range runResult.Artifacts {
		artifactPath := filepath.Join(runArtifactDir, artifact)
		if err := t.getSinksFromArtifactDir(ctx, artifactPath, sinksPerTest, seen, ignoreEarlyBoot); err != nil {
			return err
		}
	}
	copyDuration := clock.Now(ctx).Sub(startTime)
	if len(seen) > 0 {
		logger.Debugf(ctx, "copied %d data sinks in %s", len(seen), copyDuration)
	}
	return nil
}

func (t *FFXTester) getSinksFromArtifactDir(ctx context.Context, artifactDir string, sinksPerTest map[string]runtests.DataSinkMap, seen map[string]struct{}, ignoreEarlyBoot bool) error {
	// If the artifact dir contains a summary.json, parse it and copy all profiles to the
	// localOutputDir. Otherwise, copy all contents to the localOutputDir and record them
	// as early boot sinks.
	summaryPath := filepath.Join(artifactDir, runtests.TestSummaryFilename)
	f, err := os.Open(summaryPath)
	if os.IsNotExist(err) {
		if ignoreEarlyBoot {
			return nil
		}
		return t.getEarlyBootSinks(ctx, artifactDir, sinksPerTest, seen)
	}
	if err != nil {
		return err
	}
	defer f.Close()

	var summary runtests.TestSummary
	if err = json.NewDecoder(f).Decode(&summary); err != nil {
		return fmt.Errorf("failed to read test summary from %q: %w", summaryPath, err)
	}
	return t.getSinksPerTest(ctx, artifactDir, summary, sinksPerTest, seen)
}

// getSinksPerTest moves sinks from sinkDir to the localOutputDir and records
// the sinks in sinksPerTest.
func (t *FFXTester) getSinksPerTest(ctx context.Context, sinkDir string, summary runtests.TestSummary, sinksPerTest map[string]runtests.DataSinkMap, seen map[string]struct{}) error {
	for _, details := range summary.Tests {
		for i, sinks := range details.DataSinks {
			for j, sink := range sinks {
				if _, ok := seen[sink.File]; !ok {
					dest, err := t.moveProfileToOutputDir(ctx, sinkDir, sink.File, details.Name)
					if err != nil {
						return err
					}
					if dest != sink.File {
						details.DataSinks[i][j].File = dest
					}
					seen[sink.File] = struct{}{}
				}
			}
		}
		sinksPerTest[details.Name] = details.DataSinks
	}
	return nil
}

// getEarlyBootSinks moves the early boot sinks to the localOutputDir and records it with
// an "early_boot_sinks" test in sinksPerTest.
func (t *FFXTester) getEarlyBootSinks(ctx context.Context, sinkDir string, sinksPerTest map[string]runtests.DataSinkMap, seen map[string]struct{}) error {
	return filepath.WalkDir(sinkDir, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		// TODO(https://fxbug.dev/132081): Remove hardcoded check for logs once they
		// are moved to a separate location.
		if d.IsDir() || filepath.Ext(path) == ".log" {
			return nil
		}
		// Record the file as an early boot profile.
		sinkFile, err := filepath.Rel(sinkDir, path)
		if err != nil {
			return err
		}
		if _, ok := seen[path]; !ok {
			dest, err := t.moveProfileToOutputDir(ctx, sinkDir, sinkFile, earlyBootSinksTestName)
			if err != nil {
				return err
			}
			if dest != sinkFile {
				sinkFile = dest
			}
			seen[path] = struct{}{}
		}
		earlyBootSinks, ok := sinksPerTest[earlyBootSinksTestName]
		if !ok {
			earlyBootSinks = runtests.DataSinkMap{}
		}

		// The directory under sinkDir is named after the type of sinks it contains.
		sinkType := strings.Split(filepath.ToSlash(sinkFile), "/")[0]
		if _, ok := earlyBootSinks[sinkType]; !ok {
			earlyBootSinks[sinkType] = []runtests.DataSink{}
		}
		earlyBootSinks[sinkType] = append(earlyBootSinks[sinkType], runtests.DataSink{Name: sinkFile, File: sinkFile})

		sinksPerTest[earlyBootSinksTestName] = earlyBootSinks
		return nil
	})
}

// for testability
var mergeProfiles = covargs.MergeSameVersionProfiles

// moveProfileToOutputDir moves the profile from the sinkDir to the local output directory.
// If a profile of the same name already exists, then the two are merged.
func (t *FFXTester) moveProfileToOutputDir(ctx context.Context, sinkDir, sinkFile, testName string) (string, error) {
	profile := filepath.Join(sinkDir, sinkFile)
	destProfile := filepath.Join(t.localOutputDir, "v2", sinkFile)
	if _, err := os.Stat(destProfile); err == nil && t.llvmProfdata != "" {
		// Merge profiles.
		logger.Debugf(ctx, "merging profile %s to %s", profile, destProfile)
		if err := mergeProfiles(ctx, sinkDir, []string{destProfile, profile}, destProfile, t.llvmProfdata, t.llvmVersion, 0, t.debuginfodServers, t.debuginfodCache, nil); err != nil {
			logger.Debugf(ctx, "failed to merge profiles: %s", err)
			// TODO(https://fxbug.dev/368375861): Return err once missing build id issue is resolved.
			// TODO(https://fxbug.dev/374146495): Remove following code once issue is fixed.
			testName = url.PathEscape(strings.ReplaceAll(testName, ":", ""))
			dest := filepath.Join(filepath.Dir(sinkFile), testName, filepath.Base(sinkFile))
			if err := os.MkdirAll(filepath.Dir(filepath.Join(t.localOutputDir, "v2", dest)), os.ModePerm); err != nil {
				return sinkFile, err
			}
			if err := os.Rename(profile, filepath.Join(t.localOutputDir, "v2", dest)); err != nil {
				return sinkFile, err
			}
			return dest, nil
		}
		// Remove old profile.
		if err := os.Remove(profile); err != nil {
			return sinkFile, err
		}
	} else {
		if err := os.MkdirAll(filepath.Dir(destProfile), os.ModePerm); err != nil {
			return sinkFile, err
		}
		if err := os.Rename(profile, destProfile); err != nil {
			return sinkFile, err
		}
	}
	return sinkFile, nil
}

func (t *FFXTester) RunSnapshot(ctx context.Context, snapshotFile string) error {
	if snapshotFile == "" {
		return nil
	}
	startTime := clock.Now(ctx)
	err := retry.Retry(ctx, retry.WithMaxAttempts(retry.NewConstantBackoff(time.Second), maxReconnectAttempts), func() error {
		return t.ffx.Snapshot(ctx, t.localOutputDir, snapshotFile)
	}, nil)
	if err != nil {
		logger.Errorf(ctx, "%s: %s", constants.FailedToRunSnapshotMsg, err)
		// TODO(https://fxbug.dev/387497485): For debugging. Remove when issue is fixed.
		target := os.Getenv(botanistconstants.NodenameEnvKey)
		if err := t.ffx.Doctor(ctx); err != nil {
			logger.Errorf(ctx, "failed to run `ffx doctor`: %s", err)
		}
		if err := t.ffx.RunWithTarget(ctx, "target", "list", target); err != nil {
			logger.Errorf(ctx, "failed to run `ffx target list`: %s", err)
		}
		// Try capturing snapshot using target nodename.
		t.ffx.SetTarget(target)
		if err := t.ffx.Snapshot(ctx, t.localOutputDir, snapshotFile); err != nil {
			logger.Errorf(ctx, "%s: %s", constants.FailedToRunSnapshotMsg, err)
		}
	}
	logger.Debugf(ctx, "ran snapshot in %s", clock.Now(ctx).Sub(startTime))
	return err
}

// for testability
type socketConn interface {
	io.ReadWriteCloser
	SetIOTimeout(timeout time.Duration)
}

// FuchsiaSerialTester executes fuchsia tests over serial.
type FuchsiaSerialTester struct {
	socket socketConn
}

// NewFuchsiaSerialTester creates a tester that runs tests over serial.
func NewFuchsiaSerialTester(ctx context.Context, serialSocketPath string) (Tester, error) {
	socket, err := serial.NewSocket(ctx, serialSocketPath)
	if err != nil {
		return nil, err
	}

	// Test logs get interleaved with other serial logs and while most can be parsed
	// out by the parseOutKernelReader, that assumes the log starts with a timestamp
	// which some of the early logs at bootup don't have. For that reason, try to
	// wait until most of those logs are written to serial before starting to run tests.
	// In the case that the search string was already written before we started searching
	// for it, don't fail if we time out looking for it.
	socket.SetIOTimeout(30 * time.Second)
	startedCtx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()
	if _, err := iomisc.ReadUntilMatchString(startedCtx, socket, "[driver_manager.cm] INFO: Bootup completed"); err != nil {
		logger.Debugf(ctx, "%s", err)
	}
	return &FuchsiaSerialTester{socket: socket}, nil
}

func (t *FuchsiaSerialTester) SetupTest(_ context.Context, _ testsharder.Test) error {
	return nil
}

// Exposed for testability.
var newTestStartedContext = func(ctx context.Context) (context.Context, context.CancelFunc) {
	return context.WithTimeout(ctx, testStartedTimeout)
}

// lastWriteSaver is an io.Writer that saves the bytes written in the last Write().
type lastWriteSaver struct {
	buf []byte
}

func (w *lastWriteSaver) Write(p []byte) (int, error) {
	w.buf = make([]byte, len(p))
	copy(w.buf, p)
	return len(p), nil
}

// parseOutKernelReader is an io.Reader that reads from the underlying reader
// everything not pertaining to a kernel log. A kernel log is distinguished by
// a line that starts with the timestamp represented as a float inside brackets.
type parseOutKernelReader struct {
	ctx    context.Context
	reader io.Reader
	// unprocessed stores the last characters read from a Read() but not returned
	// by it. This could happen if we read more than necessary to try to complete
	// a possible kernel log and cannot return all of the bytes. This will be
	// read in the next call to Read().
	unprocessed []byte
	// kernelLineStart stores the last characters read from a Read() block if it
	// ended with a truncated line and possibly contains a kernel log. This will
	// be prepended to the next Read() block.
	kernelLineStart []byte
	reachedEOF      bool
}

func (r *parseOutKernelReader) Read(buf []byte) (int, error) {
	// If the underlying reader already reached EOF, that means kernelLineStart is
	// not the start of a kernel log, so append it to unprocessed to be read normally.
	if r.reachedEOF {
		r.unprocessed = append(r.unprocessed, r.kernelLineStart...)
		r.kernelLineStart = []byte{}
	}
	// If there are any unprocessed bytes, read them first instead of calling the
	// underlying reader's Read() again.
	if len(r.unprocessed) > 0 {
		bytesToRead := int(math.Min(float64(len(buf)), float64(len(r.unprocessed))))
		copy(buf, r.unprocessed[:bytesToRead])
		r.unprocessed = r.unprocessed[bytesToRead:]
		return bytesToRead, nil
	} else if r.reachedEOF {
		// r.unprocessed was empty so we can just return EOF.
		return 0, io.EOF
	}

	if r.ctx.Err() != nil {
		return 0, r.ctx.Err()
	}

	b := make([]byte, len(buf))
	type readResult struct {
		n   int
		err error
	}
	ch := make(chan readResult, 1)
	// Call the underlying reader's Read() in a goroutine so that we can
	// break out if the context is canceled.
	go func() {
		readN, readErr := r.reader.Read(b)
		ch <- readResult{readN, readErr}
	}()
	var n int
	var err error
	select {
	case res := <-ch:
		n = res.n
		err = res.err
		break
	case <-r.ctx.Done():
		err = r.ctx.Err()
	}

	if err != nil && err != io.EOF {
		return n, err
	}
	// readBlock contains everything stored in kernelLineStart (bytes last read
	// from the underlying reader in the previous Read() call that possibly contain
	// a truncated kernel log that has not been processed by this reader yet) along
	// with the new bytes just read. Because readBlock contains unprocessed bytes,
	// its length will likely be greater than len(buf).
	// However, it is necessary to read more bytes in the case that the unprocessed
	// bytes contain a long truncated kernel log and we need to keep reading more
	// bytes until we get to the end of the line so we can discard it.
	readBlock := append(r.kernelLineStart, b[:n]...)
	r.kernelLineStart = []byte{}
	lines := bytes.Split(readBlock, []byte("\n"))
	var bytesRead, bytesLeftToRead int
	for i, line := range lines {
		bytesLeftToRead = len(buf) - bytesRead
		isTruncated := i == len(lines)-1
		line = r.lineWithoutKernelLog(line, isTruncated)
		if bytesLeftToRead == 0 {
			// If there are no more bytes left to read, store the rest of the lines
			// into r.unprocessed to be read at the next call to Read().
			r.unprocessed = append(r.unprocessed, line...)
			continue
		}
		if len(line) > bytesLeftToRead {
			// If the line is longer than bytesLeftToRead, read as much as possible
			// and store the rest in r.unprocessed.
			copy(buf[bytesRead:], line[:bytesLeftToRead])
			r.unprocessed = line[bytesLeftToRead:]
			bytesRead += bytesLeftToRead
		} else {
			copy(buf[bytesRead:bytesRead+len(line)], line)
			bytesRead += len(line)
		}
	}
	if err == io.EOF {
		r.reachedEOF = true
	}
	if len(r.unprocessed)+len(r.kernelLineStart) > 0 {
		err = nil
	}
	return bytesRead, err
}

func (r *parseOutKernelReader) lineWithoutKernelLog(line []byte, isTruncated bool) []byte {
	containsKernelLog := false
	re := regexp.MustCompile(`\[[0-9]+\.?[0-9]+\]`)
	match := re.FindIndex(line)
	if match != nil {
		if isTruncated {
			r.kernelLineStart = line[match[0]:]
		}
		// The new line to add to bytes read contains everything in the line up to
		// the bracket indicating the kernel log.
		line = line[:match[0]]
		containsKernelLog = true
	} else if isTruncated {
		// Match the beginning of a possible kernel log timestamp.
		// i.e. `[`, `[123` `[123.4`
		re = regexp.MustCompile(`\[[0-9]*\.?[0-9]*$`)
		match = re.FindIndex(line)
		if match != nil {
			r.kernelLineStart = line[match[0]:]
			line = line[:match[0]]
		}
	}
	if !containsKernelLog && !isTruncated {
		line = append(line, '\n')
	}
	return line
}

func (t *FuchsiaSerialTester) Test(ctx context.Context, test testsharder.Test, stdout, _ io.Writer, _ string) (*runtests.TestDetails, error) {
	testResult := BaseTestResultFromTest(test)
	command, err := commandForTest(&test, true, test.Timeout)
	if err != nil {
		testResult.FailureReason = runtests.FailureReasonFromMessage(err.Error())
		return testResult, nil
	}
	logger.Debugf(ctx, "starting: %s", command)

	// TODO(https://fxbug.dev/42167818): Currently, serial output is coming out jumbled,
	// so the started string sometimes comes after the completed string, resulting
	// in a timeout because we fail to read the completed string after the
	// started string. Uncomment below to use the lastWriteSaver once the bug is
	// fixed.
	var lastWrite bytes.Buffer
	// If a single read from the socket includes both the bytes that indicate the test started and the bytes
	// that indicate the test completed, then the startedReader will consume the bytes needed for detecting
	// completion. Thus we save the last read from the socket and replay it when searching for completion.
	// lastWrite := &lastWriteSaver{}
	t.socket.SetIOTimeout(testStartedTimeout)
	reader := io.TeeReader(t.socket, &lastWrite)
	commandStarted := false

	startedStr := runtests.StartedSignature + test.Name
	if test.PackageURL != "" {
		// Packaged tests run with the "run-test-suite" tool which has different logs.
		startedStr = "Running test '" + test.PackageURL + "'"
	}

	var readErr error
	for i := 0; i < startSerialCommandMaxAttempts; i++ {
		if err := serial.RunCommands(ctx, t.socket, []serial.Command{{Cmd: command}}); err != nil {
			return nil, fmt.Errorf("failed to write to serial socket: %w", err)
		}
		startedCtx, cancel := newTestStartedContext(ctx)
		_, readErr = iomisc.ReadUntilMatchString(startedCtx, reader, startedStr)
		cancel()
		if readErr == nil {
			commandStarted = true
			break
		} else if errors.Is(readErr, startedCtx.Err()) {
			logger.Warningf(ctx, "test not started after timeout")
		} else {
			logger.Errorf(ctx, "unexpected error checking for test start signature: %s", readErr)
		}
	}
	if !commandStarted {
		err = fmt.Errorf("%s within %d attempts: %w",
			constants.FailedToStartSerialTestMsg, startSerialCommandMaxAttempts, readErr)
		// In practice, repeated failure to run a test means that the device has
		// become unresponsive and we won't have any luck running later tests.
		return nil, err
	}

	t.socket.SetIOTimeout(test.Timeout + 30*time.Second)
	testOutputReader := io.TeeReader(
		// See comment above lastWrite declaration.
		&parseOutKernelReader{ctx: ctx, reader: io.MultiReader(&lastWrite, t.socket)},
		// Writes to stdout as it reads from the above reader.
		stdout)

	if test.PackageURL != "" {
		// The test was ran with the "run-test-suite" tool, parse the result.
		res_success := test.PackageURL + " completed with result: PASSED"
		res_failed := test.PackageURL + " completed with result: FAILED"
		res_inconclusive := test.PackageURL + " completed with result: INCONCLUSIVE"
		res_timed_out := test.PackageURL + " completed with result: TIMED_OUT"
		res_errored := test.PackageURL + " completed with result: ERROR"
		res_skipped := test.PackageURL + " completed with result: SKIPPED"
		res_canceled := test.PackageURL + " completed with result: CANCELLED"
		res_dnf := test.PackageURL + " completed with result: DID_NOT_FINISH"
		match, err := iomisc.ReadUntilMatchString(ctx, testOutputReader, res_success, res_failed, res_inconclusive, res_timed_out, res_errored, res_skipped, res_canceled, res_dnf)
		if err != nil {
			err = fmt.Errorf("unable to derive test result from run-test-suite output: %w", err)
			testResult.FailureReason = runtests.FailureReasonFromMessage(err.Error())
			return testResult, nil
		}

		if match == res_success {
			testResult.Status = runtests.TestSuccess
			return testResult, nil
		}

		if match == res_timed_out || match == res_canceled {
			testResult.FailureReason = runtests.FailureReasonFromMessage("test timed out or canceled")
			testResult.Status = runtests.TestAborted
			return testResult, nil
		}

		if match == res_skipped {
			testResult.Status = runtests.TestSkipped
			return testResult, nil
		}

		logger.Errorf(ctx, "%s", match)
		testResult.FailureReason = runtests.FailureReasonFromMessage("test failed")
		testResult.Status = runtests.TestFailure
		return testResult, nil
	}

	if success, err := runtests.TestPassed(ctx, testOutputReader, test.Name); err != nil {
		testResult.FailureReason = runtests.FailureReasonFromMessage(err.Error())
		return testResult, nil
	} else if !success {
		if errors.Is(err, io.EOF) {
			// EOF indicates that serial has become disconnected. That is
			// unlikely to be caused by this test and we're unlikely to be able
			// to keep running tests.
			return nil, err
		}
		testResult.FailureReason = runtests.FailureReasonFromMessage("test failed")
		return testResult, nil
	}
	testResult.Status = runtests.TestSuccess
	return testResult, nil
}

func (t *FuchsiaSerialTester) ProcessResult(ctx context.Context, test testsharder.Test, outDir string, testResult *runtests.TestDetails, err error) (*runtests.TestDetails, error) {
	return testResult, err
}

func (t *FuchsiaSerialTester) EnsureSinks(_ context.Context, _ []runtests.DataSinkMap, _ *TestOutputs) error {
	return nil
}

func (t *FuchsiaSerialTester) RunSnapshot(_ context.Context, _ string) error {
	return nil
}

func (t *FuchsiaSerialTester) Reconnect(_ context.Context) error {
	return nil
}

// Close terminates the underlying Serial socket connection. The object is no
// longer usable after calling this method.
func (t *FuchsiaSerialTester) Close() error {
	return t.socket.Close()
}

func commandForTest(test *testsharder.Test, useSerial bool, timeout time.Duration) ([]string, error) {
	command := []string{}
	if useSerial && test.PackageURL == "" {
		// `runtests` is used to run tests over serial when there is no PackageURL.
		command = []string{runtestsName}
		if timeout > 0 {
			command = append(command, "-i", fmt.Sprintf("%d", int64(timeout.Seconds())))
		}
		if test.Path != "" {
			command = append(command, test.Path)
		} else {
			return nil, fmt.Errorf("Path is not set for %q", test.Name)
		}
	} else if test.PackageURL != "" {
		if test.IsComponentV2() {
			command = []string{runTestSuiteName, "--filter-ansi"}
			if test.Realm != "" {
				command = append(command, "--realm", fmt.Sprintf("%s", test.Realm))
			}
			if test.LogSettings.MaxSeverity != "" {
				command = append(command, "--max-severity-logs", fmt.Sprintf("%s", test.LogSettings.MaxSeverity))
			}
			if test.LogSettings.MinSeverity != "" {
				command = append(command, "--min-severity-logs", fmt.Sprintf("%s", test.LogSettings.MinSeverity))
			}
			if test.Parallel != 0 {
				command = append(command, "--parallel", fmt.Sprintf("%d", test.Parallel))
			}
			// TODO(https://fxbug.dev/42126211): Once fixed, combine timeout flag setting for v1 and v2.
			if timeout > 0 {
				command = append(command, "--timeout", fmt.Sprintf("%d", int64(timeout.Seconds())))
			}
			if test.CreateNoExceptionChannel {
				command = append(command, "--no-exception-channel")
			}
		} else {
			return nil, fmt.Errorf("CFv1 tests are no longer supported: %q", test.PackageURL)
		}
		command = append(command, test.PackageURL)
	} else {
		return nil, fmt.Errorf("PackageURL is not set and useSerial is false for %q", test.Name)
	}
	return command, nil
}
