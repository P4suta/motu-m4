use crate::{Budget, Error, Machine, MemoryLayout, Peripheral, Stop};
use motu_m4_core::FirmwareImage;

const BASE: u32 = 0x2000_0000;

fn container(code: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0; 64];
    bytes[4..8].copy_from_slice(b"test");
    bytes.extend_from_slice(&(BASE + 4096).to_le_bytes());
    bytes.extend_from_slice(&(BASE + 9).to_le_bytes());
    bytes.extend_from_slice(code);
    let length = (bytes.len() - 64) as u32;
    bytes[..4].copy_from_slice(&length.to_le_bytes());
    bytes
}

fn machine(bytes: &[u8]) -> Machine {
    Machine::new(
        FirmwareImage::parse(bytes).unwrap(),
        MemoryLayout::new(BASE, 4096).unwrap(),
    )
    .unwrap()
}

#[test]
fn executes_arithmetic_and_stops_at_a_breakpoint() {
    let bytes = container(&[5, 0x20, 2, 0x21, 0x42, 0x1a, 7, 0xbe]);
    let mut machine = machine(&bytes);
    let result = machine.run(Budget::new(10).unwrap()).unwrap();
    assert_eq!(result.stop, Stop::Breakpoint(7));
    assert_eq!(result.instructions, 4);
    assert_eq!(&result.registers[..3], &[5, 2, 3]);
    assert_eq!(result.registers[15], BASE + 16);
    assert_eq!(result.trace, [BASE + 8, BASE + 10, BASE + 12, BASE + 14]);
    assert_eq!(
        machine.run(Budget::new(1).unwrap()).unwrap_err(),
        Error::Finished
    );
}

#[test]
fn bounds_loops_and_waits_without_hanging() {
    for (code, stop, instructions) in [
        ([0xfe, 0xe7], Stop::Budget, 100),
        ([0x30, 0xbf], Stop::WaitForInterrupt, 1),
        ([0x20, 0xbf], Stop::WaitForEvent, 1),
    ] {
        let bytes = container(&code);
        let result = machine(&bytes).run(Budget::new(100).unwrap()).unwrap();
        assert_eq!(result.stop, stop);
        assert_eq!(result.instructions, instructions);
        assert_eq!(result.trace.len(), instructions.min(64) as usize);
    }
}

#[test]
fn isolated_calls_return_through_lr_and_preserve_the_stack() {
    let bytes = container(&[0x40, 0x18, 0x70, 0x47]);
    let mut machine = machine(&bytes);
    machine.prepare_call(BASE + 9, [5, 7, 0, 0]).unwrap();
    let result = machine.run(Budget::new(10).unwrap()).unwrap();
    assert_eq!(result.stop, Stop::Returned);
    assert_eq!(result.instructions, 2);
    assert_eq!(result.registers[0], 12);
    assert_eq!(result.registers[13], BASE + 4096);
    assert_eq!(result.registers[14], 0xffff_ff01);
}

#[test]
fn bounded_decoding_preserves_disassembly_and_rejects_static_registration() {
    use armagnac::core::{ArmVersion, ItState};
    use armagnac::decoder::{BasicInstructionDecoder, InstructionDecode};
    use armagnac::instructions::{Instruction, InstructionSize, Mnemonic};
    use std::cell::Cell;
    use std::rc::Rc;

    let decoder = crate::BoundedDecoder {
        inner: Box::new(BasicInstructionDecoder::new(ArmVersion::V7EM)),
        waiting: Rc::new(Cell::new(None)),
    };
    for (bits, size, expected) in [
        (0x2005, InstructionSize::Ins16, "movs     r0, #5"),
        (0xf04f_0005, InstructionSize::Ins32, "mov.w    r0, #5"),
    ] {
        let instruction = decoder.try_decode(bits, size, ItState::default()).unwrap();
        assert_eq!(instruction.mnemonic(BASE + 8, None), expected);
    }
    assert!(std::panic::catch_unwind(crate::BoundedInstruction::patterns).is_err());
}

#[test]
fn validates_layout_vectors_entries_and_budgets() {
    for (base, length) in [
        (BASE + 1, 4096),
        (BASE, 0),
        (BASE, 3),
        (0xffff_f000, 4096),
        (BASE, 1_048_580),
        (0xffff_fe00, 0x104),
    ] {
        assert!(MemoryLayout::new(base, length).is_err());
    }
    for instructions in [0, 10_000_001, u32::MAX] {
        assert_eq!(Budget::new(instructions).unwrap_err(), Error::Budget);
    }
    assert!(Budget::new(1).is_ok());
    assert!(Budget::new(10_000_000).is_ok());
    let mut bytes = container(&[0x70, 0x47]);
    for stack in [BASE, BASE + 4, BASE + 4097, BASE + 4104] {
        bytes[64..68].copy_from_slice(&stack.to_le_bytes());
        assert!(matches!(
            Machine::new(
                FirmwareImage::parse(&bytes).unwrap(),
                MemoryLayout::new(BASE, 4096).unwrap()
            ),
            Err(Error::StackPointer)
        ));
    }
    bytes[64..68].copy_from_slice(&(BASE + 4096).to_le_bytes());
    for entry in [BASE + 8, BASE - 1, BASE + 11] {
        bytes[68..72].copy_from_slice(&entry.to_le_bytes());
        assert!(matches!(
            Machine::new(
                FirmwareImage::parse(&bytes).unwrap(),
                MemoryLayout::new(BASE, 4096).unwrap()
            ),
            Err(Error::EntryPoint)
        ));
    }
    let bytes = container(&[0x70, 0x47]);
    let mut machine = machine(&bytes);
    for entry in [BASE + 8, BASE - 1, BASE + 11] {
        assert_eq!(machine.prepare_call(entry, [0; 4]), Err(Error::EntryPoint));
    }
    assert_eq!(machine.read(BASE - 1, 1), Err(Error::MemoryAccess));
    assert_eq!(machine.read(BASE + 4096, 1), Err(Error::MemoryAccess));
    assert_eq!(machine.read(BASE + 8, 2).unwrap(), [0x70, 0x47]);
}

#[test]
fn the_smallest_stack_and_an_image_that_fills_ram_are_valid_boundaries() {
    let mut bytes = container(&[0x70, 0x47, 0, 0]);
    bytes[64..68].copy_from_slice(&(BASE + 8).to_le_bytes());
    let image = FirmwareImage::parse(&bytes).unwrap();
    let mut machine = Machine::new(image, MemoryLayout::new(BASE, 12).unwrap()).unwrap();
    machine.prepare_call(BASE + 9, [0; 4]).unwrap();
    let result = machine.run(Budget::new(1).unwrap()).unwrap();
    assert_eq!(result.stop, Stop::Returned);
    assert_eq!(result.registers[13], BASE + 8);
    assert!(MemoryLayout::new(BASE, 8).is_ok());
    assert!(MemoryLayout::new(BASE, 1_048_576).is_ok());
    assert!(MemoryLayout::new(0xffff_fe00, 0x100).is_ok());
    assert!(MemoryLayout::new(0xffff_ff04, 8).is_ok());
    assert!(matches!(
        Machine::new(image, MemoryLayout::new(BASE, 8).unwrap()),
        Err(Error::ImageTooLarge)
    ));
}

#[test]
fn undefined_instructions_and_unmapped_accesses_stop_the_guest() {
    for code in [&[0, 0xde][..], &[0, 0x68][..]] {
        let bytes = container(code);
        let result = machine(&bytes).run(Budget::new(100).unwrap()).unwrap();
        assert!(matches!(result.stop, Stop::Fault(_)), "{result:?}");
        assert_eq!(result.instructions, 0);
        assert_eq!(result.trace, [BASE + 8]);
    }
}

#[test]
fn wait_effects_follow_wide_encodings_conditions_and_the_event_register() {
    for (code, stop, count) in [
        (&[0xaf, 0xf3, 3, 0x80][..], Stop::WaitForInterrupt, 1),
        (&[0xaf, 0xf3, 2, 0x80][..], Stop::WaitForEvent, 1),
        (
            &[1, 0x20, 8, 0xbf, 0x30, 0xbf, 7, 0xbe][..],
            Stop::Breakpoint(7),
            4,
        ),
        (
            &[0x40, 0xbf, 0x20, 0xbf, 7, 0xbe][..],
            Stop::Breakpoint(7),
            3,
        ),
    ] {
        let bytes = container(code);
        let result = machine(&bytes).run(Budget::new(10).unwrap()).unwrap();
        assert_eq!(result.stop, stop);
        assert_eq!(result.instructions, count);
    }
}

#[test]
fn returned_calls_can_repeat_with_shared_memory_and_a_fresh_budget() {
    let bytes = container(&[0x40, 0x18, 0x70, 0x47]);
    let mut machine = machine(&bytes);
    for i in 0..3 {
        machine.write(BASE + 100, &[i as u8; 4]).unwrap();
        machine.prepare_call(BASE + 9, [i, 7, 0, 0]).unwrap();
        let result = machine.run(Budget::new(2).unwrap()).unwrap();
        assert_eq!(result.stop, Stop::Returned);
        assert_eq!(result.instructions, 2);
        assert_eq!(result.registers[0], i + 7);
        assert_eq!(machine.read(BASE + 100, 4).unwrap(), [i as u8; 4]);
    }
    let bytes = container(&[0x30, 0xbf]);
    let mut machine = self::machine(&bytes);
    machine.run(Budget::new(1).unwrap()).unwrap();
    assert_eq!(machine.prepare_call(BASE + 9, [0; 4]), Err(Error::Finished));
    assert_eq!(machine.write(BASE, &[0]), Err(Error::Finished));
    assert_eq!(machine.attach(Peripheral::NxpUsbSram), Err(Error::Finished));
}

#[test]
fn mapped_backup_registers_record_word_accesses_and_reject_byte_accesses() {
    let mut code = vec![1, 0x49, 8, 0x60, 0x0a, 0x68, 0x70, 0x47];
    code.extend_from_slice(&0x400e_1890_u32.to_le_bytes());
    let bytes = container(&code);
    let mut machine = machine(&bytes);
    machine.attach(Peripheral::SamBackupRegisters).unwrap();
    assert_eq!(
        machine.attach(Peripheral::SamBackupRegisters),
        Err(Error::PeripheralConflict)
    );
    machine
        .prepare_call(BASE + 9, [0x12345678, 0, 0, 0])
        .unwrap();
    let result = machine.run(Budget::new(10).unwrap()).unwrap();
    assert_eq!(result.stop, Stop::Returned);
    assert_eq!(result.registers[2], 0x12345678);
    assert_eq!(result.peripheral_accesses.len(), 2);
    assert_eq!(result.peripheral_accesses[0].instruction, 1);
    assert_eq!(result.peripheral_accesses[0].address, 0x400e1890);
    assert_eq!(result.peripheral_accesses[0].value, 0x12345678);
    assert!(result.peripheral_accesses[0].write);
    assert!(!result.peripheral_accesses[1].write);
    assert_eq!(result.peripheral_accesses[1].instruction, 2);
    code[5] = 0x78;
    let bytes = container(&code);
    let mut machine = self::machine(&bytes);
    machine.attach(Peripheral::SamBackupRegisters).unwrap();
    machine.prepare_call(BASE + 9, [0; 4]).unwrap();
    assert!(matches!(
        machine.run(Budget::new(10).unwrap()).unwrap().stop,
        Stop::Fault(_)
    ));
}

#[test]
fn usb_sram_has_exact_bounds_and_is_accessible_to_instructions() {
    let mut code = vec![1, 0x49, 8, 0x60, 0x0a, 0x68, 0x70, 0x47];
    code.extend_from_slice(&0x4010_3ffc_u32.to_le_bytes());
    let bytes = container(&code);
    let mut machine = machine(&bytes);
    assert_eq!(machine.read(0x40100000, 1), Err(Error::MemoryAccess));
    machine.attach(Peripheral::NxpUsbSram).unwrap();
    assert_eq!(
        machine.attach(Peripheral::NxpUsbSram),
        Err(Error::PeripheralConflict)
    );
    machine.write(0x40100000, &[5]).unwrap();
    machine
        .prepare_call(BASE + 9, [0x12345678, 0, 0, 0])
        .unwrap();
    let result = machine.run(Budget::new(10).unwrap()).unwrap();
    assert_eq!(result.stop, Stop::Returned);
    assert_eq!(result.registers[2], 0x12345678);
    assert_eq!(machine.read(0x40100000, 1).unwrap(), [5]);
    assert_eq!(
        machine.read(0x40103ffc, 4).unwrap(),
        [0x78, 0x56, 0x34, 0x12]
    );
    assert_eq!(machine.read(0x40103ffc, 5), Err(Error::MemoryAccess));
    assert_eq!(machine.write(0x40103fff, &[1, 2]), Err(Error::MemoryAccess));
    assert_eq!(
        machine.read(0x40103ffc, 4).unwrap(),
        [0x78, 0x56, 0x34, 0x12]
    );
    assert_eq!(
        machine.read(0x40100000, usize::MAX),
        Err(Error::MemoryAccess)
    );
}

#[test]
fn deterministic_instruction_inputs_cannot_escape_the_execution_budget() {
    let mut seed = 0x8f73_4261_u32;
    for _ in 0..512 {
        let mut code = [0; 32];
        for byte in &mut code {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *byte = (seed >> 24) as u8;
        }
        let bytes = container(&code);
        let mut machine = machine(&bytes);
        let result = machine.run(Budget::new(16).unwrap()).unwrap();
        assert!(result.instructions <= 16, "{code:02x?}: {result:?}");
        assert!(!result.trace.is_empty());
        assert!(result.trace.len() <= 17);
        assert_eq!(
            machine.run(Budget::new(1).unwrap()).unwrap_err(),
            Error::Finished
        );
    }
}
