// Copyright 2018 The Fuchsia Authors. All rights reserved.
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
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"time"

	"go.fuchsia.dev/fuchsia/tools/botanist"
	"go.fuchsia.dev/fuchsia/tools/botanist/constants"
	"go.fuchsia.dev/fuchsia/tools/botanist/targets"
	"go.fuchsia.dev/fuchsia/tools/lib/environment"
	"go.fuchsia.dev/fuchsia/tools/lib/ffxutil"
	"go.fuchsia.dev/fuchsia/tools/lib/flagmisc"
	"go.fuchsia.dev/fuchsia/tools/lib/jsonutil"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
	"go.fuchsia.dev/fuchsia/tools/lib/osmisc"
	"go.fuchsia.dev/fuchsia/tools/lib/retry"
	"go.fuchsia.dev/fuchsia/tools/lib/serial"
	"go.fuchsia.dev/fuchsia/tools/lib/subprocess"
	"go.fuchsia.dev/fuchsia/tools/lib/syslog"
	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
	"go.fuchsia.dev/fuchsia/tools/testing/testrunner"
	testrunnerconstants "go.fuchsia.dev/fuchsia/tools/testing/testrunner/constants"

	"github.com/google/subcommands"
	"golang.org/x/sync/errgroup"
)

// RunCommand is a Command implementation for booting a device and running a
// given command locally.
type RunCommand struct {
	// ConfigFile is the path to the target configurations.
	configFile string

	// ProductBundle is a path to product_bundles.json file.
	productBundles string

	// ProductBundleName is a name of product bundle getting used.
	productBundleName string

	// IsBootTest tells whether the product bundle provided is for a boot test.
	isBootTest bool

	// Netboot tells botanist to netboot (and not to pave).
	netboot bool

	// ZirconArgs are kernel command-line arguments to pass on boot.
	zirconArgs flagmisc.StringsValue

	// Timeout is the duration allowed for the command to finish execution.
	timeout time.Duration

	// syslogDir, if nonempty, is the directory in which system syslogs will be written.
	syslogDir string

	// serialLogDir, if nonempty, is the directory in which system serial logs will be written.
	serialLogDir string

	// localRepo specifies the path to a local package repository. If set,
	// botanist will spin up a package server to serve packages from this
	// repository.
	localRepo string

	// The path to the ffx tool.
	ffxPath string

	// Experiments to enable. Supported experiments can be found at //tools/botanist/common.go.
	experiments flagmisc.StringsValue

	// When true skips setting up the targets.
	skipSetup bool

	// Args passed to testrunner
	testrunnerOptions testrunner.Options

	// The timeout to wait for an SSH connection after booting the target.
	bootupTimeout time.Duration

	// Whether the product bundle is expected to support SSH.
	expectsSSH bool

	// The scale factor to multiply test timeouts by. This may be set if the bot environment
	// is known to be slower than usual.
	testTimeoutScaleFactor int
}

func (*RunCommand) Name() string {
	return "run"
}

func (*RunCommand) Usage() string {
	return `
botanist run [flags...] tests-file

flags:
`
}

func (*RunCommand) Synopsis() string {
	return fmt.Sprintf("boots a device and executes all tests found in the JSON [tests-file].")
}

func (r *RunCommand) SetFlags(f *flag.FlagSet) {
	f.StringVar(&r.configFile, "config", "", "path to file of device config")
	f.StringVar(&r.productBundles, "product-bundles", "", "path to product_bundles.json file")
	f.StringVar(&r.productBundleName, "product-bundle-name", "", "name of product bundle to use")
	f.BoolVar(&r.isBootTest, "boot-test", false, "whether the provided product bundle is for a boot test.")
	f.BoolVar(&r.netboot, "netboot", false, "if set, botanist will not pave; but will netboot instead")
	f.Var(&r.zirconArgs, "zircon-args", "kernel command-line arguments")
	f.DurationVar(&r.timeout, "timeout", 0, "duration allowed for the command to finish execution, a value of 0 (zero) will not impose a timeout.")
	f.StringVar(&r.syslogDir, "syslog-dir", "", "the directory to write all system logs to.")
	f.StringVar(&r.serialLogDir, "serial-log-dir", "", "the directory to write all serial logs to.")
	f.StringVar(&r.localRepo, "local-repo", "", "path to a local package repository; the repo and blobs flags are ignored when this is set")
	f.StringVar(&r.ffxPath, "ffx", "", "Path to the ffx tool.")
	f.Var(&r.experiments, "experiment", fmt.Sprintf("The name of an experiment to enable. Supported experiments are: %v.", botanist.SupportedExperiments))
	f.BoolVar(&r.skipSetup, "skip-setup", false, "if set, botanist will not set up a target.")
	f.DurationVar(&r.bootupTimeout, "bootup-timeout", 0, "duration allowed for the command to finish execution, a value of 0 (zero) will fall back to the default.")
	f.BoolVar(&r.expectsSSH, "expects-ssh", false, "if set, botanist will try to establish an SSH connection before running tests.")
	f.IntVar(&r.testTimeoutScaleFactor, "test-timeout-scale-factor", 1, "Factor to scale test timeouts by (used for slow bot environments)")

	// Parsing of testrunner options.
	f.StringVar(&r.testrunnerOptions.OutDir, "out-dir", "", "Optional path where a directory containing test results should be created.")
	f.StringVar(&r.testrunnerOptions.NsjailPath, "nsjail", "", "Optional path to an NsJail binary to use for linux host test sandboxing.")
	f.StringVar(&r.testrunnerOptions.NsjailRoot, "nsjail-root", "", "Path to the directory to use as the NsJail root directory")
	f.StringVar(&r.testrunnerOptions.LocalWD, "C", "", "Working directory of local testing subprocesses; if unset the current working directory will be used.")
	f.StringVar(&r.testrunnerOptions.SnapshotFile, "snapshot-output", "", "The output filename for the snapshot. This will be created in the output directory.")
	f.BoolVar(&r.testrunnerOptions.UseSerial, "use-serial", false, "Use serial to run tests on the target.")
	f.StringVar(&r.testrunnerOptions.LLVMProfdataPath, "llvm-profdata", "", "Optional path to a llvm-profdata binary to use for merging profiles on the host in between tests.")
}

// This returns an `ffx` instance, a cleanup function (dispatched via `defer`), and an error.
func (r *RunCommand) setupFFX(ctx context.Context, invokeMode ffxutil.FFXInvokeMode, experiments botanist.Experiments) (*ffxutil.FFXInstance, func(), error) {
	if r.ffxPath == "" {
		return nil, nil, fmt.Errorf("ffx path must be provided with the -ffx flag.")
	}
	ffxOutputsDir := filepath.Join(os.Getenv(testrunnerconstants.TestOutDirEnvKey), "ffx_outputs")

	extraConfigs := ffxutil.ConfigSettings{
		Level: "global",
		Settings: map[string]any{
			"daemon.autostart":       false,
			"discovery.mdns.enabled": false,
		},
	}

	// By default, the ssh.priv and ssh.pub values are in $HOME, which had earlier been configured to be a tmpdir.
	// But in case we're in strict mode, let's be explicit about the path. If there is no pub key, when we will
	// let the FFXInstance specify the default
	sshPriv := filepath.Join(os.Getenv("HOME"), ".ssh", "fuchsia_ed25519")
	sshKeys := ffxutil.SSHInfo{SshPriv: sshPriv}
	ffx, err := ffxutil.NewFFXInstance(ctx, r.ffxPath, "", []string{}, "", &sshKeys, ffxOutputsDir, invokeMode, extraConfigs)
	if err != nil {
		return nil, nil, err
	}
	stdout, stderr, flush := botanist.NewStdioWriters(ctx, "ffx")
	defer flush()
	ffx.SetStdoutStderr(stdout, stderr)
	if err := ffx.ConfigEnv(ctx); err != nil {
		return ffx, nil, err
	}
	return ffx, nil, nil
}

func (r *RunCommand) setupSerialLog(ctx context.Context, eg *errgroup.Group, fuchsiaTargets []targets.FuchsiaTarget) error {
	if r.serialLogDir == "" {
		return nil
	}

	if err := os.MkdirAll(r.serialLogDir, os.ModePerm); err != nil {
		return err
	}

	for _, t := range fuchsiaTargets {
		t := t
		eg.Go(func() error {
			logger.Debugf(ctx, "starting serial collection for target %s", t.Nodename())

			// Create a new file to capture the serial log for this nodename.
			serialLogName := fmt.Sprintf("%s_serial_log.txt", t.Nodename())
			// TODO(https://fxbug.dev/42150891): Remove once there are no dependencies on this filename.
			if len(fuchsiaTargets) == 1 {
				serialLogName = "serial_log.txt"
			}
			serialLogPath := filepath.Join(r.serialLogDir, serialLogName)
			absPath, err := filepath.Abs(serialLogPath)
			if err != nil {
				return fmt.Errorf("failed to get abspath of serial log: %w", err)
			}
			if err := os.Setenv(constants.SerialLogEnvKey, absPath); err != nil {
				logger.Debugf(ctx, "failed to set %s to %s", constants.SerialLogEnvKey, absPath)
			}

			// Start capturing the serial log for this target.
			if err := t.CaptureSerialLog(serialLogPath); err != nil && ctx.Err() == nil {
				return err
			}
			return nil
		})
	}
	return nil
}

func getPkgSrvPort(ctx context.Context) (int, error) {
	var port int
	pkgSrvPort := os.Getenv(constants.PkgSrvPortKey)
	if pkgSrvPort == "" {
		logger.Warningf(ctx, "%s is empty, using default port %d", constants.PkgSrvPortKey, botanist.DefaultPkgSrvPort)
		port = botanist.DefaultPkgSrvPort
	} else {
		var err error
		port, err = strconv.Atoi(pkgSrvPort)
		if err != nil {
			return 0, err
		}
	}
	return port, nil
}

func (r *RunCommand) setupFFXPackageServer(ctx context.Context, ffx *targets.FFXInstance, name string) (int, func(), error) {
	cleanup := func() {}
	if r.localRepo == "" {
		return 0, cleanup, nil
	}

	port, err := getPkgSrvPort(ctx)
	if err != nil {
		return port, cleanup, err
	}
	cleanup = botanist.WaitForProcess(ctx, func(cmCtx context.Context) error {
		logger.Debugf(ctx, "starting package server")
		return ffx.StartPackageServer(cmCtx, name, "[::]", r.localRepo, port)
	}, "ffx repository server")
	finalCleanup := func() {
		if err := ffx.StopPackageServer(botanist.GetLoggerCtx(ctx), name, port); err != nil {
			logger.Errorf(ctx, "failed to stop package server: %s", err)
		}
		cleanup()
	}
	return port, finalCleanup, nil
}

func (r *RunCommand) setupPackageServer(ctx context.Context) (*botanist.PackageServer, error) {
	if r.localRepo == "" {
		return nil, nil
	}

	port, err := getPkgSrvPort(ctx)
	if err != nil {
		return nil, err
	}
	pkgSrv, err := botanist.NewPackageServer(ctx, r.localRepo, port)
	if err != nil {
		return pkgSrv, err
	}
	return pkgSrv, nil
}

func (r *RunCommand) setupFFXUSBDriver(ctx context.Context, ffx *targets.FFXInstance, serialNum string) (func(), error) {
	cleanup := func() {}
	socketPath := filepath.Join(os.Getenv(testrunnerconstants.TestOutDirEnvKey), "ffx_usb_driver_socketpath")
	logDir := filepath.Join(os.Getenv(testrunnerconstants.TestOutDirEnvKey), "ffx_usb_driver_logs")
	if err := os.MkdirAll(logDir, os.ModePerm); err != nil {
		return cleanup, fmt.Errorf("failed to create ffx usb driver log dir: %w", err)
	}
	if err := ffx.ConfigSet(ctx, "connectivity.usb_socket_path", socketPath); err != nil {
		return cleanup, fmt.Errorf("failed to set connectivity.usb_socket_path to %s: %w", socketPath, err)
	}
	cleanup = botanist.WaitForProcess(ctx, func(processCtx context.Context) error {
		logger.Debugf(ctx, "starting ffx usb driver")
		return ffx.USBDriver(processCtx, serialNum, logDir)
	}, "ffx usb-driver")
	return func() {
		cleanup()
		if err := os.Remove(socketPath); err != nil && !os.IsNotExist(err) {
			logger.Errorf(ctx, "failed to remove ffx usb driver socket path: %s", err)
		}
	}, nil
}

func (r *RunCommand) dispatchTests(ctx context.Context, cancel context.CancelFunc, eg *errgroup.Group, baseTargets []targets.Base, fuchsiaTargets []targets.FuchsiaTarget, primaryTarget targets.FuchsiaTarget, pkgSrv *botanist.PackageServer, testsPath string, experiments botanist.Experiments) {
	// Log any failures after running tests.
	for _, t := range fuchsiaTargets {
		t := t
		eg.Go(func() error {
			if err := t.Wait(ctx); err != nil && err != targets.ErrUnimplemented && ctx.Err() == nil {
				return fmt.Errorf("target %s failed: %w", t.Nodename(), err)
			}
			return nil
		})
	}

	// Dispatch tests.
	eg.Go(func() error {
		// Signal other goroutines to exit when tests complete.
		defer cancel()

		if r.productBundles == "" {
			return fmt.Errorf("-product-bundles is required")
		}
		if r.productBundleName == "" {
			return fmt.Errorf("-product-bundle-name is required")
		}
		startOpts := targets.StartOptions{
			Netboot:           r.netboot,
			ZirconArgs:        r.zirconArgs,
			ProductBundles:    r.productBundles,
			ProductBundleName: r.productBundleName,
			IsBootTest:        r.isBootTest,
			BootupTimeout:     r.bootupTimeout,
		}

		if err := targets.StartTargets(ctx, startOpts, fuchsiaTargets); err != nil {
			return fmt.Errorf("%s: %w", constants.FailedToStartTargetMsg, err)
		}
		logger.Debugf(ctx, "successfully started all targets")

		defer func() {
			ctx, cancel := context.WithTimeout(botanist.GetLoggerCtx(ctx), time.Minute)
			defer cancel()
			targets.StopTargets(ctx, fuchsiaTargets)
		}()

		// Create a testbed config file. We have to do this after starting the
		// targets so that we can get their IP addresses.
		testbedConfig, testbedConfigPath, err := r.createTestbedConfig(baseTargets)
		if err != nil {
			return err
		}
		defer os.Remove(testbedConfigPath)

		if r.expectsSSH {
			for _, t := range fuchsiaTargets {
				t := t
				client, err := t.SSHClient()
				if err != nil {
					if err := r.dumpSyslogOverSerial(ctx, t.SerialSocketPath()); err != nil {
						logger.Errorf(ctx, err.Error())
					}
					return err
				}
				if pkgSrv != nil {
					if err := t.AddPackageRepository(client, pkgSrv.RepoURL, pkgSrv.BlobURL); err != nil {
						return err
					}
					logger.Debugf(ctx, "added package repo to target %s", t.Nodename())
				}
				addr, err := targets.IPAddr(t)
				if err != nil {
					return err
				}
				forceFFXUSB := false
				if _, ok := t.(*targets.Device); !ok || !experiments.Contains(botanist.ForceFFXUSB) {
					t.GetFFX().SetTarget(addr.String())
				} else {
					forceFFXUSB = true
					// For devices, the ffx target has been set to the serial number
					// already so use that.
					cleanupUSBDriver, err := r.setupFFXUSBDriver(ctx, t.GetFFX(), strings.TrimPrefix(t.GetFFX().GetTarget(), "id:"))
					if err != nil {
						return fmt.Errorf("failed to set up ffx usb driver: %w", err)
					}
					defer cleanupUSBDriver()
				}
				if experiments.Contains(botanist.UseFFXRepository) {
					repoName := "fuchsia-package-server"
					pkgSrvPort, cleanupPkgServer, err := r.setupFFXPackageServer(ctx, t.GetFFX(), repoName)
					if err != nil {
						return err
					}
					defer cleanupPkgServer()
					if pkgSrvPort != 0 {
						if err := retry.Retry(ctx, retry.WithMaxDuration(retry.NewConstantBackoff(time.Second), 5*time.Second), func() error {
							if servers, err := t.GetFFX().ListPackageServer(ctx); err != nil {
								return err
							} else if !slices.Contains(servers, repoName) {
								return fmt.Errorf("package server not started yet")
							}
							return nil
						}, nil); err != nil {
							return err
						}
						cleanupForward, err := t.AddFFXPackageRepository(ctx, repoName, pkgSrvPort, forceFFXUSB)
						if err != nil {
							return err
						}
						defer cleanupForward()
					}
				}
				if !forceFFXUSB {
					cleanupControlMaster, err := t.SetupSSHControlMaster(ctx, t.SSHKey(), addr.String())
					if err != nil {
						return fmt.Errorf("failed to set up ssh controlmaster: %w", err)
					}
					defer cleanupControlMaster()
				}
				if r.syslogDir != "" {
					if _, err := os.Stat(r.syslogDir); errors.Is(err, os.ErrNotExist) {
						if err := os.Mkdir(r.syslogDir, os.ModePerm); err != nil {
							return err
						}
					}
					defer t.StopSyslog()
					go func() {
						syslogName := fmt.Sprintf("%s_syslog.txt", t.Nodename())
						// TODO(https://fxbug.dev/42150891): Remove when there are no dependencies on this filename.
						if len(fuchsiaTargets) == 1 {
							syslogName = "syslog.txt"
						}
						syslogPath := filepath.Join(r.syslogDir, syslogName)
						if err := t.CaptureSyslog(client, syslogPath, pkgSrv); err != nil && ctx.Err() == nil {
							logger.Errorf(ctx, "%s at %s: %s", constants.FailedToCaptureSyslogMsg, syslogPath, err)
						}
					}()
				}
			}
			if experiments.Contains(botanist.UseFFXMonitor) {
				ffx := primaryTarget.GetFFX()
				port := strings.TrimSpace(os.Getenv(constants.FFXMonitorPort))
				if len(port) == 0 {
					logger.Warningf(ctx, "%s is empty, using default port %s", constants.FFXMonitorPort, constants.DefaultFFXMonitorPort)
					port = constants.DefaultFFXMonitorPort
				}

				const (
					monitorName         = "ffx_monitor"
					logFileName         = "device.status.json"
					aggregationFilename = "aggregation.json"
				)

				// Create a new context for the monitor so that it isn't cancelled when the
				// errgroup context is cancelled. This ensures that we can gracefully stop
				// the monitor and flush logs.
				monitorCtx, cancelMonitor := context.WithCancel(botanist.GetLoggerCtx(ctx))

				// Stop the ffx monitor when done
				defer func() {
					// Use a separate timeout context for stopping
					stopCtx, cancelStop := context.WithTimeout(botanist.GetLoggerCtx(ctx), time.Minute)
					defer cancelStop()
					if err := ffx.StopFFXMonitor(stopCtx); err != nil {
						logger.Errorf(ctx, "failed to stop ffx monitor: %s", err)
					} else {
						logger.Debugf(ctx, "ffx monitor stopped")
						// TODO(https://fxbug.dev/489556654): Move the writing of the summary.json to botanist
						// Update summary.json to include the monitor aggregation file.
						summaryPath := filepath.Join(os.Getenv(testrunnerconstants.TestOutDirEnvKey), r.testrunnerOptions.OutDir, runtests.TestSummaryFilename)
						if data, err := os.ReadFile(summaryPath); err == nil {
							var summary runtests.TestSummary
							if err := json.Unmarshal(data, &summary); err == nil {
								summary.Tests = append(summary.Tests, runtests.TestDetails{
									Name:      monitorName,
									Status:    runtests.TestSuccess,
									StartTime: time.Now(),
									TestResult: runtests.TestResult{
										OutputDir:   monitorName,
										OutputFiles: []string{aggregationFilename},
									},
								})
								if err := jsonutil.WriteToFile(summaryPath, summary); err != nil {
									logger.Errorf(ctx, "failed to write updated summary.json: %s", err)
								}
							}
						}
					}
					cancelMonitor()
				}()
				go func() {
					logDir := filepath.Join(os.Getenv(testrunnerconstants.TestOutDirEnvKey), r.testrunnerOptions.OutDir, monitorName)
					logFile := filepath.Join(logDir, logFileName)
					aggregationsFile := filepath.Join(logDir, aggregationFilename)
					if err := ffx.StartFFXMonitor(monitorCtx, port, logFile, aggregationsFile); err != nil && !errors.Is(err, context.Canceled) {
						logger.Errorf(ctx, "failed to start ffx monitor: %s", err)
					} else {
						logger.Debugf(ctx, "ffx monitor process finished")
					}
				}()
			}
		}

		err = r.runAgainstTarget(ctx, primaryTarget, testsPath, testbedConfig, testbedConfigPath)
		// Cancel ctx to notify other goroutines that this routine has completed.
		// If another goroutine gets an error and the context is canceled, it
		// should return nil so that we always prioritize the result from this
		// goroutine.
		cancel()
		return err
	})
}

func (r *RunCommand) execute(ctx context.Context, args []string) error {
	ctx, cancel := context.WithCancel(ctx)
	if r.timeout != 0 {
		ctx, cancel = context.WithTimeout(ctx, r.timeout)
	}

	go func() {
		<-ctx.Done()
		// Log the timeout for tefmocheck to detect it.
		if ctx.Err() == context.DeadlineExceeded {
			logger.Errorf(ctx, "%s (%s)", constants.CommandExceededTimeoutMsg, r.timeout)
		}
	}()
	defer cancel()

	testsPath := args[0]

	if r.skipSetup {
		if err := testrunner.SetupAndExecute(ctx, r.testrunnerOptions, testsPath); err != nil {
			return fmt.Errorf("testrunner with flags: %v, with timeout: %s, failed: %w", r.testrunnerOptions, r.timeout, err)
		}
		return nil
	}

	experiments := botanist.GetExperiments(r.experiments)
	invokeMode := ffxutil.UseFFXStrict
	ffx, cleanup, err := r.setupFFX(ctx, invokeMode, experiments)
	if cleanup != nil {
		defer cleanup()
	}
	if err != nil {
		return err
	}
	sshKey := ffx.GetSshPrivateKey()
	authorizedKey := ffx.GetSshAuthorizedKeys()

	// Parse targets out from the target configuration file.
	baseTargets, fuchsiaTargets, err := r.deriveTargetsFromFile(ctx, targets.Options{
		Netboot:       r.netboot,
		ExpectsSSH:    r.expectsSSH,
		SSHKey:        sshKey,
		AuthorizedKey: authorizedKey,
		SerialLogDir:  r.serialLogDir,
	})
	if err != nil {
		return err
	}
	// Determine the target that a command will be run against and logs will be
	// streamed from.
	primaryTarget := fuchsiaTargets[0]

	for _, t := range fuchsiaTargets {
		// Start serial servers for all targets. Will no-op for targets that
		// already have serial servers.
		if err := t.StartSerialServer(); err != nil {
			return err
		}
		// Attach an ffx instance for all targets. All ffx instances will use the same
		// config and daemon, but run commands against its own specified target. The target
		// will be set after starting the target, so that we can resolve the IP address.
		ffxForTarget := ffxutil.FFXWithTarget(ffx, "")
		t.SetFFX(&targets.FFXInstance{ffxForTarget, experiments}, ffx.Env())
		if _, ok := t.(*targets.Device); ok && experiments.Contains(botanist.ForceFFXUSB) {
			t.GetFFX().ConfigSet(ctx, "connectivity.enable_usb", "true")
			t.GetFFX().ConfigSet(ctx, "connectivity.enable_network", "false")
		} else {
			// Make this explicit in case the tools team wants to change the default
			// for users at their desks.
			t.GetFFX().ConfigSet(ctx, "connectivity.enable_usb", "false")
		}
	}

	eg, ctx := errgroup.WithContext(ctx)

	if err := r.setupSerialLog(ctx, eg, fuchsiaTargets); err != nil {
		return err
	}

	// Run any preflights to prepare the testbed.
	if err := r.runPreflights(ctx); err != nil {
		return err
	}

	var pkgSrv *botanist.PackageServer
	if !experiments.Contains(botanist.UseFFXRepository) {
		pkgSrv, err = r.setupPackageServer(ctx)
		if pkgSrv != nil {
			defer pkgSrv.Close()
		}
		if err != nil {
			return err
		}
	}

	r.dispatchTests(ctx, cancel, eg, baseTargets, fuchsiaTargets, primaryTarget, pkgSrv, testsPath, experiments)

	if err := eg.Wait(); err != nil {
		return err
	}

	return nil
}

// runPreflights runs opaque preflight commands passed to botanist from
// the calling infrastructure.
func (r *RunCommand) runPreflights(ctx context.Context) error {
	logger.Debugf(ctx, "checking for preflights")
	botfilePath := os.Getenv("SWARMING_BOT_FILE")
	if botfilePath == "" {
		return nil
	}
	data, err := os.ReadFile(botfilePath)
	if err != nil {
		return err
	}
	if len(data) == 0 {
		// There were no commands in the botfile, exit out.
		return nil
	}
	type preflightCommands struct {
		Commands [][]string `json:"commands"`
	}
	var cmds preflightCommands
	if err := json.Unmarshal(data, &cmds); err != nil {
		return err
	}
	runner := subprocess.Runner{
		Env: os.Environ(),
	}
	for _, c := range cmds.Commands {
		logger.Debugf(ctx, "running preflight %s", c)
		if err := runner.Run(ctx, c, subprocess.RunOptions{Setpgid: true}); err != nil {
			return err
		}
	}
	if len(cmds.Commands) > 0 {
		// Some preflight commands can cause side effects that take up to 30s.
		time.Sleep(30 * time.Second)
	}
	logger.Debugf(ctx, "done running preflights")
	return nil
}

// createTestbedConfig creates a configuration file that describes the targets
// attached and returns the path to the file.
func (r *RunCommand) createTestbedConfig(baseTargets []targets.Base) ([]any, string, error) {
	var testbedConfig []any
	for _, t := range baseTargets {
		c, err := t.TestConfig(r.expectsSSH)
		if err != nil {
			return nil, "", err
		}
		testbedConfig = append(testbedConfig, c)
	}

	data, err := json.Marshal(testbedConfig)
	if err != nil {
		return nil, "", err
	}

	f, err := os.CreateTemp("", "testbed_config")
	if err != nil {
		return nil, "", err
	}
	defer f.Close()
	if _, err := f.Write(data); err != nil {
		return nil, "", err
	}
	return testbedConfig, f.Name(), nil
}

// dumpSyslogOverSerial runs log_listener over serial to collect logs that may
// help with debugging. This is intended to be used when SSH connection fails to
// get some information about the failure mode prior to exiting.
func (r *RunCommand) dumpSyslogOverSerial(ctx context.Context, socketPath string) error {
	socket, err := serial.NewSocket(ctx, socketPath)
	if err != nil {
		return fmt.Errorf("newSerialSocket failed: %w", err)
	}
	defer socket.Close()
	if err := serial.RunDiagnostics(ctx, socket); err != nil {
		return fmt.Errorf("failed to run serial diagnostics: %w", err)
	}
	// Dump the existing syslog buffer. This may not work if pkg-resolver is not
	// up yet, in which case it will just print nothing.
	cmds := []serial.Command{
		{Cmd: syslog.LogListenerWithArgs("--dump_logs", "yes"), SleepDuration: 5 * time.Second},
	}
	if err := serial.RunCommands(ctx, socket, cmds); err != nil {
		return fmt.Errorf("failed to dump syslog over serial: %w", err)
	}
	return nil
}

func (r *RunCommand) runAgainstTarget(ctx context.Context, t targets.FuchsiaTarget, testsPath string, testbedConfig []any, testbedConfigPath string) error {
	testrunnerEnv := map[string]string{
		constants.NodenameEnvKey:                   t.Nodename(),
		constants.SerialSocketEnvKey:               t.SerialSocketPath(),
		constants.ECCableEnvKey:                    os.Getenv(constants.ECCableEnvKey),
		constants.TestbedConfigEnvKey:              testbedConfigPath,
		testrunnerconstants.TestTimeoutScaleFactor: strconv.Itoa(r.testTimeoutScaleFactor),
	}

	if r.expectsSSH && botanist.GetExperiments(r.experiments).Contains(botanist.UseFFXMonitor) {
		// The shared_data directory is currently only created when using ffx monitor
		// so we should only add the env var if we expect it to exist.
		testrunnerEnv[constants.FFXSharedDataEnvKey] = t.GetSharedData()
	}

	if r.expectsSSH {
		ipv6, err := t.IPv6()
		if err != nil {
			return err
		}
		ipv4, err := t.IPv4()
		if err != nil {
			return err
		}
		addr, err := targets.IPAddr(t)
		if err != nil {
			return err
		}

		testrunnerEnv[constants.DeviceAddrEnvKey] = addr.String()
		testrunnerEnv[constants.IPv4AddrEnvKey] = ipv4.String()
		testrunnerEnv[constants.IPv6AddrEnvKey] = ipv6.String()

		// FUCHSIA_HOST_TOOLS is used to allow test-pilot to find runtime dependencies
		// when the test is being run. Those dependencies must reside in the same
		// directory as ffx. bundles/infra/test/BUILD.gn adds the dependencies, and
		// tools/integration/testsharder/task_requests.go registers the tools with
		// test_sharder.
		testrunnerEnv[constants.HostToolsEnvKey] = filepath.Dir(r.ffxPath)
	}

	// One would assume this should only be provisioned when paving, but
	// there are some tests that attempt to SSH into a netbooted image that
	// has our SSH keys baked into it. Therefore, we add the SSH key to the
	// environment unconditionally. Additionally, some tools like FFX often
	// require the SSH key path to be absolute (https://fxbug.dev/42051867).
	if t.SSHKey() != "" {
		absKeyPath, err := filepath.Abs(t.SSHKey())
		if err != nil {
			return err
		}
		testrunnerEnv[constants.SSHKeyEnvKey] = absKeyPath
	}

	// TODO(https://fxbug.dev/42063235): testrunner does heavy use of env
	// variables. Setting these env variables is temporary until we refactor
	// testrunner to take these variables as arguments or flags.
	for k, v := range testrunnerEnv {
		err := os.Setenv(k, v)
		if err != nil {
			return fmt.Errorf("error setting env variable %s=%s. %w", k, v, err)
		}
	}
	setEnviron(t.FFXEnv())
	r.testrunnerOptions.FFX = t.GetFFX().FFXInstance
	r.testrunnerOptions.Experiments = botanist.GetExperiments(r.experiments)
	r.testrunnerOptions.FuchsiaTarget = t
	r.testrunnerOptions.TestbedConfig = testbedConfig

	if err := testrunner.SetupAndExecute(ctx, r.testrunnerOptions, testsPath); err != nil {
		return fmt.Errorf("testrunner with flags: %v, with timeout: %s, failed: %w", r.testrunnerOptions, r.timeout, err)
	}
	return nil
}

// setEnviron sets |environ| into the os.Env.
// The string in the environ slice must be in the format "key=value".
func setEnviron(environ []string) {
	for _, env := range environ {
		keyval := strings.Split(env, "=")
		os.Setenv(keyval[0], keyval[1])
	}
}

func (r *RunCommand) Execute(ctx context.Context, f *flag.FlagSet, _ ...any) subcommands.ExitStatus {
	args := f.Args()
	if len(args) == 0 {
		return subcommands.ExitUsageError
	}

	// If the TestOutDirEnvKey was set, that means botanist is being run in an infra
	// setting and thus needs an isolated environment.
	testOutDir, needsIsolatedEnv := os.LookupEnv(testrunnerconstants.TestOutDirEnvKey)
	cleanUp, err := environment.Ensure(needsIsolatedEnv)
	if err != nil {
		logger.Errorf(ctx, "failed to setup environment: %s", err)
		return subcommands.ExitFailure
	}
	defer cleanUp()

	if needsIsolatedEnv {
		// Use a temp directory for the output directory which we will move to the
		// actual testOutDir once the command completes. Otherwise, when run in a
		// swarming task, a subprocess that doesn't properly finish could still be
		// writing to the out dir as we try to upload the contents with the swarming
		// task outputs which will result in the swarming bot failing with BOT_DIED.
		tmpOutDir, err := os.MkdirTemp("", "")
		if err != nil {
			return subcommands.ExitFailure
		}
		if err := os.Setenv(testrunnerconstants.TestOutDirEnvKey, tmpOutDir); err != nil {
			return subcommands.ExitFailure
		}
		defer func() {
			if skippedFiles, err := osmisc.CopyDir(tmpOutDir, testOutDir, osmisc.SkipUnknownFiles); err != nil {
				logger.Errorf(ctx, "failed to copy outputs to %s: %s", testOutDir, err)
				// TODO(https://fxbug.dev/42079078): If we fail to copy outputs, at least copy
				// the ffx logs over so we can debug. Remove when attached bug is
				// fixed.
				if r.ffxPath != "" {
					ffxLogsDir := filepath.Join("ffx_outputs", "ffx_logs")
					if _, err := os.Stat(filepath.Join(testOutDir, ffxLogsDir)); os.IsNotExist(err) {
						if _, err := osmisc.CopyDir(filepath.Join(tmpOutDir, ffxLogsDir), filepath.Join(testOutDir, ffxLogsDir), osmisc.RaiseError); err != nil {
							logger.Errorf(ctx, "failed to copy ffx logs to %s: %s", filepath.Join(testOutDir, ffxLogsDir), err)
						}
					}
				}
			} else if len(skippedFiles) > 0 {
				skippedFilesTxt := filepath.Join(testOutDir, "skipped_files.txt")
				if err := os.WriteFile(skippedFilesTxt, []byte(strings.Join(skippedFiles, "\n")), os.ModePerm); err != nil {
					logger.Errorf(ctx, "failed to write %s: %s\nskipped files: %s", skippedFilesTxt, err, skippedFiles)
				}
			}

			if err := os.Setenv(testrunnerconstants.TestOutDirEnvKey, testOutDir); err != nil {
				logger.Errorf(ctx, "failed to reset %s to %s: %s", testrunnerconstants.TestOutDirEnvKey, testOutDir, err)
			}
			if err := os.RemoveAll(tmpOutDir); err != nil {
				logger.Errorf(ctx, "failed to remove temp outputs dir %s: %s", tmpOutDir, err)
			}
		}()
	}

	if err := r.execute(ctx, args); err != nil {
		logger.Errorf(ctx, "%s: %s", constants.BotanistFailedMsg, err)
		return subcommands.ExitFailure
	}
	return subcommands.ExitSuccess
}

func (r *RunCommand) deriveTargetsFromFile(ctx context.Context, targetOpts targets.Options) ([]targets.Base, []targets.FuchsiaTarget, error) {
	data, err := os.ReadFile(r.configFile)
	if err != nil {
		return nil, nil, fmt.Errorf("%s: %w", constants.ReadConfigFileErrorMsg, err)
	}
	var configs []json.RawMessage
	if err := json.Unmarshal(data, &configs); err != nil {
		return nil, nil, fmt.Errorf("could not unmarshal config file as a JSON list: %w", err)
	}

	var baseTargets []targets.Base
	var fuchsiaTargets []targets.FuchsiaTarget

	for _, config := range configs {
		t, err := targets.FromJSON(ctx, config, targetOpts)
		if err != nil {
			return nil, nil, err
		}
		baseTargets = append(baseTargets, t)
		if f, ok := t.(targets.FuchsiaTarget); ok {
			fuchsiaTargets = append(fuchsiaTargets, f)
		}
	}

	if len(fuchsiaTargets) == 0 {
		return nil, nil, fmt.Errorf("no Fuchsia targets found")
	}

	return baseTargets, fuchsiaTargets, nil
}
