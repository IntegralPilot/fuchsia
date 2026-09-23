// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use globally_ordered_mock_mmio::MockMmioRegionBuilder;
use mmio::{
    Mmio, MmioSplit, ReadableIndexedRegister, ReadableRegister, Register, WritableIndexedRegister,
    WritableRegister,
};

mod registers {
    use mmio::register;

    register! {
        #[register(offset = 0x00, mode = RO)]
        pub struct Status(u32) {
            pub busy, _: 0;
            pub ready, _: 1;
        }

        #[register(offset = 0x04, mode = WO)]
        pub struct Command(u32) {
            pub _, set_reset: 0;
            pub _, set_start: 1;
        }

        #[register(offset = 0x08, mode = RW)]
        pub struct Data(u32);

        #[register(offset = 0x0c, mode = RW)]
        pub struct Config8(u8);

        #[register(offset = 0x0e, mode = RW)]
        pub struct Config16(u16);

        #[register(offset = 0x10, mode = RW)]
        pub struct Config64(u64);

        #[indexed_register(offset = 0x100, stride = 4, count = 4, mode = RW)]
        pub struct IndexedData(u32);
    }
}

#[fuchsia::test]
fn test_basic_raw_replays() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);

    mock_builder.expect(|t| {
        t.at(0x00).read8(0x12);
        t.at(0x01).write8(0x34);
        t.at(0x02).read16(0x5678);
        t.at(0x04).write16(0x9abc);
        t.at(0x08).read32(0x1234_5678);
        t.at(0x0c).write32(0x9abc_def0);
        t.at(0x10).read64(0x0123_4567_89ab_cdef);
        t.at(0x18).write64(0xfedc_ba98_7654_3210);
        t.write_barrier();
    });

    let mut mmio = mock_builder.build();

    assert_eq!(mmio.load8(0x00), 0x12);
    mmio.store8(0x01, 0x34);
    assert_eq!(mmio.load16(0x02), 0x5678);
    mmio.store16(0x04, 0x9abc);
    assert_eq!(mmio.load32(0x08), 0x1234_5678);
    mmio.store32(0x0c, 0x9abc_def0);
    assert_eq!(mmio.load64(0x10), 0x0123_4567_89ab_cdef);
    mmio.store64(0x18, 0xfedc_ba98_7654_3210);
    mmio.write_barrier();
}

#[fuchsia::test]
fn test_raw_rmw_chaining() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);

    mock_builder.expect(|t| {
        t.at(0x8).read32(0x0000_0001).write32(0x0000_0003);
    });

    let mut mmio = mock_builder.build();

    assert_eq!(mmio.load32(0x8), 0x0000_0001);
    mmio.store32(0x8, 0x0000_0003);
}

#[fuchsia::test]
fn test_register_type_deduction() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);

    mock_builder.expect(|t| {
        t.read::<registers::Status>(0x0000_0001);
        t.write::<registers::Command>(0x0000_0002);
        t.read::<registers::Data>(0xbeef_cafe);
        t.write::<registers::Data>(0xdead_beef);
        t.read::<registers::Config8>(0x42);
        t.write::<registers::Config8>(0x43);
        t.read::<registers::Config16>(0x1234);
        t.write::<registers::Config16>(0x5678);
        t.read::<registers::Config64>(0x0123_4567_89ab_cdef);
        t.write::<registers::Config64>(0xfedc_ba98_7654_3210);
    });

    let mut mmio = mock_builder.build();

    let status = registers::Status::read(&mmio);
    assert!(status.busy());
    assert!(!status.ready());

    let mut cmd = registers::Command::default();
    cmd.set_start(true);
    cmd.write(&mut mmio);

    assert_eq!(registers::Data::read(&mmio).value(), 0xbeef_cafe);
    registers::Data(0xdead_beef).write(&mut mmio);

    assert_eq!(registers::Config8::read(&mmio).value(), 0x42);
    registers::Config8(0x43).write(&mut mmio);

    assert_eq!(registers::Config16::read(&mmio).value(), 0x1234);
    registers::Config16(0x5678).write(&mut mmio);

    assert_eq!(registers::Config64::read(&mmio).value(), 0x0123_4567_89ab_cdef);
    registers::Config64(0xfedc_ba98_7654_3210).write(&mut mmio);
}

#[fuchsia::test]
fn test_indexed_register() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);

    mock_builder.expect(|t| {
        t.read_indexed::<registers::IndexedData>(0, 0x10);
        t.write_indexed::<registers::IndexedData>(1, 0x20);
        t.read_indexed::<registers::IndexedData>(2, 0x30);
        t.write_indexed::<registers::IndexedData>(3, 0x40);
    });

    let mut mmio = mock_builder.build();

    assert_eq!(registers::IndexedData::read_index(&mmio, 0).value(), 0x10);
    registers::IndexedData(0x20).write_index(&mut mmio, 1);
    assert_eq!(registers::IndexedData::read_index(&mmio, 2).value(), 0x30);
    registers::IndexedData(0x40).write_index(&mut mmio, 3);
}

#[fuchsia::test]
#[should_panic(expected = "Register index 4 out of bounds")]
fn test_indexed_register_out_of_bounds() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);
    mock_builder.expect(|t| {
        t.read_indexed::<registers::IndexedData>(4, 0x50);
    });
}

#[fuchsia::test]
fn test_poll_sequence() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);

    mock_builder.expect(|t| {
        t.poll::<registers::Status>([0x0000_0001, 0x0000_0001, 0x0000_0002]);
        t.write::<registers::Command>(0x0000_0001);
    });

    let mut mmio = mock_builder.build();

    // 1st read: busy (1)
    let s1 = registers::Status::read(&mmio);
    assert_eq!(s1.to_raw(), 1);

    // 2nd read: busy (1)
    let s2 = registers::Status::read(&mmio);
    assert_eq!(s2.to_raw(), 1);

    // 3rd read: ready (2)
    let s3 = registers::Status::read(&mmio);
    assert_eq!(s3.to_raw(), 2);

    // 4th read: still returns last value (2)
    let s4 = registers::Status::read(&mmio);
    assert_eq!(s4.to_raw(), 2);

    // Write transitions to the next expectation
    registers::Command(1).write(&mut mmio);
}

#[fuchsia::test]
fn test_poll_indefinitely() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);

    mock_builder.expect(|t| {
        t.poll_indefinitely::<registers::Status>(0x0000_0001);
        t.write::<registers::Command>(0x0000_0001);
    });

    let mut mmio = mock_builder.build();

    // Indefinite busy reads
    for _ in 0..10 {
        assert_eq!(registers::Status::read(&mmio).to_raw(), 1);
    }

    // Driver gives up and resets
    registers::Command(1).write(&mut mmio);
}

#[fuchsia::test]
fn test_poll_indefinitely_trailing_satisfied() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);

    mock_builder.expect(|t| {
        t.write::<registers::Command>(0x0000_0001);
        t.poll_indefinitely::<registers::Status>(0x0000_0001);
    });

    let mut mmio = mock_builder.build();
    registers::Command(1).write(&mut mmio);
    for _ in 0..5 {
        assert_eq!(registers::Status::read(&mmio).to_raw(), 1);
    }
}

#[fuchsia::test]
fn test_mmio_split_multithreaded() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);

    mock_builder.expect(|t| {
        t.at(0x0000).write32(0x1111);
        t.at(0x0100).write32(0x2222);
    });

    let mut mmio = mock_builder.build();
    let mut r1 = mmio.split_off(0x0100);
    let mut r2 = mmio.split_off(0x0100);

    let (tx, rx) = std::sync::mpsc::channel();

    let handle1 = std::thread::spawn(move || {
        r1.store32(0x0, 0x1111);
        tx.send(()).unwrap();
    });

    let handle2 = std::thread::spawn(move || {
        rx.recv().unwrap();
        r2.store32(0x0, 0x2222);
    });

    handle1.join().unwrap();
    handle2.join().unwrap();
}

#[fuchsia::test]
#[should_panic(expected = "MMIO EXPECTATION MISMATCH")]
fn test_mismatch_offset_panic() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);

    mock_builder.expect(|t| {
        t.at(0x0).read32(0x1234);
    });

    let mmio = mock_builder.build();
    let _ = mmio.load32(0x4);
}

#[fuchsia::test]
#[should_panic(expected = "MMIO EXPECTATION MISMATCH")]
fn test_mismatch_value_panic() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);

    mock_builder.expect(|t| {
        t.at(0x0).write32(0x1234);
    });

    let mut mmio = mock_builder.build();
    mmio.store32(0x0, 0x5678);
}

#[fuchsia::test]
#[should_panic(expected = "UNEXPECTED MMIO ACCESS")]
fn test_unexpected_access_panic() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);

    let mmio = mock_builder.build();
    let _ = mmio.load32(0x0);
}

#[fuchsia::test]
#[should_panic(expected = "UNRETIRED MMIO EXPECTATIONS")]
fn test_unretired_expectations_panic() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);

    mock_builder.expect(|t| {
        t.at(0x0).read32(0x1234);
    });
}

#[fuchsia::test]
#[should_panic(expected = "MMIO EXPECTATION MISMATCH")]
fn test_poll_premature_transition_panics() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);

    mock_builder.expect(|t| {
        t.poll::<registers::Status>([0x0000_0001, 0x0000_0002]);
        t.write::<registers::Command>(0x0000_0001);
    });

    let mut mmio = mock_builder.build();

    // 1st read returns 1. 2nd read (2) has not occurred.
    let s1 = registers::Status::read(&mmio);
    assert_eq!(s1.to_raw(), 1);

    // Premature write should panic because poll is not exhausted.
    registers::Command(1).write(&mut mmio);
}

#[fuchsia::test]
#[should_panic(expected = "UNRETIRED MMIO EXPECTATIONS")]
fn test_poll_unretired_panics() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);

    mock_builder.expect(|t| {
        t.poll::<registers::Status>([0x0000_0001, 0x0000_0002]);
    });

    let mmio = mock_builder.build();

    // Only 1 read of 2
    let _ = registers::Status::read(&mmio);
}

#[fuchsia::test]
#[should_panic(expected = "UNRETIRED MMIO EXPECTATIONS")]
fn test_poll_single_value_unretired_panics() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);

    mock_builder.expect(|t| {
        t.poll::<registers::Status>([0x0000_0001]);
    });

    // 0 reads performed
}

#[fuchsia::test]
#[should_panic(expected = "MMIO EXPECTATION MISMATCH")]
fn test_poll_single_value_premature_write_panics() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);

    mock_builder.expect(|t| {
        t.poll::<registers::Status>([0x0000_0001]);
        t.write::<registers::Command>(0x0000_0001);
    });

    let mut mmio = mock_builder.build();

    // 0 reads performed before write
    registers::Command(1).write(&mut mmio);
}

#[fuchsia::test]
#[should_panic(expected = "UNRETIRED MMIO EXPECTATIONS")]
fn test_poll_indefinitely_unretired_zero_reads_panics() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);

    mock_builder.expect(|t| {
        t.poll_indefinitely::<registers::Status>(0x0000_0001);
    });

    // 0 reads performed
}

#[fuchsia::test]
#[should_panic(expected = "MMIO EXPECTATION MISMATCH")]
fn test_poll_indefinitely_zero_reads_premature_write_panics() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);

    mock_builder.expect(|t| {
        t.poll_indefinitely::<registers::Status>(0x0000_0001);
        t.write::<registers::Command>(0x0000_0001);
    });

    let mut mmio = mock_builder.build();

    // 0 reads performed before write: should panic because poll_indefinitely requires at least 1 read
    registers::Command(1).write(&mut mmio);
}

#[fuchsia::test]
fn test_region_size_bytes() {
    let mock_builder = MockMmioRegionBuilder::new(0x2000);
    assert_eq!(mock_builder.region_size_bytes(), 0x2000);
}

#[fuchsia::test]
#[should_panic(expected = "exceeds region size")]
fn test_expectation_out_of_bounds_panic() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);
    mock_builder.expect(|t| {
        t.at(0x1000).read8(0);
    });
}

#[fuchsia::test]
#[should_panic(expected = "exceeds region size")]
fn test_expectation_offset_overflow_panic() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);
    mock_builder.expect(|t| {
        t.at(usize::MAX).read8(0);
    });
}

#[fuchsia::test]
#[should_panic(expected = "is unaligned")]
fn test_expectation_unaligned_access_panic() {
    let mock_builder = MockMmioRegionBuilder::new(0x1000);
    mock_builder.expect(|t| {
        t.at(0x1).read32(0);
    });
}

#[fuchsia::test]
fn test_mmio_operand_to_u64() {
    assert_eq!(globally_ordered_mock_mmio::mmio_operand_to_u64(0x5au8), 0x5a);
    assert_eq!(globally_ordered_mock_mmio::mmio_operand_to_u64(0x1234u16), 0x1234);
    assert_eq!(globally_ordered_mock_mmio::mmio_operand_to_u64(0x1234_5678u32), 0x1234_5678);
    assert_eq!(
        globally_ordered_mock_mmio::mmio_operand_to_u64(0x1234_5678_9abc_def0u64),
        0x1234_5678_9abc_def0
    );
}
