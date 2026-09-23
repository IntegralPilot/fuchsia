// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package config

import (
	"fmt"
	"os"
	"path"
	"path/filepath"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/boundary"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/classify"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/discover"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/validate"
)

// MasterConfig is the fully assembled configuration injected into the pipeline stages.
// It is constructed by the ConfigBuilder during the Assembly Phase by merging all
// scattered JSON files from the open-source and proprietary assets directories.
type MasterConfig struct {
	// FuchsiaDir is the authoritative absolute root path of the workspace.
	FuchsiaDir string

	// --- Injected into Discoverer (Stage 1) ---
	Discover discover.Config

	// --- Injected into Grouper (Stage 2) ---
	Boundary boundary.Config

	// --- Injected into Classifier (Stage 4) ---
	Classify classify.Config

	// --- Injected into Validator (Stage 5) ---
	Validate validate.Config
}

func resolveFuchsiaDir(dir string) string {
	if dir != "" {
		return dir
	}
	if env := os.Getenv("FUCHSIA_DIR"); env != "" {
		return env
	}
	return "."
}

// IsPrivateProject returns true if the project path belongs to a proprietary/private
// repository. It prevents open-source compliance configs from being contaminated.
func (c *MasterConfig) IsPrivateProject(projectPath string) bool {
	if c == nil {
		return false
	}
	return c.Boundary.IsPrivateProject(projectPath)
}

// ManifestNameFor returns the manifest package name for a given project path.
func (c *MasterConfig) ManifestNameFor(projectPath string) string {
	if c == nil {
		return ""
	}
	return c.Boundary.ManifestNameFor(projectPath)
}

// OutOfTreeReadmes returns the boundary stage out-of-tree readmes map.
func (c *MasterConfig) OutOfTreeReadmes() map[string]string {
	if c == nil {
		return nil
	}
	return c.Boundary.OutOfTreeReadmes
}

// HasPolicyException returns true if relPath is listed in the given policy exceptions map.
func (c *MasterConfig) HasPolicyException(policyName string, relPath string) bool {
	if c == nil || c.Validate.PolicyExceptions == nil {
		return false
	}
	if list, ok := c.Validate.PolicyExceptions[policyName]; ok {
		_, exists := list[relPath]
		return exists
	}
	return false
}

// IsSkipped checks if the given absolute path matches any skip rules in the configuration.
func (c *MasterConfig) IsSkipped(absPath string) bool {
	if c == nil {
		return false
	}

	relPath, err := filepath.Rel(c.FuchsiaDir, absPath)
	if err != nil {
		return c.Discover.SkipAnywhere[filepath.Base(absPath)]
	}

	slashRel := filepath.ToSlash(relPath)
	for _, part := range strings.Split(slashRel, "/") {
		if c.Discover.SkipAnywhere[part] {
			return true
		}
	}

	for p := slashRel; p != "." && p != "" && p != "/"; p = path.Dir(p) {
		if c.Discover.SkipPaths[p] {
			return true
		}
	}

	return false
}

// AssetRootFor returns the base asset directory path for a project based on its privacy state.
func (c *MasterConfig) AssetRootFor(projectPath string) string {
	fDir := resolveFuchsiaDir("")
	if c != nil {
		fDir = resolveFuchsiaDir(c.FuchsiaDir)
	}

	cleanPath := projectPath
	if strings.HasPrefix(cleanPath, "//") {
		cleanPath = strings.TrimPrefix(cleanPath, "//")
	} else if filepath.IsAbs(cleanPath) {
		if rel, err := filepath.Rel(fDir, cleanPath); err == nil && !strings.HasPrefix(rel, "..") {
			cleanPath = rel
		}
	}
	cleanPath = strings.TrimLeft(filepath.ToSlash(filepath.Clean(cleanPath)), "/")

	// 1. Check if the project resides in a specific vendor repository (e.g. vendor/<name>/...)
	parts := strings.Split(cleanPath, "/")
	if len(parts) >= 2 && parts[0] == "vendor" && parts[1] != "" && parts[1] != "." && parts[1] != ".." {
		vendorAssetDir := filepath.Join(fDir, "vendor", parts[1], "tools", "check-licenses", "assets")
		if info, err := os.Stat(vendorAssetDir); err == nil && info.IsDir() {
			return vendorAssetDir
		}
	}

	// 2. Check if the project is private (via Jiri manifest metadata, package name, or vendor path)
	if strings.HasPrefix(cleanPath, "vendor/") || cleanPath == "vendor" || c.IsPrivateProject(cleanPath) {
		return filepath.Join(fDir, "vendor", "google", "tools", "check-licenses", "assets")
	}

	// 3. Default to the root open-source repository
	return filepath.Join(fDir, "tools", "check-licenses", "assets")
}

// ConfigRootFor returns the base config directory path for a project based on its privacy state.
func (c *MasterConfig) ConfigRootFor(projectPath string) string {
	return filepath.Join(c.AssetRootFor(projectPath), "configs")
}

// ReadmeRootFor returns the base readmes asset directory path for a project based on its privacy state.
func (c *MasterConfig) ReadmeRootFor(projectPath string) string {
	return filepath.Join(c.AssetRootFor(projectPath), "readmes")
}

// RootReadmePath returns the absolute file path to the primary first-party repository root README.fuchsia.
func (c *MasterConfig) RootReadmePath() string {
	return filepath.Join(c.ReadmeRootFor(""), "README.fuchsia")
}

// ResolveReadmeWritePath determines the appropriate filesystem target path for writing
// or updating a project's README.fuchsia file. If a project resides under prebuilt/, its README
// is redirected to the corresponding virtual assets directory.
func (c *MasterConfig) ResolveReadmeWritePath(projectRoot, currentReadmePath string) (string, error) {
	relRoot, err := filepath.Rel(c.FuchsiaDir, projectRoot)
	if err != nil {
		return currentReadmePath, err
	}
	if strings.HasPrefix(relRoot, "prebuilt/") && !strings.Contains(currentReadmePath, "assets/readmes") {
		writePath := filepath.Join(c.ReadmeRootFor(relRoot), relRoot, "README.fuchsia")
		if err := os.MkdirAll(filepath.Dir(writePath), 0755); err != nil {
			return "", fmt.Errorf("failed to create asset directory for %s: %w", projectRoot, err)
		}
		return writePath, nil
	}
	if physPath := c.Boundary.OutOfTreeReadmes[relRoot]; physPath != "" {
		absPhys := physPath
		if !filepath.IsAbs(absPhys) {
			absPhys = filepath.Join(c.FuchsiaDir, absPhys)
		}
		if absPhys != filepath.Join(c.FuchsiaDir, relRoot, "README.fuchsia") {
			return absPhys, nil
		}
	}
	return currentReadmePath, nil
}

// ResolveAndValidatePath normalizes the fuchsia root and ensures the given input path
// resides safely within that root. Returns the relative target path,
// or an error if the path escapes the root workspace.
func (c *MasterConfig) ResolveAndValidatePath(inputPath string) (string, error) {
	c.FuchsiaDir = resolveFuchsiaDir(c.FuchsiaDir)
	absFuchsiaDir, err := filepath.Abs(c.FuchsiaDir)
	if err != nil {
		return "", fmt.Errorf("failed to get absolute path for fuchsia_dir %s: %w", c.FuchsiaDir, err)
	}
	c.FuchsiaDir = absFuchsiaDir

	if strings.HasPrefix(inputPath, "//") {
		inputPath = filepath.Join(absFuchsiaDir, strings.TrimPrefix(inputPath, "//"))
	}

	absInputPath, err := filepath.Abs(inputPath)
	if err != nil {
		return "", fmt.Errorf("failed to get absolute path for %s: %w", inputPath, err)
	}

	rel, err := filepath.Rel(absFuchsiaDir, absInputPath)
	if err != nil || strings.HasPrefix(rel, "..") {
		return "", fmt.Errorf("path %s must be inside fuchsia root %s", inputPath, absFuchsiaDir)
	}
	if rel == "." {
		rel = ""
	}
	return rel, nil
}

// CategoryForLicense returns the approved policy category (e.g., Restricted, Notice, Exception)
// for the given license name, or "Uncategorized" if the license is unapproved or unknown.
func (c *MasterConfig) CategoryForLicense(licenseName string) string {
	if c != nil {
		return c.Classify.CategoryForLicense(licenseName)
	}
	return "Uncategorized"
}

// NewMasterConfig initializes an empty configuration ready to be populated by the builder.
func NewMasterConfig(fuchsiaDir string) *MasterConfig {
	fuchsiaDir = resolveFuchsiaDir(fuchsiaDir)
	if abs, err := filepath.Abs(fuchsiaDir); err == nil {
		fuchsiaDir = abs
	}
	c := &MasterConfig{
		FuchsiaDir: fuchsiaDir,
		Discover: discover.Config{
			SkipPaths:    make(map[string]bool),
			SkipAnywhere: make(map[string]bool),
		},
		Boundary: boundary.NewConfig(),

		Classify: classify.NewConfig(),
		Validate: validate.Config{
			PolicyExceptions:    make(map[string]map[string]validate.RuleMetadata),
			AllowedLicenses:     make(map[string]map[string]validate.RuleMetadata),
			CopyrightExtensions: make(map[string]bool),
		},
	}
	c.Classify.PatternDirs = []string{
		filepath.Join(c.AssetRootFor(""), "patterns"),
		filepath.Join(c.AssetRootFor("vendor/google"), "patterns"),
	}
	return c
}

// --- JSON File Schemas ---
// These structs define the expected shape of the individual JSON files scattered
// throughout the `assets/configs/` directory. Any JSON file can contain any combination
// of these fields, allowing configuration to be organized by project or by theme.

type ConfigFile struct {
	RequireBug         *bool `json:"require_bug,omitempty"`
	RequireDescription *bool `json:"require_description,omitempty"`
	IsBaseline         *bool `json:"is_baseline,omitempty"`

	Includes            []string                    `json:"includes,omitempty"`
	Skips               []SkipEntry                 `json:"skips,omitempty"`
	TargetExtensions    *ExtensionEntry             `json:"target_extensions,omitempty"`
	CopyrightExtensions *ExtensionEntry             `json:"copyright_extensions,omitempty"`
	Barriers            []BarrierEntry              `json:"barriers,omitempty"`
	PolicyExceptions    map[string][]AllowlistEntry `json:"policy_exceptions,omitempty"`
	AllowedLicenses     map[string][]AllowlistEntry `json:"allowed_licenses,omitempty"`
}

// IsBugRequired returns true if exceptions in this file must include a Bug link.
func (f *ConfigFile) IsBugRequired(configPath string) bool {
	if f != nil {
		if f.RequireBug != nil {
			return *f.RequireBug
		}
		if f.IsBaseline != nil && *f.IsBaseline {
			return false
		}
	}
	switch filepath.Base(configPath) {
	case "default.json", "hidden_dirs.json", "test_dirs.json", "bazel_vendor.json":
		return false
	default:
		return true
	}
}

// IsDescriptionRequired returns true if exceptions in this file must include a Description.
func (f *ConfigFile) IsDescriptionRequired() bool {
	if f != nil && f.RequireDescription != nil {
		return *f.RequireDescription
	}
	return false
}

type SkipEntry struct {
	Bug          string   `json:"bug,omitempty"`
	Description  string   `json:"description,omitempty"`
	Paths        []string `json:"paths"`
	SkipAnywhere bool     `json:"skipAnywhere,omitempty"`
}

type ExtensionEntry struct {
	Description string   `json:"description,omitempty"`
	Extensions  []string `json:"extensions"` // E.g., [".cc", ".cpp", ".h"]
}

type BarrierEntry struct {
	Bug         string   `json:"bug,omitempty"`
	Description string   `json:"description,omitempty"`
	Paths       []string `json:"paths"`
	Exceptions  []string `json:"exceptions,omitempty"`
}

type AllowlistEntry struct {
	Bug         string   `json:"bug,omitempty"`
	Description string   `json:"description,omitempty"`
	Paths       []string `json:"paths"` // Paths to allowed projects/files
}

// LEGACY CONSTANTS
const (
	PolicyCheckAllLicenseTextsMustBeRecognized                     = validate.PolicyUnrecognizedLicense
	PolicyCheckAllFuchsiaAuthorSourceFilesMustHaveCopyrightHeaders = validate.PolicyFuchsiaCopyright
	PolicyCheckAllProjectsMustHaveALicense                         = validate.PolicyNoLicense
	CheckNameReadmeFuchsiaNeedsUpdate                              = validate.CheckReadmeNeedsUpdate
	CheckNameAllLicensePatternUsagesMustBeApproved                 = validate.CheckPatternApproval
)

type RuleMetadata = validate.RuleMetadata

var ValidPolicyChecks = func() map[string]bool {
	m := make(map[string]bool)
	for _, p := range validate.ValidPolicies() {
		m[p] = true
	}
	return m
}()
