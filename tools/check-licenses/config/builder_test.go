// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package config

import (
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

func TestBuilder_Assemble(t *testing.T) {
	// Create a mock fuchsia directory
	fuchsiaDir := t.TempDir()

	// Scaffold the recursive config system
	seedConfig := filepath.Join(fuchsiaDir, "tools", "check-licenses", "config.json")
	os.MkdirAll(filepath.Dir(seedConfig), 0755)
	os.WriteFile(seedConfig, []byte(`{"includes": ["tools/check-licenses/assets", "vendor/google/tools/check-licenses/assets"]}`), 0644)

	// 1. Setup mock open-source assets
	osAssets := filepath.Join(fuchsiaDir, "tools", "check-licenses", "assets")
	osConfigs := filepath.Join(osAssets, "configs")
	if err := os.MkdirAll(filepath.Join(osConfigs, "skips"), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(osConfigs, "target_extensions"), 0755); err != nil {
		t.Fatal(err)
	}

	osSkipBytes, _ := json.Marshal(ConfigFile{
		Skips: []SkipEntry{
			{
				Bug:   "https://fxbug.dev/12345",
				Paths: []string{"out", "prebuilt"},
			},
			{
				Bug:          "https://fxbug.dev/12345",
				Paths:        []string{".git"},
				SkipAnywhere: true,
			},
		},
	})
	os.WriteFile(filepath.Join(osConfigs, "skips", "test_skip.json"), osSkipBytes, 0644)

	osExtBytes, _ := json.Marshal(ConfigFile{
		TargetExtensions: &ExtensionEntry{
			Extensions: []string{".cc", "rs"}, // intentionally missing dot on rs
		},
	})
	os.WriteFile(filepath.Join(osConfigs, "target_extensions", "test_ext.json"), osExtBytes, 0644)

	if err := os.MkdirAll(filepath.Join(osConfigs, "copyright_extensions"), 0755); err != nil {
		t.Fatal(err)
	}
	osCopyExtBytes, _ := json.Marshal(ConfigFile{
		CopyrightExtensions: &ExtensionEntry{
			Extensions: []string{".cc", "py"}, // intentionally missing dot on py
		},
	})
	os.WriteFile(filepath.Join(osConfigs, "copyright_extensions", "test_copy_ext.json"), osCopyExtBytes, 0644)

	if err := os.MkdirAll(filepath.Join(osConfigs, "allowed_licenses", "Restricted", "GPL-2.0"), 0755); err != nil {
		t.Fatal(err)
	}
	os.WriteFile(filepath.Join(osConfigs, "allowed_licenses", "Restricted", "GPL-2.0", "foo.json"), []byte(`{"allowed_licenses": {"GPL-2.0": []}}`), 0644)

	if err := os.MkdirAll(filepath.Join(osAssets, "readmes", "third_party", "foo", "src"), 0755); err != nil {
		t.Fatal(err)
	}
	readmePath := filepath.Join(osAssets, "readmes", "third_party", "foo", "README.fuchsia")
	os.WriteFile(readmePath, []byte("Name: Foo"), 0644)

	// 2. Setup mock proprietary vendor assets
	vendorAssets := filepath.Join(fuchsiaDir, "vendor", "google", "tools", "check-licenses", "assets")
	vendorConfigs := filepath.Join(vendorAssets, "configs")
	if err := os.MkdirAll(filepath.Join(vendorConfigs, "projects"), 0755); err != nil {
		t.Fatal(err)
	}
	vendorAllowBytes, _ := json.Marshal(ConfigFile{
		PolicyExceptions: map[string][]AllowlistEntry{
			"AllProjectsMustHaveALicense": {
				{
					Paths: []string{"vendor/google/secret_project"},
				},
			},
		},
	})
	// Test the "default.json" exception allows missing bug field
	os.WriteFile(filepath.Join(vendorConfigs, "projects", "default.json"), vendorAllowBytes, 0644)

	configDir := filepath.Join(fuchsiaDir, "tools", "check-licenses")
	if err := os.MkdirAll(configDir, 0755); err != nil {
		t.Fatal(err)
	}
	rootConfigBytes, _ := json.Marshal(ConfigFile{
		Includes: []string{
			"tools/check-licenses/assets",
			"vendor/google/tools/check-licenses/assets",
		},
	})
	os.WriteFile(filepath.Join(configDir, "config.json"), rootConfigBytes, 0644)

	// 3. Run the Builder
	builder := NewBuilder(fuchsiaDir)
	if err := builder.Assemble(); err != nil {
		t.Fatalf("Assemble failed: %v", err)
	}

	config := builder.Config

	// 4. Verify results
	expectedSkips := map[string]bool{"out": true, "prebuilt": true}
	if !reflect.DeepEqual(config.Discover.SkipPaths, expectedSkips) {
		t.Errorf("Expected skips %v, got %v", expectedSkips, config.Discover.SkipPaths)
	}

	expectedExts := map[string]bool{".cc": true, ".rs": true}
	if !reflect.DeepEqual(config.Classify.TargetExtensions, expectedExts) {
		t.Errorf("Expected extensions %v, got %v", expectedExts, config.Classify.TargetExtensions)
	}

	expectedCopyExts := map[string]bool{".cc": true, ".py": true}
	if !reflect.DeepEqual(config.Validate.CopyrightExtensions, expectedCopyExts) {
		t.Errorf("Expected copyright extensions %v, got %v", expectedCopyExts, config.Validate.CopyrightExtensions)
	}

	logicalPath := filepath.Join("third_party", "foo")
	if config.Boundary.OutOfTreeReadmes[logicalPath] != readmePath {
		t.Errorf("Expected OutOfTreeReadmes[%q] = %q, got %q", logicalPath, readmePath, config.Boundary.OutOfTreeReadmes[logicalPath])
	}

	if cat := config.CategoryForLicense("GPL-2.0"); cat != "Restricted" {
		t.Errorf("Expected CategoryForLicense('GPL-2.0') = 'Restricted', got %q", cat)
	}
	if cat := config.CategoryForLicense("Nonexistent"); cat != "Uncategorized" {
		t.Errorf("Expected CategoryForLicense('Nonexistent') = 'Uncategorized', got %q", cat)
	}

	if _, ok := config.Validate.PolicyExceptions["AllProjectsMustHaveALicense"]["vendor/google/secret_project"]; !ok {
		t.Errorf("Expected vendor project to be in the policy exceptions list")
	}

	if !config.IsSkipped(filepath.Join(fuchsiaDir, "out")) {
		t.Errorf("Expected out directory to be skipped")
	}
	if !config.IsSkipped(filepath.Join(fuchsiaDir, "prebuilt")) {
		t.Errorf("Expected prebuilt directory to be skipped")
	}
	if !config.IsSkipped(filepath.Join(fuchsiaDir, ".git")) {
		t.Errorf("Expected .git directory to be skipped")
	}
	if !config.IsSkipped(filepath.Join(fuchsiaDir, ".git", "config")) {
		t.Errorf("Expected nested file inside .git directory to be skipped")
	}
	if !config.Discover.SkipAnywhere[".git"] {
		t.Errorf("Expected .git to be in Discover.SkipAnywhere")
	}
	if config.IsSkipped(filepath.Join(fuchsiaDir, "src")) {
		t.Errorf("Expected src directory not to be skipped")
	}

	if config.Validate.VirtualReadmeDir == nil {
		t.Fatal("Expected config.Validate.VirtualReadmeDir to be set")
	}
	if got := config.Validate.VirtualReadmeDir("third_party/foo"); got != "tools/check-licenses/assets/readmes" {
		t.Errorf("VirtualReadmeDir('third_party/foo') = %q, want 'tools/check-licenses/assets/readmes'", got)
	}
	if got := config.Validate.VirtualReadmeDir("vendor/google/secret_project"); got != "vendor/google/tools/check-licenses/assets/readmes" {
		t.Errorf("VirtualReadmeDir('vendor/google/secret_project') = %q, want 'vendor/google/tools/check-licenses/assets/readmes'", got)
	}
}

func TestBuilder_LoadManifests(t *testing.T) {
	fuchsiaDir := t.TempDir()

	// Create mock manifests directory
	manifestsDir := filepath.Join(fuchsiaDir, "manifests")
	if err := os.MkdirAll(manifestsDir, 0755); err != nil {
		t.Fatal(err)
	}

	// Create a mock public manifest
	publicManifest := filepath.Join(manifestsDir, "public_projects")
	publicContent := `<?xml version="1.0" encoding="UTF-8"?>
<manifest>
  <project name="third_party/acpica" path="third_party/acpica"/>
  <packages>
    <package name="fuchsia/third_party/clang" path="prebuilt/third_party/clang"/>
    <package name="fuchsia_internal/firmware/amlogic-video" path="prebuilt/media/firmware/amlogic-decoder"/>
  </packages>
</manifest>`
	os.WriteFile(publicManifest, []byte(publicContent), 0644)

	// Create mock integration directory
	integrationDir := filepath.Join(fuchsiaDir, "integration", "internal", "vendor", "google")
	if err := os.MkdirAll(integrationDir, 0755); err != nil {
		t.Fatal(err)
	}

	// Create a mock private manifest
	privateManifest := filepath.Join(integrationDir, "third_party")
	privateContent := `<?xml version="1.0" encoding="UTF-8"?>
<manifest>
  <project name="eigen/fuchsia" path="vendor/third_party/eigen3"/>
</manifest>`
	os.WriteFile(privateManifest, []byte(privateContent), 0644)

	builder := NewBuilder(fuchsiaDir)
	if err := builder.LoadManifests(); err != nil {
		t.Fatalf("LoadManifests failed: %v", err)
	}

	config := builder.Config

	// Verify mappings
	expectedMappings := map[string]string{
		filepath.Clean("third_party/acpica"):                      "third_party/acpica",
		filepath.Clean("prebuilt/third_party/clang"):              "fuchsia/third_party/clang",
		filepath.Clean("prebuilt/media/firmware/amlogic-decoder"): "fuchsia_internal/firmware/amlogic-video",
		filepath.Clean("vendor/third_party/eigen3"):               "eigen/fuchsia",
	}

	for path, expectedName := range expectedMappings {
		if name, ok := config.Boundary.ManifestProjectNames[path]; !ok || name != expectedName {
			t.Errorf("Expected ManifestProjectNames[%q] = %q, got %q", path, expectedName, name)
		}
	}

	// Verify IsPrivateProject
	tests := []struct {
		path      string
		isPrivate bool
	}{
		{"third_party/acpica", false},
		{"prebuilt/third_party/clang", false},
		{"prebuilt/media/firmware/amlogic-decoder", true},
		{"prebuilt/media/firmware/amlogic-decoder/firmware.bin", true},
		{"vendor/third_party/eigen3", true},
		{"vendor/third_party/eigen3/LICENSE", true},
		{"third_party/vendor/foo", false},
		{"src/lib/vendor/bar", false},
		{"unknown/project", false},
	}

	for _, tc := range tests {
		if got := config.IsPrivateProject(tc.path); got != tc.isPrivate {
			t.Errorf("IsPrivateProject(%q) = %v, want %v", tc.path, got, tc.isPrivate)
		}
		expectedAssetRoot := filepath.Join(fuchsiaDir, "tools", "check-licenses", "assets")
		if tc.isPrivate {
			expectedAssetRoot = filepath.Join(fuchsiaDir, "vendor", "google", "tools", "check-licenses", "assets")
		}
		if gotRoot := config.AssetRootFor(tc.path); gotRoot != expectedAssetRoot {
			t.Errorf("AssetRootFor(%q) = %q, want %q", tc.path, gotRoot, expectedAssetRoot)
		}
		expectedConfigRoot := filepath.Join(expectedAssetRoot, "configs")
		if gotRoot := config.ConfigRootFor(tc.path); gotRoot != expectedConfigRoot {
			t.Errorf("ConfigRootFor(%q) = %q, want %q", tc.path, gotRoot, expectedConfigRoot)
		}
		expectedReadmeRoot := filepath.Join(expectedAssetRoot, "readmes")
		if gotRoot := config.ReadmeRootFor(tc.path); gotRoot != expectedReadmeRoot {
			t.Errorf("ReadmeRootFor(%q) = %q, want %q", tc.path, gotRoot, expectedReadmeRoot)
		}
	}

	expectedRootReadme := filepath.Join(fuchsiaDir, "tools", "check-licenses", "assets", "readmes", "README.fuchsia")
	if gotPath := config.RootReadmePath(); gotPath != expectedRootReadme {
		t.Errorf("RootReadmePath() = %q, want %q", gotPath, expectedRootReadme)
	}
}

// TestMasterConfig_ResolveReadmeWritePath tests redirection of prebuilt README.fuchsia writes to virtual asset paths.
func TestMasterConfig_ResolveReadmeWritePath(t *testing.T) {
	tempDir := t.TempDir()
	cfg := NewMasterConfig(tempDir)
	projRoot := filepath.Join(tempDir, "prebuilt", "third_party", "testproj")
	readmePath := filepath.Join(projRoot, "README.fuchsia")

	writePath, err := cfg.ResolveReadmeWritePath(projRoot, readmePath)
	if err != nil {
		t.Fatalf("ResolveReadmeWritePath() error = %v", err)
	}
	expectedSuffix := filepath.Join("tools", "check-licenses", "assets", "readmes", "prebuilt", "third_party", "testproj", "README.fuchsia")
	if !strings.HasSuffix(writePath, expectedSuffix) {
		t.Errorf("Expected writePath to have suffix %q, got %q", expectedSuffix, writePath)
	}
}

func TestMasterConfig_AssetRootFor_MultiVendor(t *testing.T) {
	tempDir := t.TempDir()
	cfg := NewMasterConfig(tempDir)

	// Partner vendor directory with check-licenses assets
	partnerAssetDir := filepath.Join(tempDir, "vendor", "partner", "tools", "check-licenses", "assets")
	if err := os.MkdirAll(partnerAssetDir, 0755); err != nil {
		t.Fatal(err)
	}

	// Google vendor directory with check-licenses assets
	googleAssetDir := filepath.Join(tempDir, "vendor", "google", "tools", "check-licenses", "assets")
	if err := os.MkdirAll(googleAssetDir, 0755); err != nil {
		t.Fatal(err)
	}

	// 1. Partner vendor project -> partner asset root
	if got := cfg.AssetRootFor("vendor/partner/somedir"); got != partnerAssetDir {
		t.Errorf("AssetRootFor('vendor/partner/somedir') = %q, want %q", got, partnerAssetDir)
	}

	// 2. Google vendor project -> google asset root
	if got := cfg.AssetRootFor("vendor/google/somedir"); got != googleAssetDir {
		t.Errorf("AssetRootFor('vendor/google/somedir') = %q, want %q", got, googleAssetDir)
	}

	// 3. Other vendor without check-licenses assets -> falls back to google vendor assets (private)
	if got := cfg.AssetRootFor("vendor/other/somedir"); got != googleAssetDir {
		t.Errorf("AssetRootFor('vendor/other/somedir') = %q, want %q", got, googleAssetDir)
	}

	// 4. Public project -> public assets
	publicAssetDir := filepath.Join(tempDir, "tools", "check-licenses", "assets")
	if got := cfg.AssetRootFor("third_party/foo"); got != publicAssetDir {
		t.Errorf("AssetRootFor('third_party/foo') = %q, want %q", got, publicAssetDir)
	}

	// 5. Absolute path to partner vendor project
	absPartnerDir := filepath.Join(tempDir, "vendor", "partner", "somedir")
	if got := cfg.AssetRootFor(absPartnerDir); got != partnerAssetDir {
		t.Errorf("AssetRootFor(%q) = %q, want %q", absPartnerDir, got, partnerAssetDir)
	}

	// 6. Absolute path to google vendor project
	absGoogleDir := filepath.Join(tempDir, "vendor", "google", "somedir")
	if got := cfg.AssetRootFor(absGoogleDir); got != googleAssetDir {
		t.Errorf("AssetRootFor(%q) = %q, want %q", absGoogleDir, got, googleAssetDir)
	}

	// 7. GN-style repository path (//) to partner vendor project
	if got := cfg.AssetRootFor("//vendor/partner/somedir"); got != partnerAssetDir {
		t.Errorf("AssetRootFor('//vendor/partner/somedir') = %q, want %q", got, partnerAssetDir)
	}

	// 8. GN-style repository path (//) to google vendor project
	if got := cfg.AssetRootFor("//vendor/google/somedir"); got != googleAssetDir {
		t.Errorf("AssetRootFor('//vendor/google/somedir') = %q, want %q", got, googleAssetDir)
	}

	// 9. GN-style repository path (//) to public project
	if got := cfg.AssetRootFor("//third_party/foo"); got != publicAssetDir {
		t.Errorf("AssetRootFor('//third_party/foo') = %q, want %q", got, publicAssetDir)
	}
}
