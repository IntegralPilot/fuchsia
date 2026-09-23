// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_tee::{self as ftee, Buffer, Direction, ParameterUnknown, Value};
use std::ops::Range;
use tee_internal::{Param, ParamType, ParamTypes, ValueFields};

struct Memref {
    direction: ftee::Direction,
    vmo: Option<zx::Vmo>,
    offset: Option<u64>,
    original_size: usize,
    mapped_address: usize,
    mapped_length: usize,
}

enum ParamInfo {
    None,
    Memref(Memref),
    Value(ftee::Direction),
}

// ParamAdapter has logic for translating parameters for a trusted application from the
// FIDL format to the in-memory format used by the TA and back.
// Translating between the FIDL and in-memory representations of buffer requires
// memory mapping operations and is stateful. This type stores this state and cleans
// it up when dropped.
pub struct ParamAdapter {
    tee_params: [Param; 4],
    infos: [ParamInfo; 4],
}

fn param_type(parameter: &ftee::Parameter) -> ParamType {
    match parameter {
        ftee::Parameter::None(_) => ParamType::None,
        ftee::Parameter::Buffer(buffer) => match buffer.direction {
            Some(Direction::Input) => ParamType::MemrefInput,
            Some(Direction::Output) => ParamType::MemrefOutput,
            Some(Direction::Inout) => ParamType::MemrefInout,
            None => ParamType::None,
        },
        ftee::Parameter::Value(value) => match value.direction {
            Some(Direction::Input) => ParamType::ValueInput,
            Some(Direction::Output) => ParamType::ValueOutput,
            Some(Direction::Inout) => ParamType::ValueInout,
            None => ParamType::None,
        },
        ftee::ParameterUnknown!() => ParamType::None,
    }
}

fn empty_tee_param() -> Param {
    // Unsafe block necessary to unmarshal from zeroed bytes. Ideally we'd use
    // zerocopy::FromZeros, but the derivation of that stumbles on the
    // `*mut ::std::os::raw::c_void` field in `TEE_Param__bindgen_ty_1`.
    unsafe { std::mem::zeroed() }
}

fn import_value_from_fidl(value: Value) -> Result<(Param, ParamInfo), Error> {
    match value.direction {
        Some(direction) => {
            let tee_param = match direction {
                Direction::Input | Direction::Inout => Param {
                    value: ValueFields {
                        a: value.a.unwrap_or_default() as u32,
                        b: value.b.unwrap_or_default() as u32,
                    },
                },
                Direction::Output => empty_tee_param(),
            };
            Ok((tee_param, ParamInfo::Value(direction)))
        }
        None => anyhow::bail!("Invalid direction"),
    }
}

fn import_buffer_from_fidl(
    buffer: Buffer,
    mapped_param_ranges: &mut Vec<Range<usize>>,
) -> Result<(Param, ParamInfo), Error> {
    if let Some(direction) = buffer.direction {
        let mapping_offset = match buffer.offset {
            None => anyhow::bail!("Missing offset"),
            Some(offset) => offset,
        };
        let (vmo, mapped_address, data_size, mapped_length) = if let Some(vmo) = buffer.vmo {
            let flags = match direction {
                Direction::Input => zx::VmarFlags::PERM_READ,
                Direction::Inout | Direction::Output => {
                    zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE
                }
            };

            // A zero-length buffer at a valid address is okay, but that
            // should be presented to the TA as valid memory. This manifests
            // here as a VMO being provided with a size of zero: in that case
            // we accordingly map in a zero-filled page - not trusting that the
            // provided VMO has non-zero content - and specify a buffer of size
            // 0 rooted at that address.
            let data_size: usize = buffer.size.unwrap_or(0).try_into().unwrap();
            let page_size: usize = zx::system_get_page_size().try_into().unwrap();
            let (vmo, mapped_length) = if data_size == 0 {
                (zx::Vmo::create(page_size as u64)?, page_size)
            } else {
                (vmo, data_size)
            };

            let addr = fuchsia_runtime::vmar_root_self()
                .map(0, &vmo, mapping_offset, mapped_length, flags)
                .map_err(|e| anyhow::anyhow!("Unable to map VMO: {e:?}"))?;

            mapped_param_ranges.push(addr..addr + mapped_length);

            (Some(vmo), addr, data_size, mapped_length)
        } else {
            // The VMO can be omitted meaning that no memory is
            // mapped. This is useful for asking the TA to compute
            // the size of a buffer needed for an operation. In this
            // case, the null address is provided as the buffer
            // address.
            (None, 0, 0, 0)
        };
        return Ok((
            Param {
                memref: tee_internal::MemRef { buffer: mapped_address as *mut u8, size: data_size },
            },
            ParamInfo::Memref(Memref {
                direction,
                vmo,
                offset: Some(mapping_offset),
                original_size: data_size,
                mapped_address,
                mapped_length,
            }),
        ));
    } else {
        anyhow::bail!("Invalid direction")
    }
}

impl ParamAdapter {
    pub fn from_fidl(
        parameters: Vec<ftee::Parameter>,
        mapped_param_ranges: &mut Vec<Range<usize>>,
    ) -> Result<(Self, ParamTypes), Error> {
        if parameters.len() > 4 {
            anyhow::bail!("Expected <= 4 parameters in set but got {}", parameters.len());
        }

        // 4.3.6.2 Initial Content of params Argument specifies that unset values must be filled with zeros.
        // These entries do not affect the param_types value.
        let mut tee_params = [empty_tee_param(); 4];
        let mut param_types = [ParamType::None, ParamType::None, ParamType::None, ParamType::None];
        let mut infos = [ParamInfo::None, ParamInfo::None, ParamInfo::None, ParamInfo::None];
        let mut i = 0;
        for param in parameters {
            param_types[i] = param_type(&param);
            use fidl_fuchsia_tee::Parameter;
            let (param, info) = match param {
                Parameter::None(_) => (empty_tee_param(), ParamInfo::None),
                Parameter::Buffer(buffer) => import_buffer_from_fidl(buffer, mapped_param_ranges)?,
                Parameter::Value(value) => import_value_from_fidl(value)?,
                ParameterUnknown!() => anyhow::bail!("Unexpected parameter type"),
            };
            tee_params[i] = param;
            infos[i] = info;
            i += 1;
        }
        Ok((Self { tee_params, infos }, ParamTypes::from_types(param_types)))
    }

    pub fn tee_params_mut<'a>(&'a mut self) -> &'a mut [Param; 4] {
        &mut self.tee_params
    }

    // 4.3.6.3 Behavior of the Framework when the Trusted Application Returns
    pub fn export_to_fidl(mut self) -> Result<Vec<ftee::Parameter>, Error> {
        let mut ret = vec![];
        for i in 0..4 {
            ret.push(match &mut self.infos[i] {
                ParamInfo::None => ftee::Parameter::None(ftee::None_),
                ParamInfo::Value(direction) => match direction {
                    Direction::Input => ftee::Parameter::None(ftee::None_),
                    Direction::Inout | Direction::Output => {
                        let tee_value = unsafe { self.tee_params[i].value };
                        let a = Some(tee_value.a as u64);
                        let b = Some(tee_value.b as u64);
                        ftee::Parameter::Value(Value {
                            direction: Some(*direction),
                            a,
                            b,
                            c: None,
                            ..Default::default()
                        })
                    }
                },
                ParamInfo::Memref(memref) => {
                    unsafe {
                        if memref.mapped_address != 0 {
                            // If we mapped the buffer into our address space we need to unmap it.
                            fuchsia_runtime::vmar_root_self()
                                .unmap(memref.mapped_address, memref.mapped_length)
                                .unwrap();
                        }
                    }
                    match memref.direction {
                        Direction::Input => ftee::Parameter::None(ftee::None_),
                        Direction::Inout | Direction::Output => {
                            // Table 4-9: Interpretation of params[i] when Trusted Application Entry Point Returns
                            // The Framework reads params[i].memref.size:
                            let tee_memref = unsafe { self.tee_params[i].memref };
                            let size = tee_memref.size;
                            if size > memref.original_size {
                                // * If it is greater than the original value of size, it is considered
                                // as a request for a larger buffer. In this case, the Framework
                                // assumes that the Trusted Application has not written
                                // anything in the buffer and no data will be synchronized.
                                ftee::Parameter::Buffer(Buffer {
                                    direction: Some(memref.direction),
                                    vmo: None,
                                    offset: memref.offset,
                                    size: Some(size as u64),
                                    ..Default::default()
                                })
                            } else {
                                // * If it is equal or less than the original value of size, it is
                                // considered as the actual size of the memory buffer. In this
                                // case, the Framework assumes that the Trusted Application
                                // has not written beyond this actual size and only this actual
                                // size will be synchronized with the client.
                                ftee::Parameter::Buffer(Buffer {
                                    direction: Some(memref.direction),
                                    vmo: memref.vmo.take(),
                                    offset: memref.offset,
                                    size: Some(size as u64),
                                    ..Default::default()
                                })
                            }
                        }
                    }
                }
            })
        }
        // Remove trailing 'None' entries.
        while !ret.is_empty() {
            match ret.last() {
                Some(ftee::Parameter::None(_)) => {
                    let _ = ret.pop().unwrap();
                }
                _ => break,
            }
        }
        Ok(ret)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[fuchsia::test]
    fn empty_param_is_zeroed() -> Result<(), Error> {
        let empty = empty_tee_param();
        // Accessing the fields of the parameter's union is unsafe as it is a
        // repr(C) union and carries no type information.
        let memref = unsafe { empty.memref };
        assert_eq!(memref, tee_internal::MemRef { buffer: std::ptr::null_mut(), size: 0 });
        let value = unsafe { empty.value };
        assert_eq!(value, ValueFields { a: 0, b: 0 });
        Ok(())
    }

    #[fuchsia::test]
    fn empty_fidl_parameters_to_tee_params() -> Result<(), Error> {
        let fidl_parameters = vec![];

        let (mut adapter, param_types) = ParamAdapter::from_fidl(fidl_parameters, &mut vec![])?;

        assert_eq!(param_types.as_u32(), 0);

        let tee_params = adapter.tee_params_mut();
        for i in 0..4 {
            // Accessing the 'value' field of the parameter's union is unsafe as it
            // is a repr(C) union and carries no type information.
            let value = unsafe { tee_params[i].value };
            assert_eq!(value, ValueFields { a: 0, b: 0 });
        }
        Ok(())
    }

    #[fuchsia::test]
    fn too_many_fidl_parameters() -> Result<(), Error> {
        let fidl_parameters = vec![
            ftee::Parameter::Value(Value { ..Default::default() }),
            ftee::Parameter::Value(Value { ..Default::default() }),
            ftee::Parameter::Value(Value { ..Default::default() }),
            ftee::Parameter::Value(Value { ..Default::default() }),
            ftee::Parameter::Value(Value { ..Default::default() }),
        ];

        let result = ParamAdapter::from_fidl(fidl_parameters, &mut vec![]);

        assert!(result.is_err());
        Ok(())
    }

    #[fuchsia::test]
    fn fidl_parameter_value_to_tee_params() -> Result<(), Error> {
        let fidl_parameters = vec![ftee::Parameter::Value(Value {
            direction: Some(Direction::Input),
            a: Some(1),
            b: Some(2),
            ..Default::default()
        })];

        let (mut adapter, param_types) = ParamAdapter::from_fidl(fidl_parameters, &mut vec![])?;

        assert_eq!(param_types.get(0), ParamType::ValueInput);
        assert_eq!(param_types.get(1), ParamType::None);
        assert_eq!(param_types.get(2), ParamType::None);
        assert_eq!(param_types.get(3), ParamType::None);
        let tee_params = adapter.tee_params_mut();
        // Accessing the 'value' field of the parameter's union is unsafe as it
        // is a repr(C) union and carries no type information.
        let value = unsafe { tee_params[0].value };
        assert_eq!(value, ValueFields { a: 1, b: 2 });
        for i in 1..4 {
            let value = unsafe { tee_params[i].value };
            assert_eq!(value, ValueFields { a: 0, b: 0 });
        }
        Ok(())
    }

    #[fuchsia::test]
    fn fidl_parameter_multiple_value_to_tee_params() -> Result<(), Error> {
        let fidl_parameters = vec![
            ftee::Parameter::Value(Value {
                direction: Some(Direction::Input),
                a: Some(1),
                b: Some(2),
                ..Default::default()
            }),
            ftee::Parameter::Value(Value {
                direction: Some(Direction::Inout),
                a: Some(3),
                b: Some(4),
                ..Default::default()
            }),
        ];

        let (mut adapter, param_types) = ParamAdapter::from_fidl(fidl_parameters, &mut vec![])?;
        assert_eq!(param_types.get(0), ParamType::ValueInput);
        assert_eq!(param_types.get(1), ParamType::ValueInout);
        assert_eq!(param_types.get(2), ParamType::None);
        assert_eq!(param_types.get(3), ParamType::None);
        let tee_params = adapter.tee_params_mut();
        // Accessing the 'value' field of the parameter's union is unsafe as it
        // is a repr(C) union and carries no type information.
        let value = unsafe { tee_params[0].value };
        assert_eq!(value, ValueFields { a: 1, b: 2 });
        let value = unsafe { tee_params[1].value };
        assert_eq!(value, ValueFields { a: 3, b: 4 });
        for i in 2..4 {
            let value = unsafe { tee_params[i].value };
            assert_eq!(value, ValueFields { a: 0, b: 0 });
        }
        Ok(())
    }

    #[fuchsia::test]
    fn fidl_parameter_with_buffers() -> Result<(), Error> {
        let page_size = zx::system_get_page_size() as u64;
        let vmo1 = zx::Vmo::create(page_size).unwrap();
        vmo1.write(&[1, 2, 3, 4], 0).unwrap();
        let vmo2 = zx::Vmo::create(3 * page_size).unwrap();
        vmo2.write(&[5, 6, 7, 8], page_size).unwrap();
        let vmo3 = zx::Vmo::create(page_size).unwrap();
        vmo3.write(&[9, 10, 11, 12], 0).unwrap();

        let fidl_parameters = vec![
            ftee::Parameter::Buffer(Buffer {
                direction: Some(Direction::Inout),
                vmo: Some(vmo1),
                offset: Some(0),
                size: Some(page_size),
                ..Default::default()
            }),
            ftee::Parameter::Buffer(Buffer {
                direction: Some(Direction::Input),
                vmo: Some(vmo2),
                offset: Some(page_size),
                size: Some(2 * page_size),
                ..Default::default()
            }),
            ftee::Parameter::Buffer(Buffer {
                direction: Some(Direction::Inout),
                vmo: None,
                offset: Some(0),
                size: Some(0),
                ..Default::default()
            }),
            ftee::Parameter::Buffer(Buffer {
                direction: Some(Direction::Output),
                vmo: Some(vmo3),
                offset: Some(0),
                size: Some(page_size),
                ..Default::default()
            }),
        ];

        let (mut adapter, param_types) = ParamAdapter::from_fidl(fidl_parameters, &mut vec![])?;
        assert_eq!(param_types.get(0), ParamType::MemrefInout);
        assert_eq!(param_types.get(1), ParamType::MemrefInput);
        assert_eq!(param_types.get(2), ParamType::MemrefInout);
        assert_eq!(param_types.get(3), ParamType::MemrefOutput);

        let tee_params = adapter.tee_params_mut();
        // Accessing the 'value' field of the parameter's union is unsafe as it
        // is a repr(C) union and carries no type information.
        let value = unsafe { tee_params[0].memref };
        assert_eq!(value.size, page_size as usize);
        assert!(value.buffer != std::ptr::null_mut());
        let memref_contents = unsafe { std::slice::from_raw_parts(value.buffer, 4) };
        assert_eq!(memref_contents, &[1, 2, 3, 4]);

        let value = unsafe { tee_params[1].memref };
        assert_eq!(value.size, 2 * page_size as usize);
        assert!(value.buffer != std::ptr::null_mut());
        let memref_contents = unsafe { std::slice::from_raw_parts(value.buffer, 4) };
        assert_eq!(memref_contents, &[5, 6, 7, 8]);

        let value = unsafe { tee_params[2].memref };
        assert_eq!(value.size, 0);
        assert_eq!(value.buffer, std::ptr::null_mut());

        // An output parameter should be mapped at this point with its
        // original contents.
        let value = unsafe { tee_params[3].memref };
        assert_eq!(value.size, page_size as usize);
        assert!(value.buffer != std::ptr::null_mut());
        let memref_contents = unsafe { std::slice::from_raw_parts(value.buffer, 4) };
        assert_eq!(memref_contents, &[9, 10, 11, 12]);

        Ok(())
    }

    // Empty VMO parameters should result in empty buffers within a mapped
    // page.
    #[fuchsia::test]
    fn fidl_parameter_with_empty_buffers() -> Result<(), Error> {
        let empty1 = zx::Vmo::create(0).unwrap();
        let empty2 = zx::Vmo::create(0).unwrap();
        let empty3 = zx::Vmo::create(0).unwrap();

        let fidl_parameters = vec![
            ftee::Parameter::Buffer(Buffer {
                direction: Some(Direction::Input),
                vmo: Some(empty1),
                offset: Some(0),
                size: Some(0),
                ..Default::default()
            }),
            ftee::Parameter::Buffer(Buffer {
                direction: Some(Direction::Inout),
                vmo: Some(empty2),
                offset: Some(0),
                size: Some(0),
                ..Default::default()
            }),
            ftee::Parameter::Buffer(Buffer {
                direction: Some(Direction::Output),
                vmo: Some(empty3),
                offset: Some(0),
                size: Some(0),
                ..Default::default()
            }),
        ];

        let (mut adapter, param_types) = ParamAdapter::from_fidl(fidl_parameters, &mut vec![])?;
        assert_eq!(param_types.get(0), ParamType::MemrefInput);
        assert_eq!(param_types.get(1), ParamType::MemrefInout);
        assert_eq!(param_types.get(2), ParamType::MemrefOutput);
        assert_eq!(param_types.get(3), ParamType::None);

        let tee_params = adapter.tee_params_mut();
        // Accessing the 'value' field of the parameter's union is unsafe as it
        // is a repr(C) union and carries no type information.
        let value = unsafe { tee_params[0].memref };
        assert_eq!(value.size, 0);
        assert!(value.buffer != std::ptr::null_mut());

        let value = unsafe { tee_params[1].memref };
        assert_eq!(value.size, 0);
        assert!(value.buffer != std::ptr::null_mut());

        let value = unsafe { tee_params[2].memref };
        assert_eq!(value.size, 0);
        assert!(value.buffer != std::ptr::null_mut());

        Ok(())
    }

    #[fuchsia::test]
    fn test_export_empty_to_fidl() -> Result<(), Error> {
        let (adapter, param_types) = ParamAdapter::from_fidl(vec![], &mut vec![])?;
        assert_eq!(param_types.get(0), ParamType::None);
        assert_eq!(param_types.get(1), ParamType::None);
        assert_eq!(param_types.get(2), ParamType::None);
        assert_eq!(param_types.get(3), ParamType::None);

        let fidl_parameters = adapter.export_to_fidl()?;

        assert!(fidl_parameters.is_empty());
        Ok(())
    }

    #[fuchsia::test]
    fn test_unchanged_value_directions() -> Result<(), Error> {
        let fidl_parameters = vec![
            ftee::Parameter::Value(Value {
                direction: Some(Direction::Input),
                a: Some(1),
                b: Some(2),
                c: None,
                ..Default::default()
            }),
            ftee::Parameter::Value(Value {
                direction: Some(Direction::Inout),
                a: Some(3),
                b: Some(4),
                c: None,
                ..Default::default()
            }),
            ftee::Parameter::Value(Value {
                direction: Some(Direction::Output),
                a: None,
                b: None,
                c: None,
                ..Default::default()
            }),
        ];
        let (adapter, param_types) = ParamAdapter::from_fidl(fidl_parameters, &mut vec![])?;
        assert_eq!(param_types.get(0), ParamType::ValueInput);
        assert_eq!(param_types.get(1), ParamType::ValueInout);
        assert_eq!(param_types.get(2), ParamType::ValueOutput);
        assert_eq!(param_types.get(3), ParamType::None);

        let fidl_parameters = adapter.export_to_fidl()?;
        assert_eq!(fidl_parameters.len(), 3);
        assert_eq!(fidl_parameters[0], ftee::Parameter::None(ftee::None_));
        assert_eq!(
            fidl_parameters[1],
            ftee::Parameter::Value(Value {
                direction: Some(Direction::Inout),
                a: Some(3),
                b: Some(4),
                c: None,
                ..Default::default()
            })
        );
        assert_eq!(
            fidl_parameters[2],
            ftee::Parameter::Value(Value {
                direction: Some(Direction::Output),
                a: Some(0),
                b: Some(0),
                c: None,
                ..Default::default()
            })
        );

        Ok(())
    }

    #[fuchsia::test]
    fn test_export_changed_value_to_fidl() -> Result<(), Error> {
        let fidl_parameters = vec![
            ftee::Parameter::Value(Value {
                direction: Some(Direction::Inout),
                a: Some(1),
                b: Some(2),
                c: None,
                ..Default::default()
            }),
            ftee::Parameter::Value(Value {
                direction: Some(Direction::Output),
                a: None,
                b: None,
                c: None,
                ..Default::default()
            }),
        ];
        let (mut adapter, param_types) = ParamAdapter::from_fidl(fidl_parameters, &mut vec![])?;
        assert_eq!(param_types.get(0), ParamType::ValueInout);
        assert_eq!(param_types.get(1), ParamType::ValueOutput);
        assert_eq!(param_types.get(2), ParamType::None);
        assert_eq!(param_types.get(3), ParamType::None);

        let tee_params = adapter.tee_params_mut();
        let value = unsafe { &mut tee_params[0].value };
        value.a = 3;
        value.b = 4;
        let value = unsafe { &mut tee_params[1].value };
        value.a = 5;
        value.b = 6;

        let fidl_parameters = adapter.export_to_fidl()?;

        assert_eq!(fidl_parameters.len(), 2);
        assert_eq!(
            fidl_parameters[0],
            ftee::Parameter::Value(Value {
                direction: Some(Direction::Inout),
                a: Some(3),
                b: Some(4),
                c: None,
                ..Default::default()
            })
        );
        assert_eq!(
            fidl_parameters[1],
            ftee::Parameter::Value(Value {
                direction: Some(Direction::Output),
                a: Some(5),
                b: Some(6),
                c: None,
                ..Default::default()
            })
        );

        Ok(())
    }

    #[fuchsia::test]
    fn test_export_changed_memref_to_fidl() -> Result<(), Error> {
        // This test checks that we can provide a buffer with direction "INOUT" and modify the buffer's
        // contents and size then translate that back to a FIDL response.
        let page_size = zx::system_get_page_size() as u64;
        let vmo = zx::Vmo::create(page_size).unwrap();
        vmo.write(&[1, 2, 3], 0).unwrap();
        let fidl_parameters = vec![ftee::Parameter::Buffer(Buffer {
            direction: Some(Direction::Inout),
            vmo: Some(vmo),
            offset: Some(0),
            size: Some(page_size),
            ..Default::default()
        })];

        let (mut adapter, param_types) = ParamAdapter::from_fidl(fidl_parameters, &mut vec![])?;
        assert_eq!(param_types.get(0), ParamType::MemrefInout);
        assert_eq!(param_types.get(1), ParamType::None);
        assert_eq!(param_types.get(2), ParamType::None);
        assert_eq!(param_types.get(3), ParamType::None);

        {
            // This logic makes two modifications to the TEE_Param[4] representation of the buffer:
            // 1. Writes the byte sequence [3, 4, 5] to the start of the buffer.
            // 2. Sets the memref's "size" to 3 down from its initial value of page_size.
            let tee_params = adapter.tee_params_mut();
            let memref = unsafe { &mut tee_params[0].memref };
            assert_eq!(memref.size, page_size as usize);
            assert_ne!(memref.buffer, std::ptr::null_mut());

            {
                let mapped_buffer = unsafe { std::slice::from_raw_parts_mut(memref.buffer, 3) };
                mapped_buffer.copy_from_slice(&[3, 4, 5]);
            }
            memref.size = 3;
        }

        let fidl_parameters = adapter.export_to_fidl()?;

        assert_eq!(fidl_parameters.len(), 1);

        if let ftee::Parameter::Buffer(buffer) = &fidl_parameters[0] {
            // We expect to get back a buffer that has the byte sequence [3, 4, 5] at the start and
            // a reported size of 3.
            let mut read = [0u8; 3];
            let _ = buffer.vmo.as_ref().unwrap().read(&mut read, 0).unwrap();
            assert_eq!(read, [3, 4, 5]);
            assert_eq!(buffer.direction, Some(Direction::Inout));
            assert_eq!(buffer.offset, Some(0));
            assert_eq!(buffer.size, Some(3));
        } else {
            assert!(
                false,
                "Expected a Buffer parameter but observed another type: {:?}",
                &fidl_parameters[0]
            );
        }

        Ok(())
    }

    #[fuchsia::test]
    fn test_memref_size_increased() -> Result<(), Error> {
        // This tests the behavior when a buffer's size is changed in the TEE_Param[4]
        // representation to a larger value than was initially specified. According to the
        // specification this pattern is used to let a TA indicate that an operation requires a
        // larger buffer than was initially provided.  In this case no data is returned to the
        // client but the larger size is returned with the expectation that the client will allocate
        // a larger buffer and try again.
        let page_size = zx::system_get_page_size() as u64;
        let vmo = zx::Vmo::create(page_size).unwrap();
        let fidl_parameters = vec![
            ftee::Parameter::Buffer(Buffer {
                direction: Some(Direction::Inout),
                vmo: None,
                offset: Some(0),
                size: Some(0),
                ..Default::default()
            }),
            ftee::Parameter::Buffer(Buffer {
                direction: Some(Direction::Inout),
                vmo: Some(vmo),
                offset: Some(0),
                size: Some(page_size),
                ..Default::default()
            }),
        ];

        let (mut adapter, param_types) = ParamAdapter::from_fidl(fidl_parameters, &mut vec![])?;
        assert_eq!(param_types.get(0), ParamType::MemrefInout);
        assert_eq!(param_types.get(1), ParamType::MemrefInout);
        assert_eq!(param_types.get(2), ParamType::None);
        assert_eq!(param_types.get(3), ParamType::None);

        {
            let tee_params = adapter.tee_params_mut();
            let memref = unsafe { &mut tee_params[0].memref };
            // The first parameter is grown from 0 bytes to 12 bytes.
            memref.size = 12;
            let memref = unsafe { &mut tee_params[1].memref };
            // The second parameter is grown from 1 page to 2 pages.
            memref.size = 2 * page_size as usize;
        }

        let fidl_parameters = adapter.export_to_fidl()?;

        assert_eq!(
            fidl_parameters,
            vec![
                ftee::Parameter::Buffer(Buffer {
                    direction: Some(Direction::Inout),
                    vmo: None,
                    offset: Some(0),
                    size: Some(12),
                    ..Default::default()
                }),
                ftee::Parameter::Buffer(Buffer {
                    direction: Some(Direction::Inout),
                    vmo: None,
                    offset: Some(0),
                    size: Some(2 * page_size),
                    ..Default::default()
                }),
            ]
        );

        Ok(())
    }
}
