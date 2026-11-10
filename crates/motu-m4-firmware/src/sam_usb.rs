use armagnac::memory::{
    Env, MemoryAccessError, MemoryInterface, MemoryReadResult, MemoryWriteResult,
};
use std::cell::RefCell;
use std::rc::Rc;

pub(crate) struct UsbState {
    fifo: [u8; 64],
    written: usize,
    status: u32,
    mask: u32,
    packet: Option<Vec<u8>>,
}

impl UsbState {
    pub(crate) fn new() -> Self {
        Self {
            fifo: [0; 64],
            written: 0,
            status: 0,
            mask: 4,
            packet: None,
        }
    }

    pub(crate) fn receive(&mut self, data: &[u8], setup: bool) -> Result<(), MemoryAccessError> {
        if data.len() > 64 {
            return Err(MemoryAccessError::InvalidSize);
        }
        self.fifo.fill(0);
        self.fifo[..data.len()].copy_from_slice(data);
        self.written = 0;
        self.packet = None;
        let flags = if setup { 4 } else { 2 };
        let byte_count = (data.len() as u32) << 20;
        self.status = byte_count + flags;
        Ok(())
    }

    pub(crate) fn begin_in(&mut self) -> bool {
        if self.mask & 1 == 0 {
            return false;
        }
        self.status = 1;
        self.written = 0;
        self.packet = None;
        true
    }

    pub(crate) fn take_packet(&mut self) -> Option<Vec<u8>> {
        self.packet.take()
    }
}

pub(crate) struct Registers(pub(crate) Rc<RefCell<UsbState>>);

impl MemoryInterface for Registers {
    fn read_u32le(&mut self, address: u32, _: &mut Env) -> MemoryReadResult<u32> {
        match address {
            0x130 => Ok(self.0.borrow().status),
            0x1c0 => Ok(self.0.borrow().mask),
            _ => Err(MemoryAccessError::InvalidAddress),
        }
    }

    fn write_u32le(&mut self, address: u32, value: u32, _: &mut Env) -> MemoryWriteResult {
        if value & !0x7ff != 0 {
            return Err(MemoryAccessError::InvalidValue);
        }
        let mut state = self.0.borrow_mut();
        match address {
            0x160 => {
                if value & state.status & 1 != 0 {
                    state.packet = Some(state.fifo[..state.written].to_vec());
                }
                state.status &= !value;
            }
            0x190 => state.status |= value,
            0x1f0 => state.mask |= value,
            0x220 => state.mask &= !value,
            _ => return Err(MemoryAccessError::InvalidAddress),
        }
        Ok(())
    }

    fn size(&self) -> u32 {
        0x224
    }
}

pub(crate) struct Fifo(pub(crate) Rc<RefCell<UsbState>>);

impl MemoryInterface for Fifo {
    fn read_u8(&mut self, address: u32, _: &mut Env) -> MemoryReadResult<u8> {
        self.0
            .borrow()
            .fifo
            .get(address as usize)
            .copied()
            .ok_or(MemoryAccessError::InvalidAddress)
    }

    fn write_u8(&mut self, address: u32, value: u8, _: &mut Env) -> MemoryWriteResult {
        let mut state = self.0.borrow_mut();
        *state
            .fifo
            .get_mut(address as usize)
            .ok_or(MemoryAccessError::InvalidAddress)? = value;
        state.written = state.written.max(address as usize + 1);
        Ok(())
    }

    fn size(&self) -> u32 {
        64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_flags_have_separate_set_clear_and_mask_semantics() {
        let state = Rc::new(RefCell::new(UsbState::new()));
        let mut registers = Registers(state.clone());
        let mut env = Env::new(0, true);
        state.borrow_mut().receive(&[0; 8], true).unwrap();
        assert_eq!(registers.read_u32le(0x130, &mut env).unwrap(), 0x800004);
        assert_eq!(registers.read_u32le(0x1c0, &mut env).unwrap(), 4);
        assert!(!state.borrow_mut().begin_in());
        registers.write_u32le(0x1f0, 3, &mut env).unwrap();
        assert_eq!(registers.read_u32le(0x1c0, &mut env).unwrap(), 7);
        registers.write_u32le(0x1f0, 3, &mut env).unwrap();
        assert_eq!(registers.read_u32le(0x1c0, &mut env).unwrap(), 7);
        registers.write_u32le(0x160, 4, &mut env).unwrap();
        assert_eq!(state.borrow_mut().take_packet(), None);
        assert_eq!(registers.read_u32le(0x130, &mut env).unwrap(), 0x800000);
        registers.write_u32le(0x160, 1, &mut env).unwrap();
        assert_eq!(state.borrow_mut().take_packet(), None);
        assert_eq!(registers.read_u32le(0x130, &mut env).unwrap(), 0x800000);
        assert!(state.borrow_mut().begin_in());
        let mut fifo = Fifo(state.clone());
        for (offset, byte) in [3, 4, 5].into_iter().enumerate() {
            fifo.write_u8(offset as u32, byte, &mut env).unwrap();
        }
        registers.write_u32le(0x160, 1, &mut env).unwrap();
        assert_eq!(state.borrow_mut().take_packet(), Some(vec![3, 4, 5]));
        assert_eq!(state.borrow_mut().take_packet(), None);
        registers.write_u32le(0x220, 1, &mut env).unwrap();
        assert_eq!(registers.read_u32le(0x1c0, &mut env).unwrap(), 6);
        registers.write_u32le(0x220, 1, &mut env).unwrap();
        assert_eq!(registers.read_u32le(0x1c0, &mut env).unwrap(), 6);
        assert!(!state.borrow_mut().begin_in());
        registers.write_u32le(0x190, 2, &mut env).unwrap();
        assert_eq!(registers.read_u32le(0x130, &mut env).unwrap(), 2);
        registers.write_u32le(0x190, 2, &mut env).unwrap();
        assert_eq!(registers.read_u32le(0x130, &mut env).unwrap(), 2);
    }

    #[test]
    fn invalid_operations_leave_endpoint_state_unchanged() {
        let state = Rc::new(RefCell::new(UsbState::new()));
        let mut registers = Registers(state.clone());
        let mut fifo = Fifo(state.clone());
        let mut env = Env::new(0, true);
        assert!(state.borrow_mut().receive(&[0; 65], true).is_err());
        for address in [0, 0x131, 0x160, 0x224, u32::MAX] {
            assert!(registers.read_u32le(address, &mut env).is_err());
        }
        assert!(registers.write_u32le(0x130, 1, &mut env).is_err());
        assert!(registers.write_u32le(0x1f0, 0x80000000, &mut env).is_err());
        assert!(registers.read_u8(0x130, &mut env).is_err());
        assert!(registers.write_u16le(0x160, 1, &mut env).is_err());
        assert!(fifo.read_u8(64, &mut env).is_err());
        assert!(fifo.write_u8(64, 1, &mut env).is_err());
        assert_eq!(state.borrow().status, 0);
        assert_eq!(state.borrow().mask, 4);
        assert_eq!(state.borrow().written, 0);
        state.borrow_mut().receive(&[9; 64], false).unwrap();
        assert_eq!(fifo.read_u8(63, &mut env).unwrap(), 9);
        assert_eq!(state.borrow().status, 0x4000002);
    }
}
