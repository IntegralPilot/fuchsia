// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This cmd package holds utility functions that every util-based binary can use.
// Creating directories, saving files, program exit messages, etc.
package cmd

import (
	"fmt"
	"io"
	"os"
	"path/filepath"
)

func Exit(err error) {
	if err != nil {
		fmt.Fprintf(os.Stderr, "check-licenses: %s\nSee go/fuchsia-licenses-playbook for information on resolving common errors.\n", err)
		os.Exit(1)
	}
}

func MakeDirs(path string) error {
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return fmt.Errorf("failed to make directory %s: %w", dir, err)
	}
	return nil
}

func SaveFile(data []byte, path string) error {
	if err := MakeDirs(path); err != nil {
		return err
	}

	if err := os.WriteFile(path, []byte(data), 0666); err != nil {
		return fmt.Errorf("failed to write file %s: %w", path, err)
	}
	return nil
}

func CopyFile(src, dest string) error {
	fmt.Printf("CopyFile: Src: %s | dest: %s \n", src, dest)
	if err := MakeDirs(dest); err != nil {
		return err
	}

	srcFile, err := os.Open(src)
	if err != nil {
		return err
	}
	defer srcFile.Close()

	destFile, err := os.Create(dest)
	if err != nil {
		return err
	}
	defer destFile.Close()

	_, err = io.Copy(destFile, srcFile)
	if err != nil {
		return err
	}
	return nil
}
