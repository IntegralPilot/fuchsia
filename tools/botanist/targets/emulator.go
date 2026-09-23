// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package targets

import (
	"bufio"
	"context"
	"encoding/hex"
	"fmt"
	"io"
	"math/rand"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"syscall"
	"time"

	"go.fuchsia.dev/fuchsia/tools/botanist"
	"go.fuchsia.dev/fuchsia/tools/botanist/constants"
	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/lib/ffxutil"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
	"go.fuchsia.dev/fuchsia/tools/net/sshutil"

	"github.com/creack/pty"
)

const (
	// DefaultEmulatorNodename is the default nodename given to an emulator target.
	// See generate_mac_address() in //src/developer/ffx/plugins/emulator/commands/start/src/pbm.rs
	// for rationale for choosing this nodename.
	DefaultEmulatorNodename = "fuchsia-5254-1234-1234"

	// Minimum number of bytes of entropy bits required for the kernel's PRNG.
	minEntropyBytes uint = 32 // 256 bits

	// qemuSystemPrefix is the prefix of the QEMU binary name, which is of the
	// form qemu-system-<QEMU arch suffix>.
	qemuSystemPrefix = "qemu-system"

	// aemuBinaryName is the name of the AEMU binary.
	aemuBinaryName = "emulator"
)

type Target string

const (
	TargetARM64   Target = "arm64"
	TargetRISCV64 Target = "riscv64"
	TargetX64     Target = "x64"
)

// qemuTargetMapping maps the Fuchsia target name to the name recognized by QEMU.
var qemuTargetMapping = map[Target]Target{
	TargetX64:     "x86_64",
	TargetARM64:   "aarch64",
	TargetRISCV64: "riscv64",
}

// EmulatorConfig gives the common configuration for supported emulators.
type EmulatorConfig struct {
	// Path is a path to a directory that contains the emulator executable.
	Path string `json:"path"`

	// Target is the target to emulate.
	Target Target `json:"target"`

	// Emulator describes flags to pass to `ffx emu start`.
	Emulator build.EmulatorInfo `json:"emulator"`

	// Serial gives whether to create a 'serial device' for the emulator instance.
	// This option should be used judiciously, as it can slow the process down.
	Serial bool `json:"serial"`

	// FirmwareDir is a path to a directory of QEMU firmware prebuilts. The
	// contents consist of installed CIPD packages (each at
	// FirmwareDir/$cipd_pkg_name).
	FirmwareDir string `json:"firmware_dir"`

	// Path to the fvm host tool.
	FVMTool string `json:"fvm_tool"`

	// Path to the zbi host tool.
	ZBITool string `json:"zbi_tool"`
}

// Emulator represents a Fuchsia emulator target. The supported emulator types are
// QEMU, AEMU, and crosvm.
type Emulator struct {
	*genericFuchsiaTarget
	binary  string
	c       chan error
	config  EmulatorConfig
	mac     [6]byte
	opts    Options
	process *os.Process
	ptm     *os.File
	serial  io.ReadWriteCloser
}

var _ FuchsiaTarget = (*Emulator)(nil)

func getBinaryName(emuType string, target Target) (string, error) {
	switch emuType {
	case "aemu":
		return aemuBinaryName, nil
	case "qemu":
		qemuTarget, ok := qemuTargetMapping[target]
		if !ok {
			return "", fmt.Errorf("invalid target %q", target)
		}
		return fmt.Sprintf("%s-%s", qemuSystemPrefix, qemuTarget), nil
	case "crosvm":
		return "crosvm", nil
	default:
		return "", fmt.Errorf("unknown emulator type found: %q", emuType)
	}
}

// NewEmulator returns a new Emulator target with a given configuration.
func NewEmulator(ctx context.Context, config EmulatorConfig, opts Options, emuType string) (*Emulator, error) {
	binary, err := getBinaryName(emuType, config.Target)
	if err != nil {
		return nil, err
	}
	t := &Emulator{
		binary: binary,
		c:      make(chan error),
		config: config,
		opts:   opts,
	}
	r := rand.New(rand.NewSource(time.Now().UnixNano()))
	if _, err := r.Read(t.mac[:]); err != nil {
		return nil, fmt.Errorf("failed to generate random MAC: %w", err)
	}
	// Ensure that the generated MAC address is unicast
	// https://en.wikipedia.org/wiki/MAC_address#Unicast_vs._multicast_(I/G_bit)
	t.mac[0] &= ^uint8(0x01)

	if config.Serial {
		// We can run the emulator 'in a terminal' by creating a pseudoterminal
		// slave and attaching it as the process' std(in|out|err) streams.
		// Running it in a terminal - and redirecting serial to stdio - allows
		// us to use the associated pseudoterminal master as the 'serial device'
		// for the instance.
		var err error
		// TODO(joshuaseaton): Figure out how to manage ownership so that this may
		// be closed.
		t.ptm, t.serial, err = pty.Open()
		if err != nil {
			return nil, fmt.Errorf("failed to create ptm/pts pair: %w", err)
		}
	}
	base, err := newGenericFuchsia(ctx, DefaultEmulatorNodename, "", []string{opts.SSHKey}, t.serial)
	if err != nil {
		return nil, err
	}
	t.genericFuchsiaTarget = base
	return t, nil
}

// Nodename returns the name of the target node.
func (t *Emulator) Nodename() string { return DefaultEmulatorNodename }

// Serial returns the serial device associated with the target for serial i/o.
func (t *Emulator) Serial() io.ReadWriteCloser {
	return t.serial
}

// SSHKey returns the private SSH key path associated with a previously embedded authorized key.
func (t *Emulator) SSHKey() string {
	return t.opts.SSHKey
}

// SSHClient creates and returns an SSH client connected to the emulator target.
func (t *Emulator) SSHClient() (*sshutil.Client, error) {
	addr, err := t.IPv6()
	if err != nil {
		return nil, err
	}
	return t.sshClient(addr, "qemu")
}

// Start starts the emulator target.
func (t *Emulator) Start(ctx context.Context, args []string, pbPath string, isBootTest bool) (err error) {
	if t.process != nil {
		return fmt.Errorf("a process has already been started with PID %d", t.process.Pid)
	}

	if t.config.Path == "" {
		return fmt.Errorf("directory must be set")
	}
	bin := filepath.Join(t.config.Path, t.binary)
	absBin, err := normalizeFile(bin)
	if err != nil {
		return fmt.Errorf("could not find %s binary %q: %w", t.binary, bin, err)
	}

	if pbPath == "" {
		return fmt.Errorf("missing product bundle")
	}

	allKernelArgs := []string{
		// Manually set nodename, since MAC is randomly generated.
		"zircon.nodename=" + t.nodename,
		// The system will halt on a kernel panic instead of rebooting.
		"kernel.halt-on-panic=true",
		// Disable kernel lockup detector in emulated environments to prevent false alarms from
		// potentially oversubscribed hosts.
		"kernel.lockup-detector.critical-section-threshold-ms=0",
		"kernel.lockup-detector.critical-section-fatal-threshold-ms=0",
		"kernel.lockup-detector.heartbeat-period-ms=0",
		"kernel.lockup-detector.heartbeat-age-threshold-ms=0",
		"kernel.lockup-detector.heartbeat-age-fatal-threshold-ms=0",
	}

	// Add entropy to simulate bootloader entropy.
	entropy := make([]byte, minEntropyBytes)
	if _, err := rand.Read(entropy); err == nil {
		allKernelArgs = append(allKernelArgs, "kernel.entropy-mixin="+hex.EncodeToString(entropy))
	}
	// Do not print colors.
	allKernelArgs = append(allKernelArgs, "TERM=dumb")
	if t.config.Target == TargetX64 {
		// Necessary to redirect to stdout.
		allKernelArgs = append(allKernelArgs, "kernel.serial=legacy")
	}
	// Add kernel args specified by the builder.
	allKernelArgs = append(allKernelArgs, args...)
	// Add kernel args specified by the shard environment.
	allKernelArgs = append(allKernelArgs, t.config.Emulator.KernelArgs...)

	cwd, err := os.Getwd()
	if err != nil {
		return err
	}
	edk2Dir := filepath.Join(t.config.FirmwareDir, "edk2", "qemu-"+string(t.config.Target))
	tools := ffxutil.EmuTools{
		Emulator:   absBin,
		FVM:        t.config.FVMTool,
		ZBI:        t.config.ZBITool,
		UEFI_arm64: filepath.Join(edk2Dir, "QEMU_EFI.fd"),
		UEFI_x64:   filepath.Join(edk2Dir, "OVMF_CODE.fd"),
	}

	deviceType := os.Getenv("FUCHSIA_DEVICE_TYPE")

	// This firmware is only usable for arm64 QEMU TCG.
	if t.config.Target == TargetARM64 && deviceType == "QEMU" && t.config.Emulator.Accel == build.AccelNone {
		tools.SecureBootloaderArm64 = filepath.Join(
			t.config.FirmwareDir,
			"arm-trusted-firmware-qemu-bios",
			"qemu_fw.bios",
		)
	}
	startArgs := ffxutil.EmuStartArgs{
		Engine:        strings.ToLower(deviceType),
		ProductBundle: filepath.Join(cwd, pbPath),
		KernelArgs:    allKernelArgs,
		Device:        t.config.Emulator.Device,
		Accel:         t.config.Emulator.Accel,
	}
	if t.config.Emulator.Uefi {
		startArgs.ExtraEmuArgs = []string{
			"--uefi",
			"--vbmeta-key",
			t.config.Emulator.VbmetaKey,
			"--vbmeta-key-metadata",
			t.config.Emulator.VbmetaKeyMetadata,
		}
		// TODO(https://fxbug.dev/369416059): ffx emu --uefi calls mkfs-msdosfs which
		// looks for the FUCHSIA_BUILD_DIR env var. Remove once this dependency is no
		// longer needed.
		if err := os.Setenv("FUCHSIA_BUILD_DIR", cwd); err != nil {
			return err
		}
	}

	cmd, err := t.ffx.EmuStartConsole(ctx, cwd, DefaultEmulatorNodename, tools, startArgs)
	if err != nil {
		return err
	}

	stdout, stderr, flush := botanist.NewStdioWriters(ctx, t.binary)
	// Since serial is already printed to stdout, we can copy it to the
	// serial logfile as well.
	var serialLog *os.File
	closeSerialLog := func() {
		if serialLog == nil {
			return
		}
		if err := serialLog.Close(); err != nil {
			logger.Debugf(ctx, "failed to close %s", serialLog.Name())
		}
	}
	// If t.config.Serial, the serial output will be captured through the CaptureSerialLog()
	// function. Otherwise copy the emulator output.
	if !t.config.Serial && t.opts.SerialLogDir != "" {
		logfile := filepath.Join(t.opts.SerialLogDir, "serial_log.txt")
		logfileAbsPath, err := filepath.Abs(logfile)
		if err != nil {
			return fmt.Errorf("cannot get absolute path for %q: %w", logfile, err)
		}
		if err := os.MkdirAll(filepath.Dir(logfileAbsPath), os.ModePerm); err != nil {
			return fmt.Errorf("failed to make parent dirs of %q: %w", logfileAbsPath, err)
		}
		serialLog, err := os.Create(logfileAbsPath)
		if err != nil {
			return fmt.Errorf("failed to create %s", logfileAbsPath)
		}
		if err := os.Setenv(constants.SerialLogEnvKey, logfileAbsPath); err != nil {
			logger.Debugf(ctx, "failed to set %s to %s", constants.SerialLogEnvKey, logfileAbsPath)
		}
		serialWriter := botanist.NewLineWriter(botanist.NewTimestampWriter(serialLog), "")
		stdout = io.MultiWriter(serialWriter, stdout)
	}

	if t.ptm != nil {
		cmd.Stdin = t.ptm
		cmd.Stdout = io.MultiWriter(t.ptm, stdout)
		cmd.SysProcAttr = &syscall.SysProcAttr{
			Setctty: true,
			Setsid:  true,
		}
	} else {
		cmd.Stdout = stdout
		cmd.SysProcAttr = &syscall.SysProcAttr{
			// Set a process group ID so we can kill the entire group, meaning
			// the process and any of its children.
			Setpgid: true,
		}
	}

	// The emulator's stderr will not contain logging from the guest, so it
	// should just inherit the parent's stderr (and not play a part in the
	// 'serial' logging).
	cmd.Stderr = stderr

	logger.Debugf(ctx, "%s invocation:\n%s", t.binary, strings.Join(append([]string{cmd.Path}, cmd.Args...), " "))

	if err := cmd.Start(); err != nil {
		flush()
		closeSerialLog()
		return fmt.Errorf("failed to start: %w", err)
	}
	t.process = cmd.Process

	go func() {
		err := cmd.Wait()
		flush()
		closeSerialLog()
		if err != nil {
			logger.Debugf(ctx, "processes using 'qemu' interface:\n%s", strings.Join(processesUsingQEMUInterface(), "\n"))
			err = fmt.Errorf("%s invocation error: %w", t.binary, err)
		}
		t.c <- err
	}()
	return nil
}

// processesUsingQEMUInterface returns processes that have the 'qemu' interface open.
func processesUsingQEMUInterface() (cmds []string) {
	pattern := regexp.MustCompile(`iff:.*qemu`)
	fdpaths, err := filepath.Glob("/proc/[1-9][0-9]*/fdinfo/*")
	if err != nil {
		return nil
	}
	for _, p := range fdpaths {
		func() {
			f, err := os.Open(p)
			if err != nil {
				// Just skip if we can't open the file.
				return
			}
			defer f.Close()
			matched := pattern.MatchReader(bufio.NewReader(f))
			if !matched {
				return
			}
			cmdPath := filepath.Clean(filepath.Join(p, "../../cmdline"))
			cmdBytes, err := os.ReadFile(cmdPath)
			if err != nil {
				// Just skip if we can't read the file.
				return
			}
			cmds = append(cmds, string(cmdBytes))
		}()
	}
	return cmds
}

func (t *Emulator) CaptureSerialLog(filename string) error {
	if !t.config.Serial {
		// Don't do anything here. We'll copy the output from
		// the emulator instead.
		return nil
	}
	return t.genericFuchsiaTarget.CaptureSerialLog(filename)
}

// Stop stops the emulator target.
func (t *Emulator) Stop() error {
	var err error
	if err = t.ffx.EmuStopAll(botanist.GetLoggerCtx(t.targetCtx)); err != nil {
		logger.Debugf(t.targetCtx, "failed to stop emulator: %s", err)
		if t.process == nil {
			return fmt.Errorf("%s target has not yet been started", t.binary)
		}
	}
	// Kill the process in case the ffx emu stop command failed to but didn't return
	// an error, which might happen if it gets called before the start command has
	// actually set up the emulator.
	if t.process != nil {
		logger.Debugf(t.targetCtx, "Sending SIGKILL to emulator process %d", t.process.Pid)
		err = t.process.Kill()
	}
	t.process = nil
	t.genericFuchsiaTarget.Stop()
	return err
}

// Wait waits for the QEMU target to stop.
func (t *Emulator) Wait(ctx context.Context) error {
	select {
	case err := <-t.c:
		return err
	case <-ctx.Done():
		return ctx.Err()
	}
}

// Config returns fields describing the target.
func (t *Emulator) TestConfig(expectsSSH bool) (any, error) {
	return TestbedConfig(t, expectsSSH, nil)
}

func normalizeFile(path string) (string, error) {
	if _, err := os.Stat(path); err != nil {
		return "", err
	}
	absPath, err := filepath.Abs(path)
	if err != nil {
		return "", err
	}
	return absPath, nil
}
