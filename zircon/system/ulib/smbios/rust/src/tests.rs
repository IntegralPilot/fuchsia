extern crate std;
use super::*;
use std::string::String;
use std::vec::Vec;
use zerocopy::byteorder::little_endian::{U16, U32, U64};
use zerocopy::{FromBytes, IntoBytes};

const BIOS_STRING1: &str = "string1";
const BIOS_STRING2: &str = "string2";
const NUM_STRUCTURES: u16 = 2;

fn create_fake_entry_point(structs: &[u8], structures_count: u16) -> EntryPoint2_1 {
    let mut ep = EntryPoint2_1 {
        anchor_string: *b"_SM_",
        checksum: 0,
        length: core::mem::size_of::<EntryPoint2_1>() as u8,
        major_ver: 2,
        minor_ver: 1,
        max_struct_size: U16::new(256),
        ep_rev: 0,
        formatted_area: [0u8; 5],
        intermediate_anchor_string: *b"_DMI_",
        intermediate_checksum: 0,
        struct_table_length: U16::new(structs.len() as u16),
        struct_table_phys: U32::new(0x1000), // Fake physical address
        struct_count: U16::new(structures_count),
        bcd_rev: 0x21,
    };

    // The specification defines the offsets for intermediate checksum (offset 0x10, len 0xf)
    let ep_bytes = ep.as_bytes();
    let intermediate_sum = compute_checksum(&ep_bytes[0x10..0x1f]);
    ep.intermediate_checksum = (256u32.wrapping_sub(intermediate_sum as u32)) as u8;

    let ep_bytes_updated = ep.as_bytes();
    let full_sum = compute_checksum(ep_bytes_updated);
    ep.checksum = (256u32.wrapping_sub(full_sum as u32)) as u8;

    ep
}

fn create_fake_v3_entry_point() -> EntryPoint3_0 {
    let mut ep = EntryPoint3_0 {
        anchor_string: *b"_SM3_",
        checksum: 0,
        length: core::mem::size_of::<EntryPoint3_0>() as u8,
        major_ver: 3,
        minor_ver: 0,
        docrev_ver: 0,
        ep_rev: 1,
        reserved: 0,
        max_struct_size: U32::new(256),
        struct_table_phys: U64::new(0x1000), // Fake physical address
    };

    let sum = compute_checksum(ep.as_bytes());
    ep.checksum = (256u32.wrapping_sub(sum as u32)) as u8;
    ep
}

fn create_fake_smbios_common() -> Vec<u8> {
    let mut structs = Vec::new();

    // 1. BiosInformationStruct2_0
    let bios_info = BiosInformationStruct2_0 {
        hdr: Header {
            r#type: StructType::BIOS_INFO,
            length: core::mem::size_of::<BiosInformationStruct2_0>() as u8,
            handle: U16::new(0),
        },
        vendor_str_idx: 1,
        bios_version_str_idx: 2,
        bios_starting_address_segment: U16::new(0xe000),
        bios_release_date_str_idx: 0,
        bios_rom_size: 0x10,
        bios_characteristics: U64::new(0x12345678),
    };
    structs.extend_from_slice(bios_info.as_bytes());
    // String table: "string1\0string2\0\0"
    structs.extend_from_slice(BIOS_STRING1.as_bytes());
    structs.push(0);
    structs.extend_from_slice(BIOS_STRING2.as_bytes());
    structs.push(0);
    structs.push(0); // terminating null

    // 2. SystemInformationStruct2_1
    let sys_info = SystemInformationStruct2_1 {
        hdr: Header {
            r#type: StructType::SYSTEM_INFO,
            length: core::mem::size_of::<SystemInformationStruct2_1>() as u8,
            handle: U16::new(1),
        },
        manufacturer_str_idx: 0,
        product_name_str_idx: 0,
        version_str_idx: 0,
        serial_number_str_idx: 0,
        uuid: [1u8; 16],
        wakeup_type: 0x06,
    };
    structs.extend_from_slice(sys_info.as_bytes());
    // String table: "\0\0"
    structs.push(0);
    structs.push(0);

    // 3. End of table
    let end = Header {
        r#type: StructType::END_OF_TABLE,
        length: core::mem::size_of::<Header>() as u8,
        handle: U16::new(0),
    };
    structs.extend_from_slice(end.as_bytes());
    structs.push(0);
    structs.push(0);

    structs
}

fn create_fake_smbios() -> (EntryPoint2_1, Vec<u8>) {
    let structs = create_fake_smbios_common();
    let ep = create_fake_entry_point(&structs, NUM_STRUCTURES);
    assert!(ep.is_valid());
    (ep, structs)
}

fn create_fake_smbios_v3() -> (EntryPoint3_0, Vec<u8>) {
    let structs = create_fake_smbios_common();
    let ep = create_fake_v3_entry_point();
    assert!(ep.is_valid());
    (ep, structs)
}

#[test]
fn test_walk_structs() {
    let (ep, structs) = create_fake_smbios();
    let mut tables_seen = [false; 2];

    let entry = EntryPoint::from(&ep);
    let result = entry.walk_structs(&structs, |version, hdr, _st| {
        assert_eq!(version.major_ver, ep.major_ver);
        assert_eq!(version.minor_ver, ep.minor_ver);
        match hdr.r#type {
            StructType::BIOS_INFO | StructType::SYSTEM_INFO => {
                let idx = hdr.r#type.0 as usize;
                assert!(!tables_seen[idx], "Table type {} seen multiple times", idx);
                tables_seen[idx] = true;
            }
            _ => panic!("Saw unexpected header type: {:?}", hdr.r#type),
        }
        Ok(())
    });

    assert_eq!(result, Ok(()));
    assert!(tables_seen[0]);
    assert!(tables_seen[1]);
}

#[test]
fn test_walk_structs_v3() {
    let (ep, structs) = create_fake_smbios_v3();
    let mut tables_seen = [false; 2];

    let entry = EntryPoint::from(&ep);
    let result = entry.walk_structs(&structs, |version, hdr, _st| {
        assert_eq!(version.major_ver, ep.major_ver);
        assert_eq!(version.minor_ver, ep.minor_ver);
        match hdr.r#type {
            StructType::BIOS_INFO | StructType::SYSTEM_INFO => {
                let idx = hdr.r#type.0 as usize;
                assert!(!tables_seen[idx], "Table type {} seen multiple times", idx);
                tables_seen[idx] = true;
            }
            _ => panic!("Saw unexpected header type: {:?}", hdr.r#type),
        }
        Ok(())
    });

    assert_eq!(result, Ok(()));
    assert!(tables_seen[0]);
    assert!(tables_seen[1]);
}

#[test]
fn test_walk_structs_early_stop() {
    let (ep, structs) = create_fake_smbios();

    let entry = EntryPoint::from(&ep);
    let result = entry.walk_structs(&structs, |_version, hdr, _st| match hdr.r#type {
        StructType::BIOS_INFO => Err(zx_status::Status::STOP),
        StructType::SYSTEM_INFO => panic!("Iterator saw SystemInfo after early stop"),
        _ => panic!("Saw unexpected header type: {:?}", hdr.r#type),
    });

    assert_eq!(result, Ok(()));
}

#[test]
fn test_get_string() {
    let (ep, structs) = create_fake_smbios();

    let entry = EntryPoint::from(&ep);
    let result = entry.walk_structs(&structs, |_version, hdr, st| {
        match hdr.r#type {
            StructType::BIOS_INFO => {
                assert_eq!(st.get_string(0), Ok("<null>"));
                assert_eq!(st.get_string(1), Ok(BIOS_STRING1));
                assert_eq!(st.get_string(2), Ok(BIOS_STRING2));
                assert_eq!(st.get_string(3), Err(zx_status::Status::NOT_FOUND));
                assert_eq!(st.get_string_or_default(0), "<null>");
                assert_eq!(st.get_string_or_default(1), BIOS_STRING1);
                assert_eq!(st.get_string_or_default(2), BIOS_STRING2);
                assert_eq!(st.get_string_or_default(3), "<missing string>");
            }
            StructType::SYSTEM_INFO => {
                assert_eq!(st.get_string(0), Ok("<null>"));
                assert_eq!(st.get_string(1), Err(zx_status::Status::NOT_FOUND));
                assert_eq!(st.get_string_or_default(0), "<null>");
                assert_eq!(st.get_string_or_default(1), "<missing string>");
            }
            _ => panic!("Saw unexpected header type: {:?}", hdr.r#type),
        }
        Ok(())
    });

    assert_eq!(result, Ok(()));
}

#[test]
fn test_baseboard_information_truncations() {
    let mut raw = [0u8; 23];
    let mut baseboard = BaseboardInformationStruct::default();
    baseboard.hdr.r#type = StructType::BASEBOARD;
    baseboard.unsafe_asset_tag_str_idx = 1;
    baseboard.unsafe_feature_flags = 2;
    baseboard.unsafe_location_in_chassis_str_idx = 3;
    baseboard.unsafe_chassis_handle = U16::new(4);
    baseboard.unsafe_board_type = 5;
    baseboard.unsafe_contained_object_handles_count = 6;

    baseboard.hdr.length = 8;
    assert_eq!(baseboard.asset_tag_str_idx(), None);
    assert_eq!(baseboard.feature_flags(), None);
    assert_eq!(baseboard.location_in_chassis_str_idx(), None);
    assert_eq!(baseboard.chassis_handle(), None);
    assert_eq!(baseboard.board_type(), None);
    assert_eq!(baseboard.contained_object_handles_count(), None);

    baseboard.hdr.length = 9;
    assert_eq!(baseboard.asset_tag_str_idx(), Some(1));
    assert_eq!(baseboard.feature_flags(), None);
    assert_eq!(baseboard.location_in_chassis_str_idx(), None);
    assert_eq!(baseboard.chassis_handle(), None);
    assert_eq!(baseboard.board_type(), None);
    assert_eq!(baseboard.contained_object_handles_count(), None);

    baseboard.hdr.length = 10;
    assert_eq!(baseboard.asset_tag_str_idx(), Some(1));
    assert_eq!(baseboard.feature_flags(), Some(2));
    assert_eq!(baseboard.location_in_chassis_str_idx(), None);
    assert_eq!(baseboard.chassis_handle(), None);
    assert_eq!(baseboard.board_type(), None);
    assert_eq!(baseboard.contained_object_handles_count(), None);

    baseboard.hdr.length = 11;
    assert_eq!(baseboard.asset_tag_str_idx(), Some(1));
    assert_eq!(baseboard.feature_flags(), Some(2));
    assert_eq!(baseboard.location_in_chassis_str_idx(), Some(3));
    assert_eq!(baseboard.chassis_handle(), None);
    assert_eq!(baseboard.board_type(), None);
    assert_eq!(baseboard.contained_object_handles_count(), None);

    baseboard.hdr.length = 13;
    assert_eq!(baseboard.asset_tag_str_idx(), Some(1));
    assert_eq!(baseboard.feature_flags(), Some(2));
    assert_eq!(baseboard.location_in_chassis_str_idx(), Some(3));
    assert_eq!(baseboard.chassis_handle(), Some(4));
    assert_eq!(baseboard.board_type(), None);
    assert_eq!(baseboard.contained_object_handles_count(), None);

    baseboard.hdr.length = 14;
    assert_eq!(baseboard.asset_tag_str_idx(), Some(1));
    assert_eq!(baseboard.feature_flags(), Some(2));
    assert_eq!(baseboard.location_in_chassis_str_idx(), Some(3));
    assert_eq!(baseboard.chassis_handle(), Some(4));
    assert_eq!(baseboard.board_type(), Some(5));
    assert_eq!(baseboard.contained_object_handles_count(), None);

    baseboard.hdr.length = 15;
    assert_eq!(baseboard.asset_tag_str_idx(), Some(1));
    assert_eq!(baseboard.feature_flags(), Some(2));
    assert_eq!(baseboard.location_in_chassis_str_idx(), Some(3));
    assert_eq!(baseboard.chassis_handle(), Some(4));
    assert_eq!(baseboard.board_type(), Some(5));
    assert_eq!(baseboard.contained_object_handles_count(), Some(6));

    // Test contained_object_handles
    baseboard.hdr.length = 15 + 4; // 2 handles of 2 bytes each
    baseboard.unsafe_contained_object_handles_count = 2;
    raw[..15].copy_from_slice(baseboard.as_bytes());
    raw[15..17].copy_from_slice(&10u16.to_le_bytes());
    raw[17..19].copy_from_slice(&20u16.to_le_bytes());
    let handles = baseboard.contained_object_handles(&raw).unwrap();
    assert_eq!(handles.len(), 2);
    assert_eq!(handles[0].get(), 10);
    assert_eq!(handles[1].get(), 20);
}

#[test]
fn test_spec_version_includes_version() {
    let v = SpecVersion::new(2, 4, 1);
    assert!(v.includes_version(2, 4, 1));
    assert!(v.includes_version(2, 4, 0));
    assert!(v.includes_version(2, 3, 5));
    assert!(v.includes_version(1, 9, 9));
    assert!(!v.includes_version(2, 4, 2));
    assert!(!v.includes_version(2, 5, 0));
    assert!(!v.includes_version(3, 0, 0));

    let v2 = SpecVersion::new_v2(2, 4);
    assert_eq!(v2, SpecVersion::new(2, 4, 0));
}

#[test]
fn test_walk_structs_invalid_hdr_length() {
    let (ep, mut structs) = create_fake_smbios();
    let entry = EntryPoint::from(&ep);
    // Set header length less than size_of::<Header>() (4)
    structs[1] = 2; // header.length = 2
    let result = entry.walk_structs(&structs, |_ver, _hdr, _st| Ok(()));
    assert_eq!(result, Err(zx_status::Status::IO_DATA_INTEGRITY));
}

#[test]
fn test_entry_point_2_1_validation() {
    let (mut ep, structs) = create_fake_smbios();
    assert!(ep.is_valid());

    // Bad anchor
    ep.anchor_string = *b"_XX_";
    assert!(!ep.is_valid());

    // Restore anchor and test bad length
    ep = create_fake_entry_point(&structs, NUM_STRUCTURES);
    ep.length = 0x10;
    assert!(!ep.is_valid());

    // Test 0x1e length (errata)
    ep = create_fake_entry_point(&structs, NUM_STRUCTURES);
    ep.length = 0x1e;
    ep.intermediate_checksum = 0;
    ep.checksum = 0;
    let ep_bytes = ep.as_bytes();
    let intermediate_sum = compute_checksum(&ep_bytes[0x10..0x1f]);
    ep.intermediate_checksum = (256u32.wrapping_sub(intermediate_sum as u32)) as u8;
    let ep_bytes_updated = ep.as_bytes();
    let full_sum = compute_checksum(&ep_bytes_updated[..0x1f]);
    ep.checksum = (256u32.wrapping_sub(full_sum as u32)) as u8;
    assert!(ep.is_valid());

    // Bad ep_rev
    ep = create_fake_entry_point(&structs, NUM_STRUCTURES);
    ep.ep_rev = 1;
    assert!(!ep.is_valid());

    // Bad intermediate anchor
    ep = create_fake_entry_point(&structs, NUM_STRUCTURES);
    ep.intermediate_anchor_string = *b"_XXX_";
    assert!(!ep.is_valid());

    // Phys + length overflow
    ep = create_fake_entry_point(&structs, NUM_STRUCTURES);
    ep.struct_table_phys = U32::new(u32::MAX);
    ep.struct_table_length = U16::new(10);
    assert!(!ep.is_valid());
}

#[test]
fn test_entry_point_3_0_validation() {
    let (mut ep, _) = create_fake_smbios_v3();
    assert!(ep.is_valid());

    // Bad anchor
    ep.anchor_string = *b"_XXX_";
    assert!(!ep.is_valid());

    // Bad length
    ep = create_fake_v3_entry_point();
    ep.length = 0x10;
    assert!(!ep.is_valid());

    // Bad checksum
    ep = create_fake_v3_entry_point();
    ep.checksum = ep.checksum.wrapping_add(1);
    assert!(!ep.is_valid());
}

#[test]
fn test_string_table_edge_cases() {
    let hdr = Header {
        r#type: StructType::BIOS_INFO,
        length: core::mem::size_of::<Header>() as u8,
        handle: U16::new(0),
    };

    // Truncated: less than 2 bytes after header
    let buf_short = [0u8; 5];
    assert_eq!(
        StringTable::init(&hdr, 5, &buf_short).unwrap_err(),
        zx_status::Status::IO_DATA_INTEGRITY
    );

    // Leading empty string: "\0hello\0\0"
    let mut buf_leading = [0u8; 12];
    buf_leading[4] = 0;
    buf_leading[5..10].copy_from_slice(b"hello");
    buf_leading[10] = 0;
    buf_leading[11] = 0;
    let st = StringTable::init(&hdr, 12, &buf_leading).unwrap();
    assert_eq!(st.get_string(0), Ok("<null>"));
    assert_eq!(st.get_string(1), Ok(""));
    assert_eq!(st.get_string(2), Ok("hello"));
    assert_eq!(st.get_string(3), Err(zx_status::Status::NOT_FOUND));

    // Invalid UTF-8
    let mut buf_bad_utf8 = [0u8; 10];
    buf_bad_utf8[4..8].copy_from_slice(&[0xff, 0xff, 0xff, 0]);
    buf_bad_utf8[8] = 0;
    let st_bad = StringTable::init(&hdr, 10, &buf_bad_utf8).unwrap();
    assert_eq!(st_bad.get_string(1), Err(zx_status::Status::IO_DATA_INTEGRITY));
}

#[test]
fn test_dump_methods() {
    let (ep, structs) = create_fake_smbios();
    let mut out = String::new();

    ep.dump(&mut out).unwrap();
    assert!(out.contains("SMBIOS EntryPoint v2.1:"));
    assert!(out.contains("specification version: 2.1"));

    out.clear();
    let entry = EntryPoint::from(&ep);
    entry
        .walk_structs(&structs, |_ver, hdr, st| {
            st.dump(&mut out).unwrap();
            if hdr.r#type == StructType::BIOS_INFO {
                let bios = BiosInformationStruct2_0::ref_from_prefix(structs.as_slice()).unwrap().0;
                bios.dump(&mut out, st, &structs).unwrap();
            } else if hdr.r#type == StructType::SYSTEM_INFO {
                let sys = SystemInformationStruct2_1::ref_from_prefix(&structs[34..]).unwrap().0;
                sys.dump(&mut out, st).unwrap();
            }
            Ok(())
        })
        .unwrap();

    assert!(out.contains("str 1: string1"));
    assert!(out.contains("str 2: string2"));
    assert!(out.contains("SMBIOS BIOS Information Struct v2.0:"));
    assert!(out.contains("SMBIOS System Information Struct v2.1:"));
}

#[test]
fn test_walk_structs_raw() {
    let (ep, structs) = create_fake_smbios();
    let entry = EntryPoint::from(&ep);
    let addr = structs.as_ptr() as usize;

    let mut count = 0;
    // SAFETY: `addr` points to the valid `structs` buffer which has at least `struct_table_length()` bytes.
    let res = unsafe {
        entry.walk_structs_raw(addr, |_ver, _hdr, _st| {
            count += 1;
            Ok(())
        })
    };
    assert_eq!(res, Ok(()));
    assert_eq!(count, 2);
}
