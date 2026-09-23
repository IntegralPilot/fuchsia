// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package subprocess

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"sort"
	"strconv"
	"strings"
	"syscall"
	"time"

	"go.fuchsia.dev/fuchsia/tools/lib/clock"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

const (
	// cleanupGracePeriod is the time period we allow the subprocess to complete in
	// after we send a SIGTERM.
	cleanupGracePeriod = 10 * time.Second
)

// Runner is a Runner that runs commands as local subprocesses.
type Runner struct {
	// Dir is the working directory of the subprocesses; if unspecified, that
	// of the current process will be used.
	Dir string

	// Env is the environment of the subprocess, following the usual convention of a list of
	// strings of the form "<environment variable name>=<value>".
	Env []string
}

// RunOptions represents all the optional fields that may be passed into Run().
type RunOptions struct {
	// Stdout is the writer to which the subprocess's stdout should be directed.
	// It defaults to os.Stdout.
	Stdout io.Writer

	// Stderr is the writer to which the subprocess's stderr should be directed.
	// It defaults to os.Stderr.
	Stderr io.Writer

	// Stderr is the reader to which the subprocess's stdin should be connected.
	// It is unset by default.
	Stdin io.Reader

	// Env is the environment of the subprocess, appended to Runner.Env.
	Env []string

	// Dir is the directory in which the subprocess should be run. It inherits
	// Runner.Dir if unset.
	Dir string

	// Whether to set a new process group ID so that we can kill the subprocess
	// and any of its children.
	Setpgid bool
}

// Command returns an *exec.Cmd from the provided command args and run options.
func (r *Runner) Command(command []string, options RunOptions) *exec.Cmd {
	cmd := exec.Command(command[0], command[1:]...)

	if options.Stdout == nil {
		options.Stdout = os.Stdout
	}
	cmd.Stdout = options.Stdout
	if options.Stderr == nil {
		options.Stderr = os.Stderr
	}
	cmd.Stderr = options.Stderr
	// Don't inherit stdin by default because the majority of subprocesses don't
	// require access to stdin, and using os.Stdin results in any grandchildren
	// processes not being cleaned up due to the pgid logic below.
	if options.Stdin != nil {
		cmd.Stdin = options.Stdin
	}

	if options.Dir == "" {
		options.Dir = r.Dir
	}
	cmd.Dir = options.Dir

	// Inherit the parent process's environment. `exec.Command` inherits the
	// parent's environment if `Env` is nil, so unconditionally inheriting the
	// parent's environment means that adding a single environment variable to a
	// command that previously didn't have any will not implicitly remove the
	// inheritance.
	cmd.Env = append(os.Environ(), r.Env...)
	cmd.Env = append(cmd.Env, options.Env...)

	// For some reason, adding the child to the same process group as the
	// current process disconnects it from stdin. So don't do it if we're
	// running a potentially interactive command that has access to stdin. Those
	// cases are less likely to involve chains of subprocesses anyway, so it's
	// not as important to be able to kill the entire chain.
	if cmd.Stdin != os.Stdin && options.Setpgid {
		cmd.SysProcAttr = &syscall.SysProcAttr{
			// Set a process group ID so we can kill the entire group, meaning
			// the process and any of its children.
			Setpgid: true,
		}
	}
	return cmd
}

// Run runs a command generated from the provided command args and run options.
func (r *Runner) Run(ctx context.Context, command []string, options RunOptions) error {
	cmd := r.Command(command, options)
	return r.RunCommand(ctx, cmd)
}

// RunCommand runs a command until completion or until a context is canceled, in
// which case the subprocess is killed so that no subprocesses it spun up are
// orphaned.
func (r *Runner) RunCommand(ctx context.Context, cmd *exec.Cmd) error {
	if len(cmd.Env) > 0 {
		logger.Tracef(ctx, "environment of subprocess: %v", cmd.Env)
	}

	// Ensure that the context still exists before running the subprocess.
	if ctx.Err() != nil {
		logger.Debugf(ctx, "context exited before starting subprocess")
		return ctx.Err()
	}

	logger.Debugf(ctx, "starting: %v", cmd.Args)
	if err := cmd.Start(); err != nil {
		return err
	}

	return WaitForCmd(ctx, cmd)
}

// procInfo contains information about a process and its children.
type procInfo struct {
	// pid is the process ID.
	pid int
	// ppid is the parent process ID.
	ppid int
	// comm is the name of the executable.
	comm string
	// cmdline is the command line arguments of the process.
	cmdline string
	// children are the child processes.
	children []*procInfo
}

// render generates an ASCII representation of the process tree.
func (p *procInfo) render(prefix string, isLast bool, isRoot bool) []string {
	var connector string
	if !isRoot {
		if isLast {
			connector = "└── "
		} else {
			connector = "├── "
		}
	}

	line := fmt.Sprintf("%s%sPID: %d, comm: %q, cmdline: %q", prefix, connector, p.pid, p.comm, p.cmdline)
	lines := []string{line}

	var childPrefix string
	if !isRoot {
		if isLast {
			childPrefix = prefix + "    "
		} else {
			childPrefix = prefix + "│   "
		}
	}

	sort.Slice(p.children, func(i, j int) bool {
		return p.children[i].pid < p.children[j].pid
	})
	for i, child := range p.children {
		lines = append(lines, child.render(childPrefix, i == len(p.children)-1, false)...)
	}
	return lines
}

// processSet represents a collection of processes.
type processSet struct {
	procs map[int]*procInfo
}

// roots returns the root processes in the set.
func (s *processSet) roots() []*procInfo {
	for _, p := range s.procs {
		p.children = nil
	}
	var roots []*procInfo
	for _, p := range s.procs {
		if parent, ok := s.procs[p.ppid]; ok && p.ppid != p.pid {
			parent.children = append(parent.children, p)
		} else {
			roots = append(roots, p)
		}
	}
	sort.Slice(roots, func(i, j int) bool {
		return roots[i].pid < roots[j].pid
	})
	return roots
}

// render generates the entire ASCII tree as a slice of strings.
func (s *processSet) render() []string {
	roots := s.roots()
	var lines []string
	for i, root := range roots {
		lines = append(lines, root.render("", i == len(roots)-1, true)...)
	}
	return lines
}

// loadProcessSet reads the /proc filesystem to collect information about all running processes.
func loadProcessSet(ctx context.Context) (*processSet, error) {
	dir, err := os.Open("/proc")
	if err != nil {
		return nil, err
	}
	defer dir.Close()

	files, err := dir.Readdirnames(0)
	if err != nil {
		return nil, err
	}

	procs := make(map[int]*procInfo)
	for _, file := range files {
		pid, err := strconv.Atoi(file)
		if err != nil {
			continue
		}

		p := &procInfo{pid: pid}

		if comm, err := os.ReadFile(fmt.Sprintf("/proc/%d/comm", pid)); err == nil {
			p.comm = strings.TrimSpace(string(comm))
		} else if !errors.Is(err, os.ErrNotExist) {
			logger.Debugf(ctx, "failed to read /proc/%d/comm: %v", pid, err)
		}

		if cmdline, err := os.ReadFile(fmt.Sprintf("/proc/%d/cmdline", pid)); err == nil {
			p.cmdline = strings.TrimSpace(strings.ReplaceAll(string(cmdline), "\x00", " "))
		} else if !errors.Is(err, os.ErrNotExist) {
			logger.Debugf(ctx, "failed to read /proc/%d/cmdline: %v", pid, err)
		}

		if status, err := os.ReadFile(fmt.Sprintf("/proc/%d/status", pid)); err == nil {
			for _, line := range strings.Split(string(status), "\n") {
				if strings.HasPrefix(line, "PPid:") {
					ppidStr := strings.TrimSpace(strings.TrimPrefix(line, "PPid:"))
					if ppid, err := strconv.Atoi(ppidStr); err == nil {
						p.ppid = ppid
					}
					break
				}
			}
		} else if !errors.Is(err, os.ErrNotExist) {
			logger.Debugf(ctx, "failed to read /proc/%d/status: %v", pid, err)
		}
		procs[pid] = p
	}
	return &processSet{procs: procs}, nil
}

// This is an inhouse implementation of `ps` for Linux which isn't guaranteed to
// be available on all the systems we run on.
func logRunningProcesses(ctx context.Context) {
	set, err := loadProcessSet(ctx)
	if err != nil {
		logger.Errorf(ctx, "failed to load processes: %v", err)
		return
	}
	if lines := set.render(); len(lines) > 0 {
		logger.Warningf(ctx, "Running processes:\n%s", strings.Join(lines, "\n"))
	}
}

// WaitForCmd waits for the command to finish and sends a SIGTERM and SIGKILL
// if the command doesn't complete on its own.
func WaitForCmd(ctx context.Context, cmd *exec.Cmd) error {
	errs := make(chan error)

	go func() {
		errs <- cmd.Wait()
		if cmd.ProcessState != nil {
			retcode := cmd.ProcessState.ExitCode()
			logger.Debugf(ctx, "Subprocess completed with exit code %d: %v", retcode, cmd.Args)
		}
	}()

	pgidSet := cmd.SysProcAttr != nil && cmd.SysProcAttr.Setpgid
	select {
	case err := <-errs:
		// Process is done so no need to worry about cleanup. Just exit.
		return err
	case <-ctx.Done():
		logRunningProcesses(ctx)
		logger.Debugf(ctx, "sending SIGTERM to process %d: %v", cmd.Process.Pid, cmd.Args)
		if err := cmd.Process.Signal(syscall.SIGTERM); err != nil {
			logger.Debugf(ctx, "exited cmd %v with error: %s", cmd.Args, err)
		}

		// Wait up to `cleanupGracePeriod` for the subprocess to exit on its
		// own. If it takes too long we'll SIGKILL it.
		select {
		case <-errs:
			// The command has completed but it may still have child processes
			// running that we would like to clean up if possible. Sending a
			// SIGKILL to clean up the entire process group will only work if
			// the pgid is set.
			if pgidSet {
				killProcess(ctx, cmd, pgidSet)
			}
		case <-clock.After(ctx, cleanupGracePeriod):
			killProcess(ctx, cmd, pgidSet)
			// Wait for the subprocess to complete after killing it.
			<-errs
		}
		// Return the context error instead of the error returned by cmd.Wait()
		// to indicate to the caller that the command failed as a result of a
		// context cancellation; in this case the error returned by cmd.Wait()
		// will generally be more confusing than meaningful.
		return ctx.Err()
	}
}

// killProcess makes a best-effort attempt at killing the subprocess specified
// by `cmd`, along with all of its child processes if `pgidSet` is true.
func killProcess(ctx context.Context, cmd *exec.Cmd, pgidSet bool) {
	logger.Debugf(ctx, "killing process %d", cmd.Process.Pid)
	pgid := cmd.Process.Pid
	if pgidSet {
		// Negating the process ID means interpret it as a process group ID, so
		// we kill the subprocess and all of its children.
		pgid = -pgid
	}
	if err := syscall.Kill(pgid, syscall.SIGKILL); err != nil {
		// ESRCH is "no such process", meaning the process has already exited.
		if !errors.Is(err, syscall.ESRCH) {
			logger.Debugf(ctx, "killed cmd %v with error: %s", cmd.Args, err)
		}
	}
}
