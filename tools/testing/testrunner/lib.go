// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package testrunner

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"log"
	"net"
	"net/url"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"

	botanist "go.fuchsia.dev/fuchsia/tools/botanist"
	botanistconstants "go.fuchsia.dev/fuchsia/tools/botanist/constants"
	"go.fuchsia.dev/fuchsia/tools/botanist/targets"
	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/integration/testsharder"
	"go.fuchsia.dev/fuchsia/tools/lib/clock"
	"go.fuchsia.dev/fuchsia/tools/lib/environment"
	"go.fuchsia.dev/fuchsia/tools/lib/ffxutil"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
	"go.fuchsia.dev/fuchsia/tools/lib/retry"
	"go.fuchsia.dev/fuchsia/tools/lib/subprocess"
	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
	"go.fuchsia.dev/fuchsia/tools/testing/tap"
	"go.fuchsia.dev/fuchsia/tools/testing/testparser"
	"go.fuchsia.dev/fuchsia/tools/testing/testrunner/constants"
)

const testTimeoutGracePeriod = 30 * time.Second

type Options struct {
	// The path where a directory containing test results should be created.
	OutDir string

	// Working directory of the local testing subprocesses.
	LocalWD string

	// The path to an NsJail binary.
	NsjailPath string

	// The path to mount as NsJail's root directory.
	NsjailRoot string

	// The output filename for the snapshot. This will be created in the outDir.
	SnapshotFile string

	// Whether to use serial to run tests on the target.
	UseSerial bool

	// The ffx instance to use.
	FFX *ffxutil.FFXInstance

	// The experiments to enable.
	//
	// See //tools/botanist/cmd/run.go for supported experiments.
	Experiments botanist.Experiments

	// The path to the llvm-profdata binary to use to merge profiles on the host.
	// If given as `<path>=<version>`, the version should correspond to the version
	// of the profiles produced by the tests that are run.")
	LLVMProfdataPath string

	// The Fuchsia target to run tests against.
	FuchsiaTarget targets.FuchsiaTarget

	// The testbed config written to the FUCHSIA_TESTBED_CONFIG env var.
	TestbedConfig []any
}

// ScaleTestTimeout multiplies the timeout by a factor set by the TEST_TIMEOUT_SCALE_FACTOR
// environment variable. This allows us to scale timeouts on environments where
// processes run much slower than others.
func ScaleTestTimeout(timeout time.Duration) time.Duration {
	if n, err := strconv.Atoi(os.Getenv(constants.TestTimeoutScaleFactor)); err == nil {
		return timeout * time.Duration(n)
	}
	return timeout
}

func SetupAndExecute(ctx context.Context, opts Options, testsPath string) error {
	// Our mDNS library doesn't use the logger library.
	const logFlags = log.Ltime | log.Lmicroseconds | log.Lshortfile
	log.SetFlags(logFlags)

	tests, err := loadTests(testsPath)
	if err != nil {
		return fmt.Errorf("failed to load tests from %q: %w", testsPath, err)
	}

	// Configure a test outputs object, responsible for producing TAP output,
	// recording data sinks, and archiving other test outputs.
	testOutDir := filepath.Join(os.Getenv(constants.TestOutDirEnvKey), opts.OutDir)
	if testOutDir == "" {
		var err error
		testOutDir, err = os.MkdirTemp("", "testrunner")
		if err != nil {
			return fmt.Errorf("failed to create a test output directory")
		}
	}
	logger.Debugf(ctx, "test output directory: %s", testOutDir)

	var addr net.IPAddr
	if deviceAddr, ok := os.LookupEnv(botanistconstants.DeviceAddrEnvKey); ok {
		addrPtr, err := net.ResolveIPAddr("ip", deviceAddr)
		if err != nil {
			return fmt.Errorf("failed to parse device address %s: %w", deviceAddr, err)
		}
		addr = *addrPtr
	}
	sshKeyFile := os.Getenv(botanistconstants.SSHKeyEnvKey)
	serialSocketPath := os.Getenv(botanistconstants.SerialSocketEnvKey)
	// If the TestOutDirEnvKey was set, that means testrunner is being run
	// in an infra setting and thus needs an isolated environment.
	_, needsIsolatedEnv := os.LookupEnv(constants.TestOutDirEnvKey)
	// However, if testrunner is called from botanist, it doesn't need to set
	// up its own isolated environment because botanist will already have
	// done that. Botanist will set either the sshKeyFile or serialSocketPath,
	// so if neither are set, then testrunner was NOT called from botanist
	// and thus needs its own isolated environment.
	if needsIsolatedEnv {
		needsIsolatedEnv = sshKeyFile == "" && serialSocketPath == ""
	}
	cleanUp, err := environment.Ensure(needsIsolatedEnv)
	if err != nil {
		return fmt.Errorf("failed to setup environment: %w", err)
	}
	defer cleanUp()

	tapProducer := tap.NewProducer(os.Stdout)
	tapProducer.Plan(len(tests))
	outputs, err := CreateTestOutputs(tapProducer, testOutDir)
	if err != nil {
		return fmt.Errorf("failed to create test outputs: %w", err)
	}

	execErr := execute(ctx, tests, outputs, addr, sshKeyFile, serialSocketPath, testOutDir, opts)
	if err := outputs.Close(); err != nil {
		if execErr == nil {
			return err
		}
		logger.Warningf(ctx, "Failed to save test outputs: %s", err)
	}
	return execErr
}

// for testability
var (
	serialTester = NewFuchsiaSerialTester
)

// ffxInstance takes a *ffxutil.FFXInstance and returns it as a FFXInstance
// interface. This is used for testability so that we can return a mock
// instance instead.
var ffxInstance = func(
	ctx context.Context,
	ffxInstance *ffxutil.FFXInstance,
	experiments botanist.Experiments,
	t targets.FuchsiaTarget,
) (FFXInstance, error) {
	ffx, err := func() (FFXInstance, error) {
		if ffxInstance == nil {
			// Return nil instead of ffx so that the returned FFXInstance
			// will be the nil interface instead of an interface holding
			// a nil value. In the latter case, checking ffx == nil will
			// return false.
			return nil, nil
		}
		// Create a copy of the ffx instance to use for testing
		// so that we can set some config settings only when running
		// ffx test.
		ffxInstance = ffxutil.FFXWithTarget(ffxInstance, ffxInstance.GetTarget())
		if err := ffxInstance.ConfigSet(ctx, "ssh.controlmaster.mode", "explicit"); err != nil {
			return ffxInstance, err
		}
		if err := ffxInstance.ConfigSet(ctx, "ssh.controlmaster.path", t.SSHControlMasterPath()); err != nil {
			return ffxInstance, err
		}
		if experiments.Contains(botanist.UseFFXTestParallel) {
			if err := ffxInstance.ConfigSet(ctx, "test.enable_experimental_parallel_execution", "true"); err != nil {
				return ffxInstance, err
			}
		}
		// Print the config for debugging purposes.
		// TODO(ihuh): Remove when not needed.
		if err := ffxInstance.GetConfig(ctx); err != nil {
			return ffxInstance, err
		}
		return ffxInstance, nil
	}()
	return ffx, err
}

func execute(
	ctx context.Context,
	tests []testsharder.Test,
	outputs *TestOutputs,
	addr net.IPAddr,
	sshKeyFile,
	serialSocketPath,
	outDir string,
	opts Options,
) error {
	var fuchsiaSinks, localSinks []runtests.DataSinkMap
	var fuchsiaTester, localTester Tester
	var reconnectSSH func() (func(), error)

	localEnv := append(os.Environ(),
		// Tell tests written in Rust to print stack on failures.
		"RUST_BACKTRACE=1",
	)
	var err error
	testerOpts := SubprocessTesterOptions{
		Dir:        opts.LocalWD,
		Env:        localEnv,
		OutputDir:  outputs.OutDir,
		NsjailPath: opts.NsjailPath,
		NsjailRoot: opts.NsjailRoot,
		Target:     opts.FuchsiaTarget,
	}
	localTester, err = NewSubprocessTester(testerOpts)
	if err != nil {
		return err
	}

	if !opts.UseSerial && sshKeyFile != "" {
		ffx, err := ffxInstance(ctx, opts.FFX, opts.Experiments, opts.FuchsiaTarget)
		if err != nil {
			return err
		}
		if ffx != nil {
			testerOpts := FFXTesterOptions{
				Ffx:           ffx,
				OutputDir:     outputs.OutDir,
				Experiments:   opts.Experiments,
				LLVMProfdata:  opts.LLVMProfdataPath,
				Target:        opts.FuchsiaTarget,
				TestbedConfig: opts.TestbedConfig,
			}
			ffxTester, err := NewFFXTester(ctx, testerOpts)
			if err != nil {
				return fmt.Errorf("failed to initialize ffx tester: %w", err)
			}
			defer func() {
				// TODO(https://fxbug.dev/42075455): Once profiles are being merged on the host, this
				// will leave empty artifact directories within each test's output directories
				// from where the profiles originated from. Clean up empty directories.
				if err := ffxTester.RemoveAllEmptyOutputDirs(); err != nil {
					logger.Debugf(ctx, "%s", err)
				}
			}()
			fuchsiaTester = ffxTester
			reconnectSSH = func() (func(), error) {
				cleanup, err := restartSSHControlMaster(ctx, opts.FuchsiaTarget, fuchsiaTester, sshKeyFile)
				if err == nil && localTester != nil {
					logger.Debugf(ctx, "updating local env")
					localTester.(*SubprocessTester).UpdateEnv(append(os.Environ(), "RUST_BACKTRACE=1"))
				}
				return cleanup, err
			}
		}
	}

	// Function to select the tester to use for a test, along with destination
	// for the test to write any data sinks. This logic is not easily testable
	// because it requires a lot of network requests and environment inspection,
	// so we use dependency injection and pass it as a parameter to
	// `runAndOutputTests` to make that function more easily testable.
	testerForTest := func(test testsharder.Test) (Tester, *[]runtests.DataSinkMap, error) {
		switch test.OS {
		case "fuchsia":
			if fuchsiaTester == nil {
				var err error
				if serialSocketPath == "" {
					return nil, nil, fmt.Errorf("%q must be set if %q is not set", botanistconstants.SerialSocketEnvKey, botanistconstants.SSHKeyEnvKey)
				}
				fuchsiaTester, err = serialTester(ctx, serialSocketPath)
				if err != nil {
					return nil, nil, fmt.Errorf("failed to initialize fuchsia tester: %w", err)
				}
			}
			return fuchsiaTester, &fuchsiaSinks, nil
		case "linux", "mac":
			if test.OS == "linux" && runtime.GOOS != "linux" {
				return nil, nil, fmt.Errorf("cannot run linux tests when GOOS = %q", runtime.GOOS)
			}
			if test.OS == "mac" && runtime.GOOS != "darwin" {
				return nil, nil, fmt.Errorf("cannot run mac tests when GOOS = %q", runtime.GOOS)
			}
			return localTester, &localSinks, nil
		default:
			return nil, nil, fmt.Errorf("test %#v has unsupported OS: %q", test, test.OS)
		}
	}

	// filter tests if a testAllowlist is present in the environment variables.
	filteredTests, err := filterTests(tests)
	if err != nil {
		return fmt.Errorf("failed to filter tests using test allowlist: %w", err)
	}

	var finalError error
	cleanup, err := runAndOutputTests(ctx, filteredTests, testerForTest, outputs, outDir, fuchsiaTester, reconnectSSH, opts.FuchsiaTarget, opts.Experiments)
	if err != nil {
		finalError = err
	}
	defer cleanup()

	if fuchsiaTester != nil {
		defer fuchsiaTester.Close()
	}
	if localTester != nil {
		defer localTester.Close()
	}
	finalize := func(t Tester, sinks []runtests.DataSinkMap) error {
		if t != nil {
			cleanupCtx := ctx
			if ctx.Err() != nil {
				// Run snapshot and copy the sinks with a new context so we can still complete
				// these tasks even if we hit a timeout. The timeout should be long enough to
				// complete the command and short enough to fit within the cleanupGracePeriod
				// in //tools/lib/subprocess/subprocess.go.
				var cancel context.CancelFunc
				cleanupCtx, cancel = context.WithTimeout(context.Background(), 7*time.Second)
				defer cancel()
			}
			if err := t.RunSnapshot(cleanupCtx, opts.SnapshotFile); err != nil {
				// This error usually has a different root cause that gets masked when we
				// return this error. Log it so we can keep track of it, but don't fail.
				logger.Errorf(cleanupCtx, err.Error())
			}
			if err := t.EnsureSinks(cleanupCtx, sinks, outputs); err != nil {
				// Same story, log it but don't fail.
				logger.Errorf(cleanupCtx, err.Error())
			}
			if ctx.Err() != nil {
				// If the original context was cancelled, just return the context error.
				return ctx.Err()
			}
		}
		return nil
	}

	// If we lose the SSH connection after the last test, we should try to reconnect
	// so that we can run the final steps.
	useFFXUSB := againstDevice() && opts.Experiments.Contains(botanist.ForceFFXUSB)
	if _, ok := fuchsiaTester.(*FFXTester); ok && !useFFXUSB && sshKeyFile != "" && !opts.FuchsiaTarget.SSHControlMasterRunning() {
		cleanup()
		cleanup, err = restartSSHControlMaster(ctx, opts.FuchsiaTarget, fuchsiaTester, sshKeyFile)
		if err != nil && finalError == nil {
			finalError = err
		}
		defer cleanup()
	}

	if err := finalize(localTester, localSinks); err != nil && finalError == nil {
		finalError = err
	}
	if err := finalize(fuchsiaTester, fuchsiaSinks); err != nil && finalError == nil {
		finalError = err
	}
	return finalError
}

func filterTests(tests []testsharder.Test) ([]testsharder.Test, error) {
	// Get a list of tests to run from environment variables set by the recipe.
	var testAllowlist []string
	allowlistLengthStr, exists := os.LookupEnv(botanistconstants.TestAllowlistLengthEnvKey)
	if exists {
		allowlistLength, err := strconv.Atoi(allowlistLengthStr)
		if err != nil {
			return nil, fmt.Errorf("failed to convert %s to int: %w", botanistconstants.TestAllowlistLengthEnvKey, err)
		}
		for i := 0; i < allowlistLength; i++ {
			allowedTest, exists := os.LookupEnv(fmt.Sprintf(botanistconstants.TestAllowlistIndexEnvKeyTemplate, i))
			if !exists || allowedTest == "" {
				return nil, fmt.Errorf("failed to find test for environment variable %s", fmt.Sprintf(botanistconstants.TestAllowlistIndexEnvKeyTemplate, i))
			}
			testAllowlist = append(testAllowlist, allowedTest)
		}
	}
	if len(testAllowlist) == 0 {
		return tests, nil
	}

	filteredTests := []testsharder.Test{}
	testNames := make(map[string]struct{})
	for _, test := range tests {
		testNames[test.Name] = struct{}{}
		if slices.Contains(testAllowlist, test.Name) {
			filteredTests = append(filteredTests, test)
		}
	}
	for _, testName := range testAllowlist {
		if _, exists := testNames[testName]; !exists {
			return nil, fmt.Errorf("test %q from allowlist not found in the test list", testName)
		}
	}

	return filteredTests, nil
}

func validateTest(test testsharder.Test) error {
	if test.Name == "" {
		return fmt.Errorf("one or more tests missing `name` field")
	}
	if test.OS == "" {
		return fmt.Errorf("one or more tests missing `os` field")
	}
	if test.Runs <= 0 {
		return fmt.Errorf("one or more tests with invalid `runs` field")
	}
	if test.Runs > 1 {
		switch test.RunAlgorithm {
		case testsharder.KeepGoing, testsharder.StopOnFailure, testsharder.StopOnSuccess:
		default:
			return fmt.Errorf("one or more tests with invalid `run_algorithm` field")
		}
	}
	if test.OS == "fuchsia" && test.PackageURL == "" && test.Path == "" {
		return fmt.Errorf("one or more fuchsia tests missing the `path` and `package_url` fields")
	}
	if test.OS != "fuchsia" {
		if test.Path == "" {
			return fmt.Errorf("one or more host tests missing the `path` field")
		}
	}
	return nil
}

func loadTests(path string) ([]testsharder.Test, error) {
	bytes, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("failed to read %q: %w", path, err)
	}

	var tests []testsharder.Test
	if err := json.Unmarshal(bytes, &tests); err != nil {
		return nil, fmt.Errorf("failed to unmarshal %q: %w", path, err)
	}

	for _, test := range tests {
		if err := validateTest(test); err != nil {
			return nil, err
		}
	}

	return tests, nil
}

func restartSSHControlMaster(ctx context.Context, target targets.FuchsiaTarget, fuchsiaTester Tester, sshKeyFile string) (func(), error) {
	cleanup := func() {}
	err := fuchsiaTester.Reconnect(ctx)
	if err != nil {
		return cleanup, fmt.Errorf("failed to reconnect to target: %w", err)
	}
	logger.Debugf(ctx, "restarting ssh controlmaster")
	cleanup, err = target.SetupSSHControlMaster(ctx, sshKeyFile, os.Getenv(botanistconstants.DeviceAddrEnvKey))
	if err != nil {
		// If we fail to restart the ssh controlmaster, tests will have to create
		// a new SSH connection, but they'll still be able to run, so just log
		// the error instead of failing.
		logger.Errorf(ctx, "failed to restart ssh controlmaster: %s", err)
	}
	return cleanup, nil
}

// testToRun represents an entry in a queue of tests to run.
type testToRun struct {
	testsharder.Test
	// The number of times the test has already been run.
	previousRuns int
	// The sum of the durations of all the test's previous runs.
	totalDuration        time.Duration
	collectOutputsOnPass bool
}

// runAndOutputTests runs all the tests, possibly with retries, and records the
// results to `outputs`.
func runAndOutputTests(
	ctx context.Context,
	tests []testsharder.Test,
	testerForTest func(testsharder.Test) (Tester, *[]runtests.DataSinkMap, error),
	outputs *TestOutputs,
	globalOutDir string,
	fuchsiaTester Tester,
	reconnectSSH func() (func(), error),
	target targets.FuchsiaTarget,
	experiments botanist.Experiments,
) (func(), error) {
	cleanup := func() {}
	// Since only a single goroutine writes to and reads from the queue it would
	// be more appropriate to use a true Queue data structure, but we'd need to
	// implement that ourselves so it's easier to just use a channel. Make the
	// channel double the necessary size just to be safe and avoid potential
	// deadlocks.
	testQueue := make(chan testToRun, 2*len(tests))

	for _, test := range tests {
		testQueue <- testToRun{Test: test, collectOutputsOnPass: true}
	}

	// `for test := range testQueue` might seem simpler, but it would block
	// instead of exiting once the queue becomes empty. To exit the loop we
	// would need to close the channel when it became empty. That would require
	// a length check within the loop body anyway, and it's more robust to put
	// the length check in the for loop condition.
	testIndex := 0
	shouldRunHealthCheck := false
	for len(testQueue) > 0 {
		if shouldRunHealthCheck && fuchsiaTester != nil {
			if err := runHealthCheck(ctx, fuchsiaTester); err != nil {
				// Device is in a bad state and cannot run any more tests,
				// so fail and return early.
				return cleanup, fmt.Errorf("failed to run health check: %w", err)
			}
			shouldRunHealthCheck = false
		}
		useFFXUSB := againstDevice() && experiments.Contains(botanist.ForceFFXUSB)
		if _, ok := fuchsiaTester.(*FFXTester); ok && !useFFXUSB && !target.SSHControlMasterRunning() {
			cleanup()
			var err error
			if cleanup, err = reconnectSSH(); err != nil {
				return cleanup, err
			}
		}
		test := <-testQueue

		t, sinks, err := testerForTest(test.Test)
		if err != nil {
			return cleanup, err
		}

		var result *runtests.TestDetails
		var outDir string
		if err := retryOnConnectionFailure(ctx, t, func() error {
			runIndex := test.previousRuns

			outDir = filepath.Join(globalOutDir, url.PathEscape(strings.ReplaceAll(test.Name, ":", "")), strconv.Itoa(runIndex))
			var testErr error
			result, testErr = runTestOnce(ctx, test.Test, t, outDir, testIndex, test.collectOutputsOnPass)
			if result == nil {
				return testErr
			}
			result.RunIndex = runIndex

			// TODO(ihuh): Increase the runs so that we keep the outputs of all runs
			// in the task outputs, but temporarily do not record the test failure if
			// it's a connection failure so that we can correlate the connection failures
			// better through a tefmocheck.
			test.previousRuns++
			test.totalDuration += result.Duration()
			return testErr
		}); err != nil {
			return cleanup, err
		}

		if err := outputs.Record(ctx, *result); err != nil {
			return cleanup, fmt.Errorf("failed to record %v: %w", result, err)
		}
		testIndex++

		if !result.Passed() {
			shouldRunHealthCheck = true
		}
		if shouldKeepGoing(test.Test, result, test.totalDuration) {
			// Schedule the test to be run again.
			if result.Passed() {
				test.collectOutputsOnPass = false
			}
			testQueue <- test
		}
		// TODO(olivernewman): Add a unit test to make sure data sinks are
		// recorded correctly.
		*sinks = append(*sinks, result.DataSinks)
	}
	return cleanup, nil
}

type connectionError struct {
	error
}

func isConnectionError(err error) bool {
	var connErr connectionError
	return errors.As(err, &connErr)
}

var connectionErrorRetryBackoff retry.Backoff = retry.NewConstantBackoff(time.Second)

func retryOnConnectionFailure(ctx context.Context, t Tester, execFunc func() error) error {
	return retry.Retry(ctx, retry.WithMaxAttempts(connectionErrorRetryBackoff, maxReconnectAttempts), func() error {
		err := execFunc()
		if isConnectionError(err) {
			logger.Errorf(ctx, "attempting to reconnect after error: %s", err)
			if reconnectErr := t.Reconnect(ctx); reconnectErr != nil {
				logger.Errorf(ctx, "%s", reconnectErr)
				// If we fail to reconnect, continuing is likely hopeless.
				// Return the *original* error (which will generally be more
				// closely related to the root cause of the failure) rather than
				// the reconnection error.
				return retry.Fatal(err)
			}
			return err
		}
		return retry.Fatal(err)
	}, nil)
}

func againstDevice() bool {
	return (os.Getenv(botanistconstants.NodenameEnvKey) != targets.DefaultEmulatorNodename &&
		os.Getenv(botanistconstants.NodenameEnvKey) != "")
}

func runHealthCheck(ctx context.Context, t Tester) error {
	if err := t.Reconnect(ctx); err == nil {
		return nil
	} else if !againstDevice() {
		// Power-cycling doesn't apply for emulators
		// so just return the error if we fail to reconnect.
		return err
	}
	r := &subprocess.Runner{Env: os.Environ()}
	if err := setPowerState(ctx, r, "cycle"); err != nil {
		return fmt.Errorf("failed to power cycle target: %w", err)
	}
	return t.Reconnect(ctx)
}

func setPowerState(ctx context.Context, r *subprocess.Runner, state string) error {
	cmd := []string{
		os.Getenv(constants.DMCPathEnvKey),
		"set-power-state",
		"--nodename",
		os.Getenv(botanistconstants.NodenameEnvKey),
		"--state",
		state,
	}
	return r.Run(ctx, cmd, subprocess.RunOptions{Setpgid: true})
}

// shouldKeepGoing returns whether we should schedule another run of the test.
// It'll return true if we haven't yet exceeded the time limit for reruns, or
// if the most recent test run didn't meet the stop condition for this test.
func shouldKeepGoing(test testsharder.Test, lastResult *runtests.TestDetails, testTotalDuration time.Duration) bool {
	stopRepeatingDuration := time.Duration(test.StopRepeatingAfterSecs) * time.Second
	if stopRepeatingDuration > 0 && testTotalDuration >= stopRepeatingDuration {
		return false
	} else if test.Runs > 0 && lastResult.RunIndex+1 >= test.Runs {
		return false
	} else if test.RunAlgorithm == testsharder.StopOnSuccess && lastResult.Passed() {
		return false
	} else if test.RunAlgorithm == testsharder.StopOnFailure && !lastResult.Passed() {
		return false
	}
	return true
}

// stdioBuffer is a simple thread-safe wrapper around bytes.Buffer. It
// implements the io.Writer interface.
type stdioBuffer struct {
	// Used to protect access to `buf`.
	mu sync.Mutex

	// The underlying buffer.
	buf bytes.Buffer
}

func (b *stdioBuffer) Write(p []byte) (n int, err error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buf.Write(p)
}

// runTestOnce runs the given test once. It will not return an error if the test
// fails, only if an unrecoverable error occurs or testing should otherwise stop.
func runTestOnce(
	ctx context.Context,
	origTest testsharder.Test,
	t Tester,
	outDir string,
	testIndex int,
	collectOutputsOnPass bool,
) (*runtests.TestDetails, error) {
	// The test case parser specifically uses stdout, so we need to have a
	// dedicated stdout buffer.
	stdoutForParsing := new(bytes.Buffer)
	stdio := new(stdioBuffer)

	stdout, stderr, flush := botanist.NewStdioWriters(ctx, fmt.Sprintf("test%d", testIndex))
	defer flush()
	multistdout := io.MultiWriter(stdout, stdio, stdoutForParsing)
	multistderr := io.MultiWriter(stderr, stdio)

	// In the case of running tests on QEMU over serial, we do not wish to
	// forward test output to stdout, as QEMU is already redirecting serial
	// output there: we do not want to double-print.
	//
	// This is a bit of a hack, but is a lesser evil than extending the
	// testrunner CLI just to sidecar the information of 'is QEMU'.
	againstQEMU := os.Getenv(botanistconstants.NodenameEnvKey) == targets.DefaultEmulatorNodename
	if _, ok := t.(*FuchsiaSerialTester); ok && againstQEMU {
		multistdout = io.MultiWriter(stdio, stdoutForParsing)
	}

	test := origTest
	test.Timeout = ScaleTestTimeout(test.Timeout)
	logger.Debugf(ctx, "test timeout: %v, orig timeout: %v\n", test.Timeout, origTest.Timeout)

	startTime := clock.Now(ctx)

	if err := t.SetupTest(ctx, test); err != nil {
		result := BaseTestResultFromTest(test)
		result.Status = runtests.TestFailure
		result.FailureReason = runtests.FailureReasonFromMessage(fmt.Sprintf("failed to setup test: %s", err))
		logger.Errorf(ctx, "Test %s failed: %s", test.Name, err)
		result.StartTime = startTime
		result.EndTime = clock.Now(ctx)
		result.Affected = test.Affected
		return result, nil
	}

	// Set the outer timeout to a slightly higher value in order to give the tester
	// time to handle the timeout itself.  Other steps such as retrying tests over
	// serial or fetching data sink references may also cause the Test() method to
	// exceed the test's timeout, so we give enough time for the tester to
	// complete those steps as well.
	outerTestTimeout := test.Timeout + testTimeoutGracePeriod

	var timeoutCh <-chan time.Time
	if test.Timeout > 0 {
		// Intentionally call After(), thereby resolving a completion deadline,
		// *before* starting to run the test. This helps avoid race conditions
		// in this function's unit tests that advance the fake clock's time
		// within the `t.Test()` call.
		timeoutCh = clock.After(ctx, outerTestTimeout)
	}
	// Else, timeoutCh will be nil. Receiving from a nil channel blocks forever,
	// so no timeout will be enforced, which is what we want.

	type testResult struct {
		result *runtests.TestDetails
		err    error
	}
	ch := make(chan testResult, 1)

	// We don't use context.WithTimeout() because it uses the real time.Now()
	// instead of clock.Now(), which makes it much harder to simulate timeouts
	// in this function's unit tests.
	testCtx, cancelTest := context.WithCancel(ctx)
	defer cancelTest()
	// Run the test in a goroutine so that we don't block in case the tester fails
	// to respect the timeout.
	go func() {
		result, err := t.Test(testCtx, test, multistdout, multistderr, outDir)
		ch <- testResult{result, err}
	}()

	result := BaseTestResultFromTest(test)
	// In the case of a timeout, store whether it hit the inner or outer test
	// timeout.
	var timeout time.Duration
	var err error
	select {
	case res := <-ch:
		if res.result != nil {
			res.result.IsMultipliedRun = !collectOutputsOnPass
			result, err = t.ProcessResult(testCtx, test, outDir, res.result, res.err)
		} else {
			result, err = nil, res.err
		}
		timeout = test.Timeout
	case <-timeoutCh:
		result.Status = runtests.TestAborted
		timeout = outerTestTimeout
		cancelTest()
	}

	if err != nil && !isConnectionError(err) {
		// The tester encountered a fatal condition and cannot run any more
		// tests.
		return nil, err
	}
	if result == nil {
		return nil, err
	}
	if !result.Passed() && ctx.Err() != nil {
		// testrunner is shutting down, give up running tests and don't
		// report this test result as it may have been impacted by the
		// context cancelation.
		return nil, ctx.Err()
	}

	switch result.Status {
	case runtests.TestFailure:
		logger.Errorf(ctx, "Test %s failed: %v", test.Name, result.FailureReason)
	case runtests.TestAborted:
		logger.Errorf(ctx, "Test %s timed out after %s", test.Name, timeout)
	}

	endTime := clock.Now(ctx)
	// The start time and end time should cover the entire duration to run the test
	// and process the results.
	result.StartTime = startTime
	result.EndTime = endTime
	result.Affected = test.Affected

	if result.Passed() && !collectOutputsOnPass {
		if err := os.RemoveAll(outDir); err != nil {
			return nil, err
		}
		result.OutputFiles = []string{}
		result.OutputDir = ""
		multipliedTag := build.TestTag{Key: "is_multiplied_run", Value: "true"}
		result.Tags = append(result.Tags, multipliedTag)
		for i, tc := range result.Cases {
			tc.OutputFiles = []string{}
			tc.OutputDir = ""
			tc.Tags = append(tc.Tags, multipliedTag)
			result.Cases[i] = tc
		}
		return result, nil
	}
	// Record the test details in the summary.
	result.Stdio = stdio.buf.Bytes()
	// Only the FFXTester handles output files on its own. It also handles cases
	// so only parse the stdout for test cases if both are missing.
	// parse the stdout for test cases and check the outdir for output files.
	if len(result.Cases) == 0 && len(result.OutputFiles) == 0 {
		cases, err := testparser.Parse(stdoutForParsing.Bytes())
		if err != nil {
			result.Status = runtests.TestFailure
			logger.Errorf(ctx, "Failed to parse test cases: %s", err)
			if result.FailureReason == nil {
				result.FailureReason = runtests.FailureReasonFromMessage(fmt.Sprintf("failed to parse test cases: %s", err))
			}
		} else {
			result.Cases = cases
		}
	} else {
		// TODO(b/311443213): Some tests rely on testparser to add tags to test case results.
		// Remove this hack once tags are properly handled by `ffx test`.
		cases, err := testparser.Parse(stdoutForParsing.Bytes())
		if err != nil && len(result.Cases) == 0 {
			result.Status = runtests.TestFailure
			logger.Errorf(ctx, "Failed to parse test cases: %s", err)
			if result.FailureReason == nil {
				result.FailureReason = runtests.FailureReasonFromMessage(fmt.Sprintf("failed to parse test cases: %s", err))
			}
		} else if err == nil {
			caseToTags := make(map[string][]build.TestTag)
			for _, tc := range cases {
				caseToTags[tc.DisplayName] = tc.Tags
			}
			for i, tc := range result.Cases {
				result.Cases[i].Tags = append(result.Cases[i].Tags, caseToTags[tc.DisplayName]...)
			}
		}
	}
	// Check the outdir for output files if not handled by the tester.
	if len(result.OutputFiles) == 0 {
		caseOutputFiles := []string{}
		for _, tc := range result.Cases {
			for _, of := range tc.OutputFiles {
				caseOutputFiles = append(caseOutputFiles, filepath.Join(tc.OutputDir, of))
			}
		}

		if err := filepath.WalkDir(outDir, func(path string, d fs.DirEntry, err error) error {
			if err != nil {
				return err
			}
			if d.IsDir() {
				return nil
			}
			relPath, err := filepath.Rel(outDir, path)
			if err != nil {
				return err
			}
			// Don't include the file if it's already recorded as a test case output file.
			if !strings.Contains(strings.Join(caseOutputFiles, " "), path) {
				result.OutputFiles = append(result.OutputFiles, relPath)
			}
			return nil
		}); err != nil && !os.IsNotExist(err) {
			logger.Debugf(ctx, "unable to record output files: %s", err)
		}
		if len(result.OutputFiles) > 0 {
			result.OutputDir = outDir
		}
	}

	return result, err
}
