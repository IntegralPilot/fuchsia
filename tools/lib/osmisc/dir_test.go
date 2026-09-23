// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSE file.

package osmisc

import (
	"bytes"
	"os"
	"path/filepath"
	"testing"
)

func TestIsDir(t *testing.T) {
	t.Run("returns false if path does not exist", func(t *testing.T) {
		path := filepath.Join(t.TempDir(), "does-not-exist")
		isDir, err := IsDir(path)
		if err != nil {
			t.Fatal(err)
		}
		if isDir {
			t.Fatalf("Expected IsDir to return false for a non-existent directory")
		}
	})

	t.Run("returns false if path is a regular file", func(t *testing.T) {
		path := filepath.Join(t.TempDir(), "foo.txt")
		f, err := os.Create(path)
		if err != nil {
			t.Fatal(err)
		}
		if err := f.Close(); err != nil {
			t.Fatal(err)
		}

		isDir, err := IsDir(path)
		if err != nil {
			t.Fatal(err)
		}
		if isDir {
			t.Fatalf("Expected IsDir to return false for a regular file")
		}
	})

	t.Run("returns true if path is a directory", func(t *testing.T) {
		path := filepath.Join(t.TempDir(), "foo")
		if err := os.Mkdir(path, 0o700); err != nil {
			t.Fatal(err)
		}

		isDir, err := IsDir(path)
		if err != nil {
			t.Fatal(err)
		}
		if !isDir {
			t.Fatalf("Expected IsDir to return true for a directory that exists")
		}
	})
}

func TestDirIsEmpty(t *testing.T) {
	tmpDir := t.TempDir()

	// Directory should start off empty.
	empty, err := DirIsEmpty(tmpDir)
	if err != nil {
		t.Fatal(err.Error())
	} else if !empty {
		t.Fatalf("directory should be empty")
	}

	if err := os.WriteFile(filepath.Join(tmpDir, "file.txt"), []byte("content"), 0o600); err != nil {
		t.Fatal(err.Error())
	}

	// Directory should now be non-empty.
	empty, err = DirIsEmpty(tmpDir)
	if err != nil {
		t.Fatal(err.Error())
	} else if empty {
		t.Fatalf("directory should be non-empty")
	}

	// Non-existent directories should be empty by convention.
	nonexistentSubdir := filepath.Join(tmpDir, "i_dont_exist")
	empty, err = DirIsEmpty(nonexistentSubdir)
	if err != nil {
		t.Fatal(err.Error())
	} else if !empty {
		t.Fatalf("non-existent directory should be empty")
	}
}

func TestCopyDir(t *testing.T) {
	tmpDir := t.TempDir()

	srcDir := filepath.Join(tmpDir, "src")
	if err := os.Mkdir(srcDir, 0o700); err != nil {
		t.Fatalf("failed to create src %q: %v", srcDir, err)
	}

	dstDir := filepath.Join(tmpDir, "dst")
	if err := os.Mkdir(dstDir, 0o700); err != nil {
		t.Fatalf("failed to create dst %q: %v", dstDir, err)
	}

	otherDir := filepath.Join(tmpDir, "other")
	if err := os.Mkdir(otherDir, 0o700); err != nil {
		t.Fatalf("failed to create other %q: %v", otherDir, err)
	}

	srcPaths := map[string]string{
		"a":     "a",
		"b":     "",
		"b/a":   "b/a",
		"b/b":   "b/b",
		"b/c":   "",
		"b/c/a": "b/c/a",
	}

	for path, contents := range srcPaths {
		path = filepath.Join(srcDir, path)

		if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
			t.Fatalf("failed to create path %q: %v", path, err)
		}

		if contents == "" {
			if err := os.MkdirAll(path, 0o700); err != nil {
				t.Fatalf("failed to create path %q: %v", path, err)
			}
		} else {
			if err := os.WriteFile(path, []byte(contents), 0o400); err != nil {
				t.Fatalf("failed to write contents to src %q: %v", path, err)
			}
		}
	}
	otherSymlinkTargetPath := filepath.Join(otherDir, "z")
	if err := os.WriteFile(otherSymlinkTargetPath, []byte("z"), 0o400); err != nil {
		t.Fatalf("failed to write contents to src %q: %v", otherSymlinkTargetPath, err)
	}

	expectedDstSymlinkTarget := map[string]string{}
	if err := os.Symlink(filepath.Join(srcDir, "a"), filepath.Join(srcDir, "c")); err != nil {
		t.Fatalf("failed to create symlink: %v", err)
	}
	srcPaths["c"] = "a"
	expectedDstSymlinkTarget[filepath.Join(dstDir, "c")] = filepath.Join(dstDir, "a")
	if err := os.Symlink(otherSymlinkTargetPath, filepath.Join(srcDir, "d")); err != nil {
		t.Fatalf("failed to create symlink: %v", err)
	}
	// If the symlink target is outside the srcDir, the symlink target should not
	// be copied to the dstDir. Instead the dstPath should point towards the
	// original symlink target.
	srcPaths["d"] = "../other/z"
	expectedDstSymlinkTarget[filepath.Join(dstDir, "d")] = otherSymlinkTargetPath

	if skippedFiles, err := CopyDir(srcDir, dstDir, SkipUnknownFiles); err != nil {
		t.Fatalf("failed to copy directory: %v", err)
	} else if len(skippedFiles) > 0 {
		t.Fatalf("unexpected skipped files: %v", skippedFiles)
	}

	err := filepath.Walk(dstDir, func(dstPath string, dstInfo os.FileInfo, err error) error {
		if dstDir == dstPath {
			return nil
		}

		if err != nil {
			return err
		}

		relPath, err := filepath.Rel(dstDir, dstPath)
		if err != nil {
			t.Fatalf("failed to get relative path for dst: %v", err)
		}

		if _, ok := srcPaths[relPath]; !ok {
			t.Fatalf("unknown path %q", relPath)
		}
		delete(srcPaths, relPath)

		srcPath := filepath.Join(srcDir, relPath)
		srcInfo, err := os.Lstat(srcPath)
		if err != nil {
			t.Fatalf("failed to stat src %q: %v", srcPath, err)
		}

		if srcInfo.Mode() != dstInfo.Mode() {
			t.Fatalf("mode not copied from %q to %q: %s != %s", srcPath, dstPath, srcInfo.Mode(), dstInfo.Mode())
		}

		if !srcInfo.IsDir() {
			srcContents, err := os.ReadFile(srcPath)
			if err != nil {
				t.Fatalf("failed to read src %q: %v", srcPath, err)
			}

			dstContents, err := os.ReadFile(dstPath)
			if err != nil {
				t.Fatalf("failed to read dst %q: %v", dstPath, err)
			}

			if !bytes.Equal(srcContents, dstContents) {
				t.Fatalf("src %q has different contents than dst %q", srcPath, dstPath)
			}

			if srcInfo.Mode()&os.ModeType == os.ModeSymlink {
				dstSymlinkTarget, err := filepath.EvalSymlinks(dstPath)
				if err != nil {
					t.Fatalf("cannot find target for symlink in the dstDir %q", dstPath)
				}
				if dstSymlinkTarget != expectedDstSymlinkTarget[dstPath] {
					t.Fatalf("the destination symlink target %s did not match the expected symlink target %s", dstSymlinkTarget, expectedDstSymlinkTarget[dstPath])
				}
			}
		}

		return nil
	})
	if err != nil {
		t.Fatalf("failed to walk directory: %v", err)
	}

	if len(srcPaths) != 0 {
		t.Fatalf("some files not copied: %+v", srcPaths)
	}
}
