// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package util

import (
	"bytes"
	"fmt"
	"strings"
	"testing"

	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/build"
)

const (
	merkleA = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
	merkleB = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
)

func pkgURL(name, hash string) string {
	return fmt.Sprintf("fuchsia-pkg://host/%s?hash=%s", name, hash)
}

type pkgMerkle struct {
	name string
	hash build.MerkleRoot
}

func TestParsePackagesJSON(t *testing.T) {
	tests := []struct {
		id        string
		json      []byte
		pkgs      []pkgMerkle
		expectErr bool
	}{
		{
			id:        "invalid json fails",
			json:      []byte("abcd"),
			expectErr: true,
		},
		{
			id:        "empty object fails",
			json:      []byte("{}"),
			expectErr: true,
		},
		{
			id:        "invalid version fails",
			json:      []byte(`{"version":"42","content":[]}`),
			expectErr: true,
		},
		{
			id:        "numeric 1 succeeds",
			json:      []byte(`{"version":1,"content":[]}`),
			expectErr: false,
		},
		{
			id:        "numeric 2 fails",
			json:      []byte(`{"version":2,"content":[]}`),
			expectErr: true,
		},
		{
			id:        "contentless version succeeds",
			json:      []byte(`{"version":"1"}`),
			expectErr: false,
		},
		{
			id:        "variant-less package succeeds",
			json:      []byte(fmt.Sprintf(`{"version":"1","content":["%s"]}`, pkgURL("pkg", merkleA))),
			pkgs:      []pkgMerkle{{"pkg", build.MustDecodeMerkleRoot(merkleA)}},
			expectErr: false,
		},
		{
			id:        "variant package succeeds",
			json:      []byte(fmt.Sprintf(`{"version":"1","content":["%s"]}`, pkgURL("pkg/0", merkleA))),
			pkgs:      []pkgMerkle{{"pkg/0", build.MustDecodeMerkleRoot(merkleA)}},
			expectErr: false,
		},
		{
			id: "multiple packages succeed",
			json: []byte(fmt.Sprintf(
				`{"version":"1","content":["%s","%s"]}`,
				pkgURL("pkg/0", merkleA),
				pkgURL("another/0", merkleB)),
			),
			pkgs: []pkgMerkle{
				{"pkg/0", build.MustDecodeMerkleRoot(merkleA)},
				{"another/0", build.MustDecodeMerkleRoot(merkleB)},
			},
			expectErr: false,
		},
	}

	for _, test := range tests {
		r := bytes.NewBuffer(test.json)
		pkgs, err := ParsePackagesJSON(r)

		gotErr := (err != nil)
		if test.expectErr && !gotErr {
			t.Errorf("test %s: want error; got none", test.id)
		} else if !test.expectErr && gotErr {
			t.Errorf("test %s: want no error; got %v", test.id, err)
		}

		if len(pkgs) != len(test.pkgs) {
			t.Errorf("test %s: got %d packages; want %d", test.id, len(pkgs), len(test.pkgs))
		}

		if test.pkgs != nil {
			for _, pv := range test.pkgs {
				h, ok := pkgs[pv.name]
				if !ok {
					t.Errorf("test %s: key not found; want %s", test.id, pv.name)
				}
				if h != pv.hash {
					t.Errorf("test %s for package %s: got hash %s; want %s", test.id, pv.name, h, pv.hash)
				}
			}
		}
	}
}

func TestRehostPackagesJSON(t *testing.T) {
	tests := []struct {
		id       string
		json     []byte
		expected string
	}{
		{
			id:       "regular update package",
			json:     []byte(fmt.Sprintf(`{"version":"1","content":["%s","%s"]}`, pkgURL("pkg/0", "abc"), pkgURL("another/0", "def"))),
			expected: `{"version":"1","content":["fuchsia-pkg://new/pkg/0?hash=abc","fuchsia-pkg://new/another/0?hash=def"]}`,
		},
		{
			id:       "numeric version 1 converted to string",
			json:     []byte(`{"version":1,"content":[]}`),
			expected: `{"version":"1","content":[]}`,
		},
	}

	for _, test := range tests {
		r := bytes.NewBuffer(test.json)
		var b bytes.Buffer
		err := RehostPackagesJSON(r, &b, "new")
		if err != nil {
			t.Errorf("test %s: RehostPackagesJSON failed: %v", test.id, err)
		}
		actual := strings.TrimSpace(b.String())
		if actual != test.expected {
			t.Errorf("test %s: packages.json does not match, expected '%s', got '%s'", test.id, test.expected, actual)
		}
	}
}
