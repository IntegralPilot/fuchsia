// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package osmisc

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// UnknownFilesMode is a mode that describes how to deal with
// unknown file types.
type UnknownFilesMode int

const (
	RaiseError UnknownFilesMode = iota
	SkipUnknownFiles
)

// IsDir determines whether a given path exists *and* is a directory. It will
// return false (with no error) if the path does not exist. It will return true
// if the path exists, even if the user doesn't have permission to enter and
// read files in the directory.
func IsDir(path string) (bool, error) {
	info, err := os.Stat(path)
	if errors.Is(err, os.ErrNotExist) {
		return false, nil
	} else if err != nil {
		return false, err
	}
	return info.IsDir(), nil
}

// DirIsEmpty returns whether a given directory is empty.
// By convention, we say that a directory is empty if it does not exist.
func DirIsEmpty(dir string) (bool, error) {
	entries, err := os.ReadDir(dir)
	if errors.Is(err, os.ErrNotExist) {
		return true, nil
	} else if err != nil {
		return false, err
	}
	return len(entries) == 0, nil
}

// CopyDir copies the src directory into the target directory, preserving file
// and directory modes. If skipUnknown is true, it returns the list of skipped files.
func CopyDir(srcDir, dstDir string, unknownFilesMode UnknownFilesMode) ([]string, error) {
	var skippedFiles []string
	// Requires srcDir to be an absolute path given that the code below (filepath.Rel and
	// filepath.EvalSymlinks) are not going to work as expected with relative paths.
	if !filepath.IsAbs(srcDir) {
		return skippedFiles, fmt.Errorf("CopyDir wants %s argument to be absolute, got relative path.", srcDir)
	}
	err := filepath.Walk(srcDir, func(srcPath string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}

		relPath, err := filepath.Rel(srcDir, srcPath)
		if err != nil {
			return err
		}

		dstPath := filepath.Join(dstDir, relPath)

		switch info.Mode() & os.ModeType {
		case 0: // default file
			if err := CopyFile(srcPath, dstPath); err != nil {
				return err
			}

		case os.ModeDir:
			if err := os.Mkdir(dstPath, info.Mode()); err != nil && !os.IsExist(err) {
				return err
			}

		case os.ModeSymlink:
			srcLink, err := filepath.EvalSymlinks(srcPath)
			if errors.Is(err, os.ErrNotExist) {
				switch unknownFilesMode {
				case RaiseError:
					return fmt.Errorf("symlink %s: link %s does not exist", srcPath, srcLink)
				case SkipUnknownFiles:
					skippedFiles = append(skippedFiles, srcPath)
					return nil
				}
			} else if err != nil {
				return fmt.Errorf("filepath.EvalSymlinks %s: %w", srcPath, err)
			}

			if strings.HasPrefix(srcLink, filepath.Clean(srcDir)) {
				srcLinkRelToSrcDir, err := filepath.Rel(srcDir, srcLink)
				if err != nil {
					return err
				}
				dstLink := filepath.Join(dstDir, srcLinkRelToSrcDir)
				if _, err := os.Stat(dstLink); err != nil {
					if err := CopyFile(srcLink, dstLink); err != nil {
						return err
					}
				}
				if err := os.Symlink(dstLink, dstPath); err != nil {
					return fmt.Errorf("os.Symlink %s, %s: %w", dstLink, dstPath, err)
				}
			} else {
				if err := os.Symlink(srcLink, dstPath); err != nil {
					return fmt.Errorf("os.Symlink %s, %s: %w", srcLink, dstPath, err)
				}
			}

		default:
			switch unknownFilesMode {
			case RaiseError:
				return fmt.Errorf("unknown file type for %s", srcPath)
			case SkipUnknownFiles:
				skippedFiles = append(skippedFiles, srcPath)
			}
		}

		return nil
	})

	return skippedFiles, err
}
