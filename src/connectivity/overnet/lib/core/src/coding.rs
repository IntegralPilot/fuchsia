// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Message encode/decode helpers

use anyhow::Error;

/// Decode some bytes into a FIDL type
pub fn decode_fidl<T: fidl::Persistable>(bytes: &mut [u8]) -> Result<T, Error> {
    fidl::unpersist(bytes).map_err(Into::into)
}

/// Encode a FIDL type into some bytes
pub fn encode_fidl<'a, T: fidl::Persistable>(value: &'a mut T) -> Result<Vec<u8>, Error> {
    fidl::persist(value).map_err(Into::into)
}

#[cfg(test)]
mod test {
    use super::*;
    use fidl_test_coding::Foo;

    #[fuchsia::test]
    fn encode_decode() {
        let mut bytes = encode_fidl(&mut Foo { byte: 5 }).expect("encoding fails");
        let result: Foo = decode_fidl(&mut bytes).expect("decoding fails");
        assert_eq!(result.byte, 5);
    }
}
