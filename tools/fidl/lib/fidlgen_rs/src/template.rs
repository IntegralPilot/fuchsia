// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[macro_export]
macro_rules! template_variants {
    (
        impl $base:ident {
            $(fn $variant:ident($path:literal) -> $variant_struct:ident;)*
        }
    ) => {
        impl<'a> $base<'a> {
            $(
                pub fn $variant(self) -> $variant_struct<'a> {
                    $variant_struct {
                        inner_: self,
                    }
                }
            )*
        }

        $(
            #[derive(::askama::Template)]
            #[template(path = $path)]
            pub struct $variant_struct<'a> {
                inner_: $base<'a>,
            }

            impl<'a> ::core::ops::Deref for $variant_struct<'a> {
                type Target = $base<'a>;

                fn deref(&self) -> &Self::Target {
                    &self.inner_
                }
            }
        )*
    };
}
