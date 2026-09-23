// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package summarize

import (
	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

// alias represents an element corresponding to a FIDL alias declaration.
type alias struct {
	named
	notMember
	targetType Type
}

const aliasType Kind = "alias"

func (a *alias) Serialize() ElementStr {
	e := a.named.Serialize()
	e.Kind = aliasType
	e.Type = a.targetType
	return e
}

// addAliases adds the aliases from the FIDL IR.
func (s *summarizer) addAliases(aliases []fidlgen.Alias) {
	for _, a := range aliases {
		s.addElement(&alias{
			named:      named{name: Name(a.Name)},
			targetType: s.symbols.fidlTypeString(a.Type),
		})
	}
}
