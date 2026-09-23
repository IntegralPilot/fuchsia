// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package config

import (
	"fmt"
	"os"
	"path/filepath"
)

// AddPolicyException creates or updates a policy exception JSON config file on disk.
func (c *MasterConfig) AddPolicyException(checkName, targetPath, bug, description string) (string, error) {
	relPath := targetPath
	if filepath.IsAbs(relPath) {
		rel, err := filepath.Rel(c.FuchsiaDir, relPath)
		if err == nil {
			relPath = rel
		}
	}

	configDir := filepath.Join(c.ConfigRootFor(relPath), "policy_exceptions", checkName)
	if err := os.MkdirAll(configDir, 0755); err != nil {
		return "", fmt.Errorf("failed to create config directory %s: %w", configDir, err)
	}

	baseName := c.FindProjectBasename(relPath)
	destFile := filepath.Join(configDir, baseName+".json")

	if err := UpdateConfigFile(destFile, func(cfg *ConfigFile) {
		if cfg.PolicyExceptions == nil {
			cfg.PolicyExceptions = make(map[string][]AllowlistEntry)
		}
		entry := AllowlistEntry{
			Bug:         bug,
			Description: description,
			Paths:       []string{relPath},
		}
		cfg.PolicyExceptions[checkName] = append(cfg.PolicyExceptions[checkName], entry)
	}); err != nil {
		return "", err
	}

	return destFile, nil
}
