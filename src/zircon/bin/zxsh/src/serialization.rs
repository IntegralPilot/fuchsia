// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// A trait for types that can be serialized into a byte buffer.
pub trait Serialize {
    /// Serializes `self` into the provided byte buffer.
    fn serialize_into(&self, buf: &mut Vec<u8>);
}

/// A trait for types that can be deserialized from a byte buffer.
pub trait Deserialize: Sized {
    /// Deserializes an instance of `Self` from `bytes` starting at `offset`.
    /// Updates `offset` to point to the next byte after the deserialized data.
    fn deserialize(bytes: &[u8], offset: &mut usize) -> Result<Self, String>;
}

impl Serialize for u32 {
    fn serialize_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.to_le_bytes());
    }
}

impl Deserialize for u32 {
    fn deserialize(bytes: &[u8], offset: &mut usize) -> Result<Self, String> {
        let size = std::mem::size_of::<Self>();
        if *offset + size > bytes.len() {
            return Err("EOF reading u32".to_string());
        }
        let val = u32::from_le_bytes(
            bytes[*offset..*offset + size]
                .try_into()
                .map_err(|e| format!("u32 deserialize error: {}", e))?,
        );
        *offset += size;
        Ok(val)
    }
}

impl Serialize for i32 {
    fn serialize_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.to_le_bytes());
    }
}

impl Deserialize for i32 {
    fn deserialize(bytes: &[u8], offset: &mut usize) -> Result<Self, String> {
        let size = std::mem::size_of::<Self>();
        if *offset + size > bytes.len() {
            return Err("EOF reading i32".to_string());
        }
        let val = i32::from_le_bytes(
            bytes[*offset..*offset + size]
                .try_into()
                .map_err(|e| format!("i32 deserialize error: {}", e))?,
        );
        *offset += size;
        Ok(val)
    }
}

impl Serialize for u64 {
    fn serialize_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.to_le_bytes());
    }
}

impl Deserialize for u64 {
    fn deserialize(bytes: &[u8], offset: &mut usize) -> Result<Self, String> {
        let size = std::mem::size_of::<Self>();
        if *offset + size > bytes.len() {
            return Err("EOF reading u64".to_string());
        }
        let val = u64::from_le_bytes(
            bytes[*offset..*offset + size]
                .try_into()
                .map_err(|e| format!("u64 deserialize error: {}", e))?,
        );
        *offset += size;
        Ok(val)
    }
}

impl Serialize for Vec<u8> {
    fn serialize_into(&self, buf: &mut Vec<u8>) {
        (self.len() as u32).serialize_into(buf);
        buf.extend_from_slice(self);
    }
}

impl Deserialize for Vec<u8> {
    fn deserialize(bytes: &[u8], offset: &mut usize) -> Result<Self, String> {
        let len = u32::deserialize(bytes, offset)? as usize;
        if *offset + len > bytes.len() {
            return Err("EOF reading vec".to_string());
        }
        let v = bytes[*offset..*offset + len].to_vec();
        *offset += len;
        Ok(v)
    }
}

impl Serialize for bstr::BStr {
    fn serialize_into(&self, buf: &mut Vec<u8>) {
        (self.len() as u32).serialize_into(buf);
        buf.extend_from_slice(self);
    }
}

impl Serialize for bstr::BString {
    fn serialize_into(&self, buf: &mut Vec<u8>) {
        let b: &bstr::BStr = self.as_ref();
        b.serialize_into(buf);
    }
}

impl Deserialize for bstr::BString {
    fn deserialize(bytes: &[u8], offset: &mut usize) -> Result<Self, String> {
        let len = u32::deserialize(bytes, offset)? as usize;
        if *offset + len > bytes.len() {
            return Err("EOF reading string".to_string());
        }
        let s = bstr::BString::from(&bytes[*offset..*offset + len]);
        *offset += len;
        Ok(s)
    }
}

impl Serialize for Vec<bstr::BString> {
    fn serialize_into(&self, buf: &mut Vec<u8>) {
        (self.len() as u32).serialize_into(buf);
        for item in self {
            item.serialize_into(buf);
        }
    }
}

impl Deserialize for Vec<bstr::BString> {
    fn deserialize(bytes: &[u8], offset: &mut usize) -> Result<Self, String> {
        let len = u32::deserialize(bytes, offset)? as usize;
        let mut v = Vec::with_capacity(len);
        for _ in 0..len {
            v.push(bstr::BString::deserialize(bytes, offset)?);
        }
        Ok(v)
    }
}

pub trait BStrExt {
    fn split_byte(&self, sep: u8) -> impl Iterator<Item = &bstr::BStr>;
}

impl BStrExt for [u8] {
    fn split_byte(&self, sep: u8) -> impl Iterator<Item = &bstr::BStr> {
        self.split(move |&b| b == sep).map(bstr::BStr::new)
    }
}
