// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package flagmisc

import (
	"fmt"
	"strings"
)

// StringsValue implements flag.Value so it may be treated as a flag type.
type StringsValue []string

// Set implements flag.Value.Set.
func (s *StringsValue) Set(val string) error {
	*s = append(*s, val)
	return nil
}

// String implements flag.Value.String.
func (s *StringsValue) String() string {
	if s == nil {
		return ""
	}
	return strings.Join([]string(*s), ", ")
}

// StringMapValue implements flag.Value so it may be treated as a flag type.
type StringMapValue map[string]string

// Set implements flag.Value.Set.
func (m *StringMapValue) Set(value string) error {
	if *m == nil {
		*m = StringMapValue{}
	}
	if value == "" {
		return nil
	}
	key, value, ok := strings.Cut(value, "=")
	if !ok {
		return fmt.Errorf("invalid map item: %s", value)
	}
	(*m)[key] = value
	return nil
}

// String implements flag.Value.String.
func (m *StringMapValue) String() string {
	return fmt.Sprintf("%v", *m)
}
