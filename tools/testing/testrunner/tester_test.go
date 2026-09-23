// Copyright 2019 The Fuchsia Authors. All rights reserved.
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
	"net"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/google/go-cmp/cmp"
	"github.com/google/go-cmp/cmp/cmpopts"

	"go.fuchsia.dev/fuchsia/tools/botanist"
	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/integration/testsharder"
	"go.fuchsia.dev/fuchsia/tools/integration/testsharder/metadata"
	"go.fuchsia.dev/fuchsia/tools/lib/ffxutil"
	ffxutilconstants "go.fuchsia.dev/fuchsia/tools/lib/ffxutil/constants"
	"go.fuchsia.dev/fuchsia/tools/lib/iomisc"
	"go.fuchsia.dev/fuchsia/tools/lib/retry"
	"go.fuchsia.dev/fuchsia/tools/lib/subprocess"
	sshutilconstants "go.fuchsia.dev/fuchsia/tools/net/sshutil/constants"
	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
)

type fakeSSHClient struct {
	reconnectErrs  []error
	reconnectCalls int
	runErrs        []error
	runCalls       int
	lastCmd        []string
	shuttingDown   chan struct{}
}

func (c *fakeSSHClient) Run(_ context.Context, command []string, _, _ io.Writer) error {
	c.runCalls++
	c.lastCmd = command
	if c.runErrs == nil {
		return nil
	}
	err, remainingErrs := c.runErrs[0], c.runErrs[1:]
	c.runErrs = remainingErrs
	return err
}

func (c *fakeSSHClient) Close() {
	if c.shuttingDown != nil {
		close(c.shuttingDown)
	}
}

func (c *fakeSSHClient) DisconnectionListener() <-chan struct{} {
	if c.shuttingDown == nil {
		c.shuttingDown = make(chan struct{})
	}
	return c.shuttingDown
}

func (c *fakeSSHClient) ReconnectWithBackoff(_ context.Context, _ retry.Backoff) error {
	c.reconnectCalls++
	if c.reconnectErrs == nil {
		return nil
	}
	err, remainingErrs := c.reconnectErrs[0], c.reconnectErrs[1:]
	c.reconnectErrs = remainingErrs
	return err
}

type fakeSerialClient struct {
	runCalls int
}

func (c *fakeSerialClient) runDiagnostics(_ context.Context) error {
	c.runCalls++
	return nil
}

type fakeCmdRunner struct {
	runErrs  []error
	runCalls int
	lastCmd  []string
}

func (r *fakeCmdRunner) Run(_ context.Context, command []string, _ subprocess.RunOptions) error {
	r.runCalls++
	r.lastCmd = command
	if r.runErrs == nil {
		return nil
	}
	err, remainingErrs := r.runErrs[0], r.runErrs[1:]
	r.runErrs = remainingErrs
	return err
}

func TestSubprocessTester(t *testing.T) {
	tmpDir := t.TempDir()
	passingTest := filepath.Join("host_x64", "passing")
	passingProfile := filepath.Join("llvm-profile", passingTest, "default.profraw")
	failingTest := filepath.Join("host_x64", "failing")
	failingProfile := filepath.Join("llvm-profile", failingTest, "default.profraw")
	for _, profile := range []string{passingProfile, failingProfile} {
		abs := filepath.Join(tmpDir, profile)
		os.MkdirAll(filepath.Dir(abs), 0o700)
		f, err := os.Create(abs)
		if err != nil {
			t.Fatalf("failed to create profile: %s", err)
		}
		f.Close()
	}
	// Override tempdir creation function to ensure deterministic
	// command output.
	newTempDir = func(dir, pattern string) (string, error) {
		return "/newtmp", nil
	}

	cases := []struct {
		name           string
		test           build.Test
		runErrs        []error
		expectedStatus runtests.TestStatus
		useSandboxing  bool
		env            map[string]string
		wantCmd        []string
		wantDataSinks  runtests.DataSinkMap
	}{
		{
			name:           "no path",
			test:           build.Test{},
			expectedStatus: runtests.TestFailure,
		},
		{
			name:           "test passes with profile",
			test:           build.Test{Path: passingTest},
			expectedStatus: runtests.TestSuccess,
			wantCmd:        []string{"./" + passingTest},
			wantDataSinks: runtests.DataSinkMap{
				"llvm-profile": []runtests.DataSink{
					{
						Name: filepath.Base(passingProfile),
						File: passingProfile,
					},
				},
			},
		},
		{
			name:          "test passes with profile and host test sandboxing",
			test:          build.Test{Path: passingTest},
			useSandboxing: true,
			env: map[string]string{
				llvmProfileEnvKey: "fake/llvm/profile/path/p.profraw",
			},
			expectedStatus: runtests.TestSuccess,
			wantCmd: []string{
				"./fake_nsjail",
				"--disable_clone_newcgroup",
				"--disable_clone_newnet",
				"--quiet",
				"--bindmount_ro",
				"/bin:/bin",
				"--bindmount_ro",
				"/dev/kvm:/dev/kvm",
				"--bindmount_ro",
				"/dev/net/tun:/dev/net/tun",
				"--bindmount",
				"/dev/null:/dev/null",
				"--bindmount_ro",
				"/dev/urandom:/dev/urandom",
				"--bindmount_ro",
				"/dev/zero:/dev/zero",
				"--bindmount_ro",
				"/etc/alternatives/awk:/etc/alternatives/awk",
				"--bindmount_ro",
				"/etc/host.conf:/etc/host.conf",
				"--bindmount_ro",
				"/etc/hosts:/etc/hosts",
				"--bindmount_ro",
				"/etc/nsswitch.conf:/etc/nsswitch.conf",
				"--bindmount_ro",
				"/etc/passwd:/etc/passwd",
				"--bindmount_ro",
				"/etc/resolv.conf:/etc/resolv.conf",
				"--bindmount_ro",
				"/etc/ssl/certs:/etc/ssl/certs",
				"--bindmount_ro",
				"/usr/share/ca-certificates:/usr/share/ca-certificates",
				"--bindmount_ro",
				"/lib:/lib",
				"--bindmount_ro",
				"/lib64:/lib64",
				"--bindmount",
				"/newtmp:/tmp",
				"--bindmount",
				fmt.Sprintf("%s:%s", tmpDir, tmpDir),
				"--bindmount",
				fmt.Sprintf("%s/host_x64/passing:%s/host_x64/passing", tmpDir, tmpDir),
				"--bindmount_ro",
				"/usr/bin:/usr/bin",
				"--bindmount_ro",
				"/usr/lib:/usr/lib",
				"--bindmount_ro",
				"/usr/share/misc/magic.mgc:/usr/share/misc/magic.mgc",
				"--bindmount",
				"/dev/shm:/dev/shm",
				"--bindmount_ro",
				"/usr/share/tcltk:/usr/share/tcltk",
				"--bindmount_ro",
				"/usr/share/vulkan:/usr/share/vulkan",
				"--symlink",
				"/proc/self/fd:/dev/fd",
				"--rlimit_as",
				"soft",
				"--rlimit_fsize",
				"soft",
				"--rlimit_nofile",
				"soft",
				"--rlimit_nproc",
				"soft",
				"--env",
				"ANDROID_TMP=/tmp",
				"--env",
				"FFX_CONFIGS=",
				"--env",
				fmt.Sprintf("FUCHSIA_TEST_OUTDIR=%s/host_x64/passing", tmpDir),
				"--env",
				"HOME=/tmp",
				"--env",
				fmt.Sprintf("LLVM_PROFILE_FILE=%s/llvm-profile/host_x64/passing/%%m.profraw", tmpDir),
				"--env",
				"TEMP=/tmp",
				"--env",
				"TEMPDIR=/tmp",
				"--env",
				fmt.Sprintf("TEST_OUTPUT_SUMMARY_PATH=%s/test_output_summary/host_x64/passing/test_output_summary_path.json", tmpDir),
				"--env",
				"TMP=/tmp",
				"--env",
				"TMPDIR=/tmp",
				"--env",
				"XDG_CACHE_HOME=/tmp",
				"--env",
				"XDG_CONFIG_HOME=/tmp",
				"--env",
				"XDG_DATA_HOME=/tmp",
				"--env",
				"XDG_HOME=/tmp",
				"--env",
				"XDG_STATE_HOME=/tmp",
				"--",
				"./" + passingTest,
			},
			wantDataSinks: runtests.DataSinkMap{
				"llvm-profile": []runtests.DataSink{
					{
						Name: filepath.Base(passingProfile),
						File: passingProfile,
					},
				},
			},
		},
		{
			name:           "test passes without profile",
			test:           build.Test{Path: "uninstrumented_test"},
			expectedStatus: runtests.TestSuccess,
			wantCmd:        []string{"./uninstrumented_test"},
			wantDataSinks:  nil,
		},
		{
			name:           "test fails",
			test:           build.Test{Path: failingTest},
			runErrs:        []error{fmt.Errorf("test failed")},
			expectedStatus: runtests.TestFailure,
			wantCmd:        []string{"./" + failingTest},
			wantDataSinks: runtests.DataSinkMap{
				"llvm-profile": []runtests.DataSink{
					{
						Name: filepath.Base(failingProfile),
						File: failingProfile,
					},
				},
			},
		},
	}

	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			runner := &fakeCmdRunner{
				runErrs: c.runErrs,
			}
			tester := SubprocessTester{
				localOutputDir: tmpDir,
				testRuns:       make(map[string]string),
			}
			if c.useSandboxing {
				tester.sProps = &sandboxingProps{
					nsjailPath: "./fake_nsjail",
				}
				// Override the environment to make sure we have a deterministic
				// command.
				prevEnv := os.Environ()
				prev := make(map[string]string)
				for _, entry := range prevEnv {
					parts := strings.SplitN(entry, "=", 2)
					os.Unsetenv(parts[0])
					prev[parts[0]] = parts[1]
				}
				for k, v := range c.env {
					os.Setenv(k, v)
				}
				// Restore the environment after the test case.
				t.Cleanup(func() {
					for k := range c.env {
						os.Unsetenv(k)
					}
					for k, v := range prev {
						os.Setenv(k, v)
					}
				})
			}
			newRunner = func(dir string, env []string) cmdRunner {
				return runner
			}
			outDir := filepath.Join(tmpDir, c.test.Path)
			ctx := context.Background()
			test := testsharder.Test{Test: c.test}
			if err := tester.SetupTest(ctx, test); err != nil {
				t.Errorf("tester.SetupTest got error: %s, want nil", err)
			}
			testResult, err := tester.Test(ctx, test, io.Discard, io.Discard, outDir)
			testResult, err = tester.ProcessResult(ctx, test, outDir, testResult, err)
			if err != nil {
				t.Errorf("tester.Test got error: %s, want nil", err)
			}
			if c.expectedStatus != testResult.Status {
				t.Errorf("tester.Test got result: %s, want: %s", testResult.Status, c.expectedStatus)
			}
			if _, statErr := os.Stat(outDir); statErr != nil {
				t.Error("tester.Test did not create a readable outDir:", statErr)
			}
			var opts []cmp.Option
			// We sort the cmd when using sandboxing because the mounts are
			// ordered alphabetically.
			if c.useSandboxing {
				opts = append(opts, cmpopts.SortSlices(func(s, t string) bool {
					return s < t
				}))
			}
			if diff := cmp.Diff(c.wantCmd, runner.lastCmd, opts...); diff != "" {
				t.Errorf("Unexpected command run (-want +got):\n%s", diff)
			}

			sinks := testResult.DataSinks
			if diff := cmp.Diff(c.wantDataSinks, sinks); diff != "" {
				t.Errorf("Diff in data sinks (-want +got):\n%s", diff)
			}
		})
	}
}

type fakeDataSinkCopier struct {
	reconnectCalls int
	remoteDirs     map[string]struct{}
}

func (c *fakeDataSinkCopier) GetAllDataSinks(remoteDir string) ([]runtests.DataSink, error) {
	c.remoteDirs[remoteDir] = struct{}{}
	return []runtests.DataSink{{Name: "sink", File: filepath.Join("sink_type", "sink")}}, nil
}

func (c *fakeDataSinkCopier) GetReferences(remoteDir string) (map[string]runtests.DataSinkMap, error) {
	c.remoteDirs[remoteDir] = struct{}{}
	return map[string]runtests.DataSinkMap{}, nil
}

func (*fakeDataSinkCopier) Copy(_ []runtests.DataSinkMap, _ string) (runtests.DataSinkMap, error) {
	return runtests.DataSinkMap{}, nil
}

func (*fakeDataSinkCopier) RemoveAll(_ string) error {
	return nil
}

func (c *fakeDataSinkCopier) Reconnect() error {
	c.reconnectCalls++
	return nil
}

func (*fakeDataSinkCopier) Close() error {
	return nil
}

func TestFFXTester(t *testing.T) {
	cases := []struct {
		name           string
		expectedStatus runtests.TestStatus
		connErr        bool
		experiments    []string
		output         string
	}{
		{
			name:           "run v2 tests with ffx",
			expectedStatus: runtests.TestSuccess,
			experiments:    []string{"use_ffx_test", "use_ffx_test_parallel"},
		},
		{
			name:           "ffx test fails",
			expectedStatus: runtests.TestFailure,
			experiments:    []string{"use_ffx_test"},
		},
		{
			name:           "ffx test times out",
			expectedStatus: runtests.TestAborted,
			experiments:    []string{"use_ffx_test"},
		},
		{
			name:           "ffx test skipped",
			expectedStatus: runtests.TestSkipped,
			experiments:    []string{"use_ffx_test"},
		},
		{
			name:           "ffx test returns ssh connection failure",
			expectedStatus: runtests.TestFailure,
			connErr:        true,
			experiments:    []string{"use_ffx_test"},
			output:         sshutilconstants.ProcessTerminatedMsg + "\n" + ffxutilconstants.ClientChannelClosedMsg,
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			var outcome string
			switch c.expectedStatus {
			case runtests.TestSuccess:
				outcome = ffxutil.TestPassed
			case runtests.TestFailure:
				outcome = ffxutil.TestFailed
			case runtests.TestAborted:
				outcome = ffxutil.TestTimedOut
			case runtests.TestSkipped:
				outcome = ffxutil.TestNotStarted
			}
			ffx := &ffxutil.MockFFXInstance{TestOutcome: outcome, Output: c.output}
			localOutputDir := t.TempDir()
			experiments := botanist.GetExperiments(c.experiments)
			tester, err := NewFFXTester(context.Background(), FFXTesterOptions{Ffx: ffx, OutputDir: localOutputDir, Experiments: experiments})
			if err != nil {
				t.Fatalf("NewFFXTester got unexpected error: %s", err)
			}

			defer func() {
				if err := tester.Close(); err != nil {
					t.Errorf("Close returned error: %s", err)
				}
			}()

			test := testsharder.Test{
				Test:         build.Test{PackageURL: "fuchsia-pkg://foo#meta/bar.cm"},
				Runs:         1,
				RunAlgorithm: testsharder.StopOnSuccess,
			}
			ctx := context.Background()
			outDir := t.TempDir()
			if err := tester.SetupTest(ctx, test); err != nil {
				t.Errorf("tester.SetupTest got err: %s, want nil", err)
			}
			testResult, err := tester.Test(ctx, test, io.Discard, io.Discard, outDir)
			testResult, err = tester.ProcessResult(ctx, test, outDir, testResult, err)
			if !ffx.ContainsCmd("resolve", strings.Split(test.PackageURL, "#")[0]) {
				t.Errorf("failed to call `ffx target package resolve`, called: %s", ffx.CmdsCalled)
			}
			if c.connErr && !isConnectionError(err) {
				t.Errorf("tester.Test got err: %s, want conn err", err)
			} else if !c.connErr && err != nil {
				t.Errorf("tester.Test got unexpected error: %s", err)
			}
			if testResult.Status != c.expectedStatus {
				t.Errorf("tester.Test got result: %s, want result: %s", testResult.Status, c.expectedStatus)
			}

			testArgs := []string{}
			if experiments.Contains(botanist.UseFFXTestParallel) {
				testArgs = append(testArgs, "--experimental-parallel-execution", "8")
			}
			if !ffx.ContainsCmd("test", testArgs...) {
				t.Errorf("failed to call `ffx test`, called: %s", ffx.CmdsCalled)
			}
			numRuns := strings.Count(strings.Join(ffx.CmdsCalled, " "), "test:")
			if numRuns != 1 {
				t.Errorf("called `ffx test` %d times, expected 1", numRuns)
			}
			expectedCaseStatus := runtests.TestSuccess
			if c.expectedStatus != runtests.TestSuccess {
				expectedCaseStatus = runtests.TestFailure
			}
			if len(testResult.Cases) != 1 {
				t.Errorf("expected 1 test case, got %d", len(testResult.Cases))
			} else {
				if testResult.Cases[0].Status != expectedCaseStatus {
					t.Errorf("test case has status: %s, want: %s", testResult.Cases[0].Status, expectedCaseStatus)
				}
			}
			p := filepath.Join(t.TempDir(), "testrunner-cmd-test")
			if err = tester.RunSnapshot(ctx, p); err != nil {
				t.Errorf("failed to run snapshot: %s", err)
			}
			if !ffx.ContainsCmd("snapshot") {
				t.Errorf("failed to call `ffx target snapshot`, called: %s", ffx.CmdsCalled)
			}

			// Write the early-boot profiles so EnsureSinks() can find them.
			if _, err := ffx.WriteRunResult(build.TestList{}, filepath.Join(localOutputDir, "early-boot-profiles")); err != nil {
				t.Errorf("failed to write early-boot profiles: %s", err)
			}
			// Call EnsureSinks() for v2 tests to set the copier.remoteDir to the data output dir for v2 tests.
			// v1 tests will already have set the appropriate remoteDir value within Test().
			outputs := &TestOutputs{OutDir: t.TempDir()}
			if err = tester.EnsureSinks(ctx, []runtests.DataSinkMap{testResult.DataSinks}, outputs); err != nil {
				t.Errorf("failed to collect sinks: %s", err)
			}
			foundEarlyBootSinks := false
			for _, test := range outputs.Summary.Tests {
				if test.Name == "early_boot_sinks" {
					foundEarlyBootSinks = true
					if len(test.DataSinks["llvm-profile"]) != 1 {
						t.Errorf("got %d early boot sinks, want 1", len(test.DataSinks["llvm-profile"]))
					}
					break
				}
			}
			if !foundEarlyBootSinks {
				t.Errorf("failed to find early boot sinks")
			}
		})
	}

	t.Run("profile merging failed", func(t *testing.T) {
		ctx := context.Background()
		localOutputDir := t.TempDir()
		tester, err := NewFFXTester(ctx, FFXTesterOptions{Ffx: &ffxutil.MockFFXInstance{}, OutputDir: localOutputDir, LLVMProfdata: "llvm-profdata"})
		if err != nil {
			t.Fatalf("NewFFXTester got unexpected error: %s", err)
		}
		oldMergeProfiles := mergeProfiles
		defer func() {
			mergeProfiles = oldMergeProfiles
		}()
		mergeProfiles = func(_ context.Context, _ string, _ []string, _, _, _ string, _ int, _ []string, _ string, _ []string) error {
			return fmt.Errorf("failed to merge")
		}
		sinkDirA := t.TempDir()
		sinkDirB := t.TempDir()
		sinkFile := filepath.Join("llvm-profile", "sinkfile")
		for _, dir := range []string{sinkDirA, sinkDirB} {
			profile := filepath.Join(dir, sinkFile)
			if err := os.MkdirAll(filepath.Dir(profile), os.ModePerm); err != nil {
				t.Fatalf("failed to create dir of %s: %s", profile, err)
			}
			if err := os.WriteFile(filepath.Join(dir, sinkFile), []byte("data"), os.ModePerm); err != nil {
				t.Fatalf("failed to write profile: %s", err)
			}
		}
		dest, err := tester.moveProfileToOutputDir(ctx, sinkDirA, sinkFile, "test1")
		expectedDest := sinkFile
		if err != nil {
			t.Errorf("failed to move profile %s to %s: %s", sinkFile, expectedDest, err)
		} else if dest != expectedDest {
			t.Errorf("got dest %s, want %s", dest, expectedDest)
		}
		if _, err := os.Stat(filepath.Join(localOutputDir, "v2", expectedDest)); err != nil {
			t.Errorf("failed to move proifle to v2 dir: %s", err)
		}
		dest, err = tester.moveProfileToOutputDir(ctx, sinkDirB, sinkFile, "test2")
		expectedDest = filepath.Join("llvm-profile", "test2", "sinkfile")
		if err != nil {
			t.Errorf("failed to move profile %s to %s: %s", sinkFile, expectedDest, err)
		} else if dest != expectedDest {
			t.Errorf("got dest %s, want %s", dest, expectedDest)
		}
		if _, err := os.Stat(filepath.Join(localOutputDir, "v2", expectedDest)); err != nil {
			t.Errorf("failed to move proifle to v2 dir: %s", err)
		}
	})
}

// Creates pair of ReadWriteClosers that mimics the relationship between serial
// and socket i/o. Implemented with in-memory pipes, the input of one can
// synchronously by read as the output of the other.
func serialAndSocket() (socketConn, socketConn) {
	rSerial, wSocket := io.Pipe()
	rSocket, wSerial := io.Pipe()
	serial := &joinedPipeEnds{rSerial, wSerial}
	socket := &joinedPipeEnds{rSocket, wSocket}
	return serial, socket
}

type fakeSerialServer struct {
	received       []byte
	shutdownString string
	socketPath     string
	listeningChan  chan bool
}

func (s *fakeSerialServer) Serve() error {
	listener, err := net.Listen("unix", s.socketPath)
	if err != nil {
		s.listeningChan <- false
		return fmt.Errorf("Listen(%s) failed: %w", s.socketPath, err)
	}
	defer listener.Close()
	s.listeningChan <- true
	conn, err := listener.Accept()
	if err != nil {
		return fmt.Errorf("Accept() failed: %w", err)
	}
	defer conn.Close()
	// Signal we're ready to accept input.
	if _, err := conn.Write([]byte(serialConsoleCursor)); err != nil {
		return fmt.Errorf("conn.Write() failed: %w", err)
	}
	reader := iomisc.NewMatchingReader(conn, []byte(s.shutdownString))
	for {
		buf := make([]byte, 1024)
		bytesRead, err := reader.Read(buf)
		s.received = append(s.received, buf[:bytesRead]...)
		if err != nil {
			if err == io.EOF {
				return nil
			}
			return fmt.Errorf("conn.Read() failed: %w", err)
		}
	}
}

// fakeContext conforms to context.Context but lets us control the return
// value of Err().
type fakeContext struct {
	sync.Mutex
	err error
}

func (ctx *fakeContext) Deadline() (time.Time, bool) {
	return time.Time{}, false
}

func (ctx *fakeContext) Done() <-chan struct{} {
	return make(chan struct{})
}

func (ctx *fakeContext) Err() error {
	ctx.Lock()
	defer ctx.Unlock()
	return ctx.err
}

func (ctx *fakeContext) SetErr(err error) {
	ctx.Lock()
	ctx.err = err
	ctx.Unlock()
}

func (ctx *fakeContext) Value(key any) any {
	return nil
}

func TestSerialTester(t *testing.T) {
	fooTest := testsharder.Test{
		Test: build.Test{
			Name: "myfoo",
			Path: "foo",
		},
	}

	pkgTest := testsharder.Test{
		Test: build.Test{
			Name:       "fuchsia-boot:///myfoo#meta/bar.cm",
			PackageURL: "fuchsia-boot:///myfoo#meta/bar.cm",
		},
	}

	fooExpectedCmd := "\r\nruntests foo\r\n"
	pkgExpectedCmd := "\r\nrun-test-suite --filter-ansi fuchsia-boot:///myfoo#meta/bar.cm\r\n"

	fooStarted := runtests.StartedSignature + fooTest.Name
	pkgStarted := "Running test '" + pkgTest.PackageURL + "'"

	cases := []struct {
		name           string
		test           testsharder.Test
		expectedCmd    string
		expectedStatus runtests.TestStatus
		wantErr        bool
		wantRetry      bool
		startedStr     string
		returnStr      string
	}{
		{
			name:           "test passes",
			test:           fooTest,
			expectedCmd:    fooExpectedCmd,
			expectedStatus: runtests.TestSuccess,
			startedStr:     fooStarted,
			returnStr:      runtests.SuccessSignature + fooTest.Name,
		},
		{
			name:           "packaged test passes",
			test:           pkgTest,
			expectedCmd:    pkgExpectedCmd,
			expectedStatus: runtests.TestSuccess,
			startedStr:     pkgStarted,
			returnStr:      pkgTest.PackageURL + " completed with result: PASSED",
		},
		{
			name:           "test fails",
			test:           fooTest,
			expectedCmd:    fooExpectedCmd,
			expectedStatus: runtests.TestFailure,
			startedStr:     fooStarted,
			returnStr:      runtests.FailureSignature + fooTest.Name,
		},
		{
			name:           "packaged test fails",
			test:           pkgTest,
			expectedCmd:    pkgExpectedCmd,
			expectedStatus: runtests.TestFailure,
			startedStr:     pkgStarted,
			returnStr:      pkgTest.PackageURL + " completed with result: FAILED",
		},
		{
			name:           "packaged test skipped",
			test:           pkgTest,
			expectedCmd:    pkgExpectedCmd,
			expectedStatus: runtests.TestSkipped,
			startedStr:     pkgStarted,
			returnStr:      pkgTest.PackageURL + " completed with result: SKIPPED",
		},
		{
			name:           "packaged test canceled",
			test:           pkgTest,
			expectedCmd:    pkgExpectedCmd,
			expectedStatus: runtests.TestAborted,
			startedStr:     pkgStarted,
			returnStr:      pkgTest.PackageURL + " completed with result: CANCELLED",
		},
		{
			name:           "test does not start on first try",
			test:           fooTest,
			expectedCmd:    fooExpectedCmd,
			expectedStatus: runtests.TestSuccess,
			wantRetry:      true,
			startedStr:     fooStarted,
			returnStr:      runtests.SuccessSignature + fooTest.Name,
		},
		{
			name:           "packaged test does not start on first try",
			test:           pkgTest,
			expectedCmd:    pkgExpectedCmd,
			expectedStatus: runtests.TestSuccess,
			wantRetry:      true,
			startedStr:     pkgStarted,
			returnStr:      pkgTest.PackageURL + " completed with result: PASSED",
		},
		{
			name:        "test returns fatal err",
			test:        fooTest,
			expectedCmd: fooExpectedCmd,
			wantErr:     true,
		},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			ctx := context.Background()
			serial, socket := serialAndSocket()
			defer socket.Close()
			defer serial.Close()

			tester := FuchsiaSerialTester{socket: socket}
			expectedCmd := tc.expectedCmd

			if tc.wantRetry {
				oldNewTestStartedContext := newTestStartedContext
				defer func() {
					newTestStartedContext = oldNewTestStartedContext
				}()
			}
			var fakeTestStartedContext fakeContext
			fakeTestStartedCancel := func() {}
			newTestStartedContext = func(ctx context.Context) (context.Context, context.CancelFunc) {
				return &fakeTestStartedContext, fakeTestStartedCancel
			}

			type testResult struct {
				result *runtests.TestDetails
				err    error
			}
			results := make(chan testResult)
			var stdout bytes.Buffer
			go func() {
				if err := tester.SetupTest(ctx, tc.test); err != nil {
					t.Errorf("tester.SetupTest got error: %s, want nil", err)
				}
				result, err := tester.Test(ctx, tc.test, &stdout, io.Discard, "unused-out-dir")
				results <- testResult{result, err}
			}()

			expectedTries := 1
			if tc.wantRetry {
				// Ensure tester times out waiting for the test to start.
				fakeTestStartedContext.SetErr(context.DeadlineExceeded)
				expectedTries = 2
			}

			// The write to the socket will block until we read from serial.
			buff := make([]byte, len(expectedCmd))
			for i := 0; i < expectedTries; i++ {
				if _, err := io.ReadFull(serial, buff); err != nil {
					t.Errorf("error reading from serial: %s", err)
				} else if string(buff) != expectedCmd {
					t.Errorf("unexpected command: %s", buff)
				}
			}

			if tc.wantRetry {
				fakeTestStartedContext.SetErr(nil)

				// At this point, the tester is either waiting for the startedSignature
				// (we set the startedContext error to nil before it started waiting for
				// it) or it's waiting to write to the socket again for its next retry.
				// In the latter case, we should read the remaining bytes from serial to
				// unblock the tester's write to the socket.
				// Put in a goroutine to not block in the case that there's nothing to read.
				// This will return when the serial is closed at the end of the test.
				go func() {
					if b, err := io.ReadAll(serial); err != nil {
						fmt.Println(err)
						results <- testResult{nil, fmt.Errorf("error reading from serial: %w", err)}
					} else if string(b) != expectedCmd || string(b) != "" {
						results <- testResult{nil, fmt.Errorf("unexpected serial input: %s", string(buff))}
					}
				}()
			}

			// At this point, the tester will be blocked reading from the socket.
			started := tc.startedStr
			testReturn := tc.returnStr

			if tc.wantErr {
				// Close the serial connection so that the tester returns a fatal error.
				serial.Close()
			} else {
				if _, err := io.WriteString(serial, started); err != nil {
					t.Errorf("failed to write %s to serial", started)
				}
				if _, err := io.WriteString(serial, testReturn); err != nil {
					t.Errorf("failed to write %s to serial", testReturn)
				}
			}

			select {
			case r := <-results:
				if gotErr := r.err != nil; gotErr != tc.wantErr {
					t.Errorf("test got err: %s, want err: %t", r.err, tc.wantErr)
				}
				if !tc.wantErr && r.result == nil {
					t.Error("got nil test result")
				}
				if !tc.wantErr && tc.expectedStatus != r.result.Status {
					t.Errorf("test got result: %s, want: %s", r.result.Status, tc.expectedStatus)
				}
			}
			stdoutBytes := stdout.Bytes()
			if !tc.wantErr && !bytes.Contains(stdoutBytes, []byte(started+testReturn)) {
				t.Errorf("Expected stdout to contain %q, got %q", started+testReturn, string(stdoutBytes))
			}
		})
	}
}

func longKernelLog(numChars int) string {
	var kernelLog strings.Builder
	kernelLog.WriteString("[123.456]")
	for i := 0; i < numChars; i++ {
		kernelLog.WriteString("a")
	}
	return kernelLog.String() + "\n"
}

func longPotentialKernelLog(numChars int) string {
	var log strings.Builder
	log.WriteString("[")
	for i := 0; i < numChars; i++ {
		log.WriteString("1")
	}
	return log.String()
}

type hangForeverReader struct {
	readCalled chan struct{}
	readChan   chan struct{}
}

func (r *hangForeverReader) Read(buf []byte) (int, error) {
	r.readCalled <- struct{}{}
	<-r.readChan
	return 0, nil
}

func TestParseOutKernelReader(t *testing.T) {
	cases := []struct {
		name           string
		output         string
		expectedOutput string
	}{
		{
			name:           "no kernel logs",
			output:         "line1\n[line2]output\nline3[output]\nline4[out",
			expectedOutput: "line1\n[line2]output\nline3[output]\nline4[out",
		}, {
			name:           "with kernel logs",
			output:         "line1\n[123.456]kernel line1\noutput[123.456]kernel line2\noutput continued [bracket] output [123.456] kernel [line3]\noutput continued[123]\n",
			expectedOutput: "line1\noutputoutput continued [bracket] output output continued",
		}, {
			name:           "long kernel log",
			output:         "line1" + longKernelLog(2000) + "output continued\n",
			expectedOutput: "line1output continued\n",
		}, {
			name: "long potential kernel log",
			// This should test the proper reading of logs which need to store both
			// kernel and non-kernel logs for later processing.
			output:         "line1" + longPotentialKernelLog(2000) + "\noutput continued[123",
			expectedOutput: "line1" + longPotentialKernelLog(2000) + "\noutput continued[123",
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			r := &parseOutKernelReader{
				ctx:    context.Background(),
				reader: strings.NewReader(c.output),
			}
			b, err := io.ReadAll(r)
			if err != nil {
				t.Fatal(err)
			}
			if string(b) != c.expectedOutput {
				t.Errorf("expected: %s, got: %s", c.expectedOutput, string(b))
			}
		})
	}
	t.Run("canceled context", func(t *testing.T) {
		ctx, cancel := context.WithCancel(context.Background())
		reader := &hangForeverReader{readCalled: make(chan struct{}), readChan: make(chan struct{})}
		r := &parseOutKernelReader{
			ctx:    ctx,
			reader: reader,
		}
		errs := make(chan error)
		go func() {
			_, err := io.ReadAll(r)
			errs <- err
		}()
		// Wait for Read() to be called before canceling the context.
		<-reader.readCalled
		cancel()
		if err := <-errs; !errors.Is(err, ctx.Err()) {
			t.Errorf("expected: %s, got: %s", ctx.Err(), err)
		}
		close(reader.readChan)
	})
}

func TestCommandForTest(t *testing.T) {
	cases := []struct {
		name      string
		test      testsharder.Test
		useSerial bool
		timeout   time.Duration
		expected  []string
		wantErr   bool
	}{
		{
			name:      "use serial URL",
			useSerial: true,
			test: testsharder.Test{
				Test: build.Test{
					PackageURL: "fuchsia-pkg://example.com/test.cm",
				},
			},
			expected: []string{"run-test-suite", "--filter-ansi", "fuchsia-pkg://example.com/test.cm"},
		},
		{
			name:      "use serial path",
			useSerial: true,
			test: testsharder.Test{
				Test: build.Test{
					Path: "/path/to/test",
				},
			},
			expected: []string{"runtests", "/path/to/test"},
		},
		{
			name:      "use serial timeout",
			useSerial: true,
			test: testsharder.Test{
				Test: build.Test{
					Path: "/path/to/test",
				},
			},
			timeout:  time.Second,
			expected: []string{"runtests", "-i", "1", "/path/to/test"},
		},
		{
			name:      "system path",
			useSerial: false,
			test: testsharder.Test{
				Test: build.Test{
					Path: "/path/to/test",
				},
			},
			wantErr: true,
		},
		{
			name:      "components v2",
			useSerial: false,
			test: testsharder.Test{
				Test: build.Test{
					Path:       "/path/to/test",
					PackageURL: "fuchsia-pkg://example.com/test.cm",
				},
			},
			expected: []string{"run-test-suite", "--filter-ansi", "fuchsia-pkg://example.com/test.cm"},
		},
		{
			name:      "components v2 no parallel",
			useSerial: false,
			test: testsharder.Test{
				Test: build.Test{
					Path:       "/path/to/test",
					PackageURL: "fuchsia-pkg://example.com/test.cm",
				},
			},
			expected: []string{"run-test-suite", "--filter-ansi", "fuchsia-pkg://example.com/test.cm"},
		},
		{
			name:      "components v2 parallel",
			useSerial: false,
			test: testsharder.Test{
				Test: build.Test{
					Path:       "/path/to/test",
					PackageURL: "fuchsia-pkg://example.com/test.cm",
					Parallel:   2,
				},
			},
			expected: []string{"run-test-suite", "--filter-ansi", "--parallel", "2", "fuchsia-pkg://example.com/test.cm"},
		},
		{
			name:      "components v2 timeout",
			useSerial: false,
			test: testsharder.Test{
				Test: build.Test{
					Path:       "/path/to/test",
					PackageURL: "fuchsia-pkg://example.com/test.cm",
				},
			},
			timeout:  time.Second,
			expected: []string{"run-test-suite", "--filter-ansi", "--timeout", "1", "fuchsia-pkg://example.com/test.cm"},
		},
		{
			name:      "components v2 with realm",
			useSerial: false,
			test: testsharder.Test{
				Test: build.Test{
					Path:       "/path/to/test",
					PackageURL: "fuchsia-pkg://example.com/test.cm",
				},
				Realm: "/some/realm",
			},
			expected: []string{"run-test-suite", "--filter-ansi", "--realm", "/some/realm", "fuchsia-pkg://example.com/test.cm"},
		},
		{
			name:      "components v2 with empty realm",
			useSerial: false,
			test: testsharder.Test{
				Test: build.Test{
					Path:       "/path/to/test",
					PackageURL: "fuchsia-pkg://example.com/test.cm",
				},
				Realm: "",
			},
			expected: []string{"run-test-suite", "--filter-ansi", "fuchsia-pkg://example.com/test.cm"},
		},
	}

	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			command, err := commandForTest(&c.test, c.useSerial, c.timeout)
			if err == nil {
				if c.wantErr {
					t.Errorf("commandForTest returned nil error, want non-nil error")
				}
			} else if !c.wantErr {
				t.Errorf("commandForTest returned error: %s, want nil", err)
			}
			if diff := cmp.Diff(c.expected, command); diff != "" {
				t.Errorf("unexpected command (-want +got):\n%s", diff)
			}
		})
	}
}

type joinedPipeEnds struct {
	r *io.PipeReader
	w *io.PipeWriter
}

func (pe *joinedPipeEnds) Read(p []byte) (int, error) {
	return pe.r.Read(p)
}

func (pe *joinedPipeEnds) Write(p []byte) (int, error) {
	return pe.w.Write(p)
}

func (pe *joinedPipeEnds) SetIOTimeout(_ time.Duration) {
}

func (pe *joinedPipeEnds) Close() error {
	if err := pe.r.Close(); err != nil {
		pe.w.Close()
		return err
	}
	return pe.w.Close()
}

func TestBaseTestResultFromTest(t *testing.T) {
	test := testsharder.Test{
		Test: build.Test{
			Name:  "fuchsia-pkg://fuchsia.com/sparky-sparky-boom-test#meta/sparky-sparky-boom-test.cm",
			Label: "//src/sys:foo_test(//build/toolchain/fuchsia:x64)",
		},
		Tags: []build.TestTag{
			{
				Key:   "key",
				Value: "value",
			},
		},
		Metadata: metadata.TestMetadata{
			Owners:      []string{"carverforbes@google.com"},
			ComponentID: 1478143,
		},
	}
	expected := runtests.TestDetails{
		Name:    "fuchsia-pkg://fuchsia.com/sparky-sparky-boom-test#meta/sparky-sparky-boom-test.cm",
		GNLabel: "//src/sys:foo_test(//build/toolchain/fuchsia:x64)",
		Status:  runtests.TestFailure,
		Tags: []build.TestTag{
			{
				Key:   "key",
				Value: "value",
			},
		},
		// This is necessary for LUCI analysis to know where (which component) to
		// file the bug.
		Metadata: metadata.TestMetadata{
			Owners:      []string{"carverforbes@google.com"},
			ComponentID: 1478143,
		},
	}
	testResult := *BaseTestResultFromTest(test)
	if diff := cmp.Diff(expected, testResult); diff != "" {
		t.Errorf("BaseTestResultFromTest() failed: (-want +got): \n%s", diff)
	}
}

func TestProcessTestResult(t *testing.T) {
	tmpDir := t.TempDir()

	// Create a mock stderr file.
	stderrContent := "test case failed with some error"
	stderrFile := "stderr.txt"
	caseArtifactDir := filepath.Join(tmpDir, "case_artifacts")
	if err := os.MkdirAll(caseArtifactDir, 0755); err != nil {
		t.Fatalf("failed to create case artifact dir: %s", err)
	}
	if err := os.WriteFile(filepath.Join(caseArtifactDir, stderrFile), []byte(stderrContent), 0644); err != nil {
		t.Fatalf("failed to write stderr file: %s", err)
	}

	runResultData := ffxutil.TestRunResult{
		Suites: []ffxutil.SuiteResult{
			{
				Outcome:     ffxutil.TestFailed,
				Name:        "fuchsia-pkg://foo#meta/bar.cm",
				ArtifactDir: "suite_artifacts",
				Cases: []ffxutil.CaseResult{
					{
						Outcome:     ffxutil.TestFailed,
						Name:        "case1",
						ArtifactDir: "case_artifacts",
						Artifacts: map[string]ffxutil.ArtifactMetadata{
							stderrFile: {
								ArtifactType: ffxutil.StderrType,
							},
						},
					},
					{
						Outcome:     ffxutil.TestPassed,
						Name:        "case_passed",
						ArtifactDir: "case_artifacts",
						Artifacts: map[string]ffxutil.ArtifactMetadata{
							stderrFile: {
								ArtifactType: ffxutil.StderrType,
							},
						},
					},
					{
						Outcome:     ffxutil.TestSkipped,
						Name:        "case_skipped",
						ArtifactDir: "case_artifacts",
						Artifacts: map[string]ffxutil.ArtifactMetadata{
							stderrFile: {
								ArtifactType: ffxutil.StderrType,
							},
						},
					},
				},
			},
		},
	}

	envelope := ffxutil.TestRunResultEnvelope{
		Data:     runResultData,
		SchemaID: "https://fuchsia.dev/schema/ffx_test/run_summary-8d1dd964.json",
	}

	envelopeBytes, err := json.Marshal(envelope)
	if err != nil {
		t.Fatalf("failed to marshal envelope: %s", err)
	}

	if err := os.WriteFile(filepath.Join(tmpDir, "run_summary.json"), envelopeBytes, 0644); err != nil {
		t.Fatalf("failed to write run_summary.json: %s", err)
	}

	runResult, err := ffxutil.GetRunResult(tmpDir)
	if err != nil {
		t.Fatalf("ffxutil.GetRunResult failed: %s", err)
	}

	test := testsharder.Test{
		Test: build.Test{
			PackageURL: "fuchsia-pkg://foo#meta/bar.cm",
		},
	}

	testDetails, err := processTestResult(runResult, test, 5*time.Second, false)
	if err != nil {
		t.Fatalf("processTestResult failed: %s", err)
	}

	if testDetails.Status != runtests.TestFailure {
		t.Errorf("got test status %v, want %v", testDetails.Status, runtests.TestFailure)
	}

	if len(testDetails.Cases) != 3 {
		t.Fatalf("got %d test cases, want 3", len(testDetails.Cases))
	}

	tcFailed := testDetails.Cases[0]
	expectedFailureReason := &runtests.FailureReason{
		Errors: []*runtests.FailureReasonError{
			{Message: stderrContent},
		},
	}

	if diff := cmp.Diff(expectedFailureReason, tcFailed.FailureReason); diff != "" {
		t.Errorf("unexpected FailureReason (-want +got):\n%s", diff)
	}

	tcPassed := testDetails.Cases[1]
	if tcPassed.FailureReason != nil {
		t.Errorf("passed case got FailureReason %v, want nil", tcPassed.FailureReason)
	}

	tcSkipped := testDetails.Cases[2]
	if tcSkipped.FailureReason != nil {
		t.Errorf("skipped case got FailureReason %v, want nil", tcSkipped.FailureReason)
	}
}
