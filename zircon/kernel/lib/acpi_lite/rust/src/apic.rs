// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::binary_reader::{BinaryReader, DowncastFrom};
use crate::structures::{
    ACPI_MADT_FLAG_ENABLED, ACPI_MADT_TYPE_INT_SOURCE_OVERRIDE, ACPI_MADT_TYPE_IO_APIC,
    ACPI_MADT_TYPE_LOCAL_APIC, AcpiMadtIntSourceOverrideEntry, AcpiMadtIoApicEntry,
    AcpiMadtLocalApicEntry, AcpiMadtTable, AcpiSubTableHeader, VariableSized,
};
use crate::{AcpiParserInterface, get_table_by_type};
use zx_status::Status;

impl_downcast_from!(AcpiSubTableHeader =>
    AcpiMadtLocalApicEntry,
    AcpiMadtIoApicEntry,
    AcpiMadtIntSourceOverrideEntry,
);

fn for_each_madt_entry_of_type<'a, T, F>(
    parser: &'a dyn AcpiParserInterface,
    r#type: u8,
    mut visitor: F,
) -> Result<(), Status>
where
    T: VariableSized + DowncastFrom<AcpiSubTableHeader> + 'a,
    F: FnMut(&T) -> Result<(), Status>,
{
    let table = get_table_by_type::<AcpiMadtTable>(parser).ok_or(Status::NOT_FOUND)?;
    let mut reader = BinaryReader::from_payload_of_struct(table);
    while !reader.is_empty() {
        let header = reader.read::<AcpiSubTableHeader>().ok_or(Status::INTERNAL)?;
        if header.r#type != r#type {
            continue;
        }
        // SAFETY: We verified that `header.r#type` matches the expected type for `T`
        // by checking it against `r#type` passed to this function.
        let entry = unsafe { T::downcast_from(header) }.ok_or(Status::INTERNAL)?;
        visitor(entry)?;
    }
    Ok(())
}

// Enumerate all enabled Processor Local APICs in the system, calling
// |callback| once for each.
//
// Each entry corresponds to an entry in the ACPI MADT table of type "Local
// Apic". See ACPI v6.3 Section 5.2.12.2 for details.
//
// |callback| is invoked once for each entry. The function returns any error
// returned by the callback, or if an error was found attempting to parse the
// tables.
pub fn enumerate_processor_local_apics<F>(
    parser: &dyn AcpiParserInterface,
    mut callback: F,
) -> Result<(), Status>
where
    F: FnMut(&AcpiMadtLocalApicEntry) -> Result<(), Status>,
{
    for_each_madt_entry_of_type::<AcpiMadtLocalApicEntry, _>(
        parser,
        ACPI_MADT_TYPE_LOCAL_APIC,
        |record| {
            let flags = record.flags;
            if (flags & ACPI_MADT_FLAG_ENABLED) == 0 {
                return Ok(());
            }
            callback(record)
        },
    )
}

// Enumerate all IO APICs in the system, calling |callback| once for each.
//
// |callback| is called onced per IO APIC entry in the ACPI MADT table. The
// function returns any error returned by the callback, or if an error was
// found attempting to parse the tables.
pub fn enumerate_io_apics<F>(parser: &dyn AcpiParserInterface, callback: F) -> Result<(), Status>
where
    F: FnMut(&AcpiMadtIoApicEntry) -> Result<(), Status>,
{
    for_each_madt_entry_of_type::<AcpiMadtIoApicEntry, F>(parser, ACPI_MADT_TYPE_IO_APIC, callback)
}

// Enumerate all ISA interrupt source override entries in the system MADT
// table.
//
// By default, it is assumed that the first _n_ APIC interrupts correspond to
// the first _n_ legacy ISA interrupts. Entries in this table record any
// exceptions to this assumption.
//
// |callback| is called once per override entry in the ACPI MADT table. The
// function returns any error returned by the callback, or if an error was
// found attempting to parse the tables.
pub fn enumerate_io_apic_isa_overrides<F>(
    parser: &dyn AcpiParserInterface,
    callback: F,
) -> Result<(), Status>
where
    F: FnMut(&AcpiMadtIntSourceOverrideEntry) -> Result<(), Status>,
{
    for_each_madt_entry_of_type::<AcpiMadtIntSourceOverrideEntry, F>(
        parser,
        ACPI_MADT_TYPE_INT_SOURCE_OVERRIDE,
        callback,
    )
}
