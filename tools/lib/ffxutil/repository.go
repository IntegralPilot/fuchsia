// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package ffxutil

import (
	"context"
	"encoding/json"
	"fmt"
	"strconv"
)

// Output for `repository server list`.
type RepoServersList struct {
	Ok struct {
		Data []struct {
			Name string `json:"name"`
		} `json:"data"`
	} `json:"ok"`
}

// StartPackageServer starts a package repository server at the given repoDir.
func (f *FFXInstance) StartPackageServer(ctx context.Context, name, address, repoDir string, port int) error {
	if f.target == "" {
		return fmt.Errorf("no target is set")
	}
	args := []string{"repository", "server", "start", "--foreground", "--address", fmt.Sprintf("%s:%d", address, port),
		"--repository", name, "--repo-path", repoDir,
		"--trusted-root", fmt.Sprintf("%s/repository/9.root.json", repoDir),
		"--alias", "fuchsia.com", "--alias", "chromium.org",
		"--no-device",
	}
	return f.invoker(args).setTimeout(0).setTarget(f.target).setMachineFormat(MachineRaw).run(ctx)
}

// StopPackageServer stops the package repository server with the given name.
func (f *FFXInstance) StopPackageServer(ctx context.Context, name string, port int) error {
	if f.target == "" {
		return fmt.Errorf("no target is set")
	}
	return f.invoker([]string{"repository", "server", "stop", name, "--port", strconv.Itoa(port)}).setTarget(f.target).run(ctx)
}

// ListPackageServer lists the running package servers.
func (f *FFXInstance) ListPackageServer(ctx context.Context) ([]string, error) {
	if f.target == "" {
		return nil, fmt.Errorf("no target is set")
	}
	i := f.invoker([]string{"repository", "server", "list"}).setTarget(f.target).setCaptureOutput()
	err := i.run(ctx)
	var result RepoServersList
	if err := json.Unmarshal(i.output.Bytes(), &result); err != nil {
		return nil, err
	}
	servers := []string{}
	for _, server := range result.Ok.Data {
		servers = append(servers, server.Name)
	}

	return servers, err
}

// Forward forwards connections between the host and the target.
func (f *FFXInstance) Forward(ctx context.Context, port int) error {
	if f.target == "" {
		return fmt.Errorf("no target is set")
	}
	return f.invoker([]string{"forward", fmt.Sprintf("%d<=0", port)}).setTarget(f.target).setTimeout(0).run(ctx)
}

// RegisterRepository registers the given package repository server with the target.
func (f *FFXInstance) RegisterRepository(ctx context.Context, repoName string, port int, overrideAddr string) error {
	if f.target == "" {
		return fmt.Errorf("no target is set")
	}
	args := []string{"target", "repository", "register", "--repository", repoName, "--port", strconv.Itoa(port),
		"--alias", "fuchsia.com", "--alias", "chromium.org",
		"--alias-conflict-mode", "replace"}
	if overrideAddr != "" {
		args = append(args, "--address-override", overrideAddr)
	}
	return f.invoker(args).setTarget(f.target).run(ctx)
}
