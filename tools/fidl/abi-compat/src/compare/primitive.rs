// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt::Display;

#[derive(PartialEq, PartialOrd, Eq, Ord, Debug, Clone, Copy)]
pub enum Primitive {
    Bool,
    Int8,
    Uint8,
    Int16,
    Uint16,
    Int32,
    Uint32,
    Int64,
    Uint64,
    Float32,
    Float64,
}

impl Display for Primitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Primitive::Bool => write!(f, "bool"),
            Primitive::Int8 => write!(f, "int8"),
            Primitive::Uint8 => write!(f, "uint8"),
            Primitive::Int16 => write!(f, "int16"),
            Primitive::Uint16 => write!(f, "uint16"),
            Primitive::Int32 => write!(f, "int32"),
            Primitive::Uint32 => write!(f, "uint32"),
            Primitive::Int64 => write!(f, "int64"),
            Primitive::Uint64 => write!(f, "uint64"),
            Primitive::Float32 => write!(f, "float32"),
            Primitive::Float64 => write!(f, "float64"),
        }
    }
}
