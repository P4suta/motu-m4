#![forbid(unsafe_code)]

mod atmel;
mod audio;
mod nxp;
mod peripherals;
#[cfg(kani)]
mod proofs;
mod sam_usb;
#[cfg(test)]
mod tests;

pub use armagnac::core::RunError as CpuFault;
use armagnac::core::{Condition, Config, Effect, Emulator, Event, ItState, Processor, RunError};
use armagnac::decoder::{InstructionDecode, InstructionDecodeError};
use armagnac::instructions::{Instruction, InstructionSize};
use armagnac::memory::RamMemory;
pub use atmel::AtmelAudioControl;
pub use audio::{FirmwareAudioControl, FirmwareControl, FirmwareFamily};
use motu_m4_core::FirmwareImage;
pub use nxp::{ControlError, NxpAudioControl};
pub use peripherals::{Access, Peripheral};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;

const RETURN_ADDRESS: u32 = 0xffff_ff00;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    MemoryLayout,
    ImageTooLarge,
    StackPointer,
    EntryPoint,
    Budget,
    Finished,
    MemoryAccess,
    PeripheralConflict,
}

#[derive(Debug, Clone, Copy)]
pub struct MemoryLayout {
    base: u32,
    bytes: u32,
}

impl MemoryLayout {
    pub fn new(base: u32, bytes: u32) -> Result<Self, Error> {
        if base & 3 != 0
            || bytes < 8
            || bytes & 3 != 0
            || bytes > 1_048_576
            || base.checked_add(bytes).is_none()
            || (base <= RETURN_ADDRESS && base + bytes > RETURN_ADDRESS)
        {
            return Err(Error::MemoryLayout);
        }
        Ok(Self { base, bytes })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Budget(u32);

impl Budget {
    pub fn new(instructions: u32) -> Result<Self, Error> {
        if !(1..=10_000_000).contains(&instructions) {
            return Err(Error::Budget);
        }
        Ok(Self(instructions))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    Budget,
    Returned,
    Breakpoint(u8),
    WaitForInterrupt,
    WaitForEvent,
    Fault(CpuFault),
    ProcessorPanic,
    Reset,
    DebugHint(u8),
}

#[derive(Debug)]
pub struct Execution {
    pub stop: Stop,
    pub instructions: u64,
    pub registers: [u32; 16],
    pub trace: Vec<u32>,
    pub peripheral_accesses: Vec<Access>,
}

pub struct Machine {
    processor: Processor,
    ram: Rc<RefCell<RamMemory>>,
    layout: MemoryLayout,
    image_bytes: u32,
    waiting: Rc<Cell<Option<StopWait>>>,
    finished: bool,
    call: bool,
    returned: bool,
    usb_ram: Option<Rc<RefCell<RamMemory>>>,
    accesses: Rc<RefCell<VecDeque<Access>>>,
    sam_usb: Option<Rc<RefCell<sam_usb::UsbState>>>,
}

#[derive(Clone, Copy)]
enum StopWait {
    Interrupt,
    Event,
}

impl Machine {
    pub fn new(image: FirmwareImage<'_>, layout: MemoryLayout) -> Result<Self, Error> {
        let image_bytes = u32::try_from(image.payload().len()).map_err(|_| Error::ImageTooLarge)?;
        if image_bytes > layout.bytes {
            return Err(Error::ImageTooLarge);
        }
        let stack = image.initial_stack_pointer();
        if stack & 7 != 0 || stack < layout.base + 8 || stack > layout.base + layout.bytes {
            return Err(Error::StackPointer);
        }
        validate_entry(layout.base, image_bytes, image.reset_handler())?;
        let mut processor = Processor::new(Config::v7em());
        let ram = processor
            .map_ram(layout.base, layout.bytes)
            .map_err(|_| Error::MemoryLayout)?;
        ram.borrow_mut().data[..image_bytes as usize].copy_from_slice(image.payload());
        processor.set_sp(stack);
        processor.set_pc(image.reset_handler() & !1);
        processor.registers.psr.set_t(true);
        let waiting = Rc::new(Cell::new(None));
        let inner = std::mem::replace(
            &mut processor.instruction_decoder,
            Box::new(armagnac::decoder::BasicInstructionDecoder::new(
                armagnac::core::ArmVersion::V7EM,
            )),
        );
        processor.instruction_decoder = Box::new(BoundedDecoder {
            inner,
            waiting: waiting.clone(),
        });
        Ok(Self {
            processor,
            ram,
            layout,
            image_bytes,
            waiting,
            finished: false,
            call: false,
            returned: false,
            usb_ram: None,
            accesses: Rc::new(RefCell::new(VecDeque::new())),
            sam_usb: None,
        })
    }

    pub fn prepare_call(&mut self, entry: u32, args: [u32; 4]) -> Result<(), Error> {
        if self.finished && !self.returned {
            return Err(Error::Finished);
        }
        validate_entry(self.layout.base, self.image_bytes, entry)?;
        let registers = &mut self.processor.registers;
        [registers.r0, registers.r1, registers.r2, registers.r3] = args;
        self.processor.set_lr(RETURN_ADDRESS + 1);
        self.processor.set_pc(entry & !1);
        self.call = true;
        self.finished = false;
        self.returned = false;
        Ok(())
    }

    pub fn run(&mut self, budget: Budget) -> Result<Execution, Error> {
        if self.finished {
            return Err(Error::Finished);
        }
        self.finished = true;
        let start = self.processor.cycles;
        let mut trace = VecDeque::with_capacity(64);
        let stop = loop {
            if self.call && self.processor.pc() == RETURN_ADDRESS {
                break Stop::Returned;
            }
            if self.processor.cycles - start >= u64::from(budget.0) {
                break Stop::Budget;
            }
            if trace.len() == 64 {
                trace.pop_front();
            }
            trace.push_back(self.processor.pc());
            let event = catch_unwind(AssertUnwindSafe(|| self.processor.next_event()));
            if let Some(wait) = self.waiting.get() {
                break match wait {
                    StopWait::Interrupt => Stop::WaitForInterrupt,
                    StopWait::Event => Stop::WaitForEvent,
                };
            }
            match event {
                Err(_) => break Stop::ProcessorPanic,
                Ok(Err(error)) => break Stop::Fault(error),
                Ok(Ok(Event::Break(value))) => break Stop::Breakpoint(value),
                Ok(Ok(Event::Reset)) => break Stop::Reset,
                Ok(Ok(Event::DebugHint(value))) => break Stop::DebugHint(value),
                Ok(Ok(Event::Hook { .. })) => break Stop::ProcessorPanic,
                Ok(Ok(Event::Instruction { .. })) => {}
            }
        };
        let r = &self.processor.registers;
        self.returned = stop == Stop::Returned;
        Ok(Execution {
            stop,
            instructions: self.processor.cycles - start,
            registers: [
                r.r0,
                r.r1,
                r.r2,
                r.r3,
                r.r4,
                r.r5,
                r.r6,
                r.r7,
                r.r8,
                r.r9,
                r.r10,
                r.r11,
                r.r12,
                r.sp(),
                r.lr,
                r.pc,
            ],
            trace: trace.into(),
            peripheral_accesses: self.accesses.borrow().iter().copied().collect(),
        })
    }

    pub fn read(&self, address: u32, bytes: usize) -> Result<Vec<u8>, Error> {
        if let Some(ram) = self
            .usb_ram
            .as_ref()
            .filter(|_| (0x4010_0000..0x4010_4000).contains(&address))
        {
            let start = (address - 0x4010_0000) as usize;
            let end = start.checked_add(bytes).ok_or(Error::MemoryAccess)?;
            return ram
                .borrow()
                .data
                .get(start..end)
                .map(<[u8]>::to_vec)
                .ok_or(Error::MemoryAccess);
        }
        let start = address
            .checked_sub(self.layout.base)
            .ok_or(Error::MemoryAccess)? as usize;
        let end = start.checked_add(bytes).ok_or(Error::MemoryAccess)?;
        self.ram
            .borrow()
            .data
            .get(start..end)
            .map(<[u8]>::to_vec)
            .ok_or(Error::MemoryAccess)
    }

    pub fn write(&mut self, address: u32, bytes: &[u8]) -> Result<(), Error> {
        if self.finished && !self.returned {
            return Err(Error::Finished);
        }
        if let Some(ram) = self
            .usb_ram
            .as_ref()
            .filter(|_| (0x4010_0000..0x4010_4000).contains(&address))
        {
            let start = (address - 0x4010_0000) as usize;
            let end = start.checked_add(bytes.len()).ok_or(Error::MemoryAccess)?;
            ram.borrow_mut()
                .data
                .get_mut(start..end)
                .ok_or(Error::MemoryAccess)?
                .copy_from_slice(bytes);
            return Ok(());
        }
        let start = address
            .checked_sub(self.layout.base)
            .ok_or(Error::MemoryAccess)? as usize;
        let end = start.checked_add(bytes.len()).ok_or(Error::MemoryAccess)?;
        self.ram
            .borrow_mut()
            .data
            .get_mut(start..end)
            .ok_or(Error::MemoryAccess)?
            .copy_from_slice(bytes);
        Ok(())
    }

    pub fn attach(&mut self, peripheral: Peripheral) -> Result<(), Error> {
        if self.finished {
            return Err(Error::Finished);
        }
        match peripheral {
            Peripheral::NxpUsbSram => {
                if self.usb_ram.is_some() {
                    return Err(Error::PeripheralConflict);
                }
                self.usb_ram = Some(
                    self.processor
                        .map_ram(0x4010_0000, 0x4000)
                        .map_err(|_| Error::PeripheralConflict)?,
                );
            }
            Peripheral::SamBackupRegisters => {
                self.processor
                    .map_iface(
                        0x400e_1890,
                        Rc::new(RefCell::new(peripherals::BackupRegisters::new(
                            self.accesses.clone(),
                        ))),
                    )
                    .map_err(|_| Error::PeripheralConflict)?;
            }
            Peripheral::SamUsbControl => {
                if self.sam_usb.is_some() {
                    return Err(Error::PeripheralConflict);
                }
                let state = Rc::new(RefCell::new(sam_usb::UsbState::new()));
                self.processor
                    .map_iface(
                        0x4003_8000,
                        Rc::new(RefCell::new(sam_usb::Registers(state.clone()))),
                    )
                    .map_err(|_| Error::PeripheralConflict)?;
                self.processor
                    .map_iface(
                        0xa010_0000,
                        Rc::new(RefCell::new(sam_usb::Fifo(state.clone()))),
                    )
                    .map_err(|_| Error::PeripheralConflict)?;
                self.sam_usb = Some(state);
            }
        }
        Ok(())
    }

    pub(crate) fn sam_receive(&mut self, data: &[u8], setup: bool) -> Result<(), Error> {
        self.sam_usb
            .as_ref()
            .ok_or(Error::MemoryAccess)?
            .borrow_mut()
            .receive(data, setup)
            .map_err(|_| Error::MemoryAccess)
    }

    pub(crate) fn sam_begin_in(&mut self) -> Result<bool, Error> {
        Ok(self
            .sam_usb
            .as_ref()
            .ok_or(Error::MemoryAccess)?
            .borrow_mut()
            .begin_in())
    }

    pub(crate) fn sam_packet(&mut self) -> Result<Option<Vec<u8>>, Error> {
        Ok(self
            .sam_usb
            .as_ref()
            .ok_or(Error::MemoryAccess)?
            .borrow_mut()
            .take_packet())
    }
}

fn validate_entry(base: u32, bytes: u32, entry: u32) -> Result<(), Error> {
    let offset = (entry & !1).checked_sub(base).ok_or(Error::EntryPoint)?;
    if entry & 1 == 0 || offset < 8 || offset > bytes - 2 {
        return Err(Error::EntryPoint);
    }
    Ok(())
}

struct BoundedDecoder {
    inner: Box<dyn InstructionDecode>,
    waiting: Rc<Cell<Option<StopWait>>>,
}

impl InstructionDecode for BoundedDecoder {
    fn try_decode(
        &self,
        ins: u32,
        size: InstructionSize,
        state: ItState,
    ) -> Result<Rc<dyn Instruction>, InstructionDecodeError> {
        let inner = self
            .inner
            .try_decode(ins, size, state)
            .map_err(|error| match error {
                InstructionDecodeError::Undefined => InstructionDecodeError::Unknown,
                error => error,
            })?;
        Ok(Rc::new(BoundedInstruction {
            inner,
            waiting: self.waiting.clone(),
            send_event: matches!(
                (size, ins),
                (InstructionSize::Ins16, 0xbf40) | (InstructionSize::Ins32, 0xf3af8004)
            ),
        }))
    }
}

struct BoundedInstruction {
    inner: Rc<dyn Instruction>,
    waiting: Rc<Cell<Option<StopWait>>>,
    send_event: bool,
}

impl Instruction for BoundedInstruction {
    fn patterns() -> &'static [armagnac::instructions::Pattern] {
        unreachable!("bounded instructions must be decoded through BoundedDecoder")
    }

    fn try_decode(
        _: armagnac::instructions::Encoding,
        _: u32,
        _: ItState,
    ) -> Result<Self, armagnac::decoder::DecodeError> {
        Err(armagnac::decoder::DecodeError::Other)
    }

    fn condition(&self) -> Option<Condition> {
        self.inner.condition()
    }

    fn execute(&self, processor: &mut Processor) -> Result<Effect, RunError> {
        if self.send_event {
            processor.registers.event = true;
            return Ok(Effect::None);
        }
        match self.inner.execute(processor)? {
            Effect::WaitForInterrupt => {
                self.waiting.set(Some(StopWait::Interrupt));
                Ok(Effect::Break(0))
            }
            Effect::WaitForEvent => {
                self.waiting.set(Some(StopWait::Event));
                Ok(Effect::Break(0))
            }
            effect => Ok(effect),
        }
    }

    fn name(&self) -> String {
        self.inner.name()
    }
    fn qualifier(&self) -> armagnac::instructions::Qualifier {
        self.inner.qualifier()
    }
    fn sets_flags(&self) -> bool {
        self.inner.sets_flags()
    }
    fn args(&self, pc: u32) -> String {
        self.inner.args(pc)
    }
}
