use armagnac::memory::{
    Env, MemoryAccessError, MemoryInterface, MemoryReadResult, MemoryWriteResult,
};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

#[derive(Debug, Clone, Copy)]
pub enum Peripheral {
    NxpUsbSram,
    SamBackupRegisters,
    SamUsbControl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Access {
    pub instruction: u64,
    pub address: u32,
    pub value: u32,
    pub write: bool,
}

pub(crate) struct BackupRegisters {
    values: [u32; 8],
    pub(crate) accesses: Rc<RefCell<VecDeque<Access>>>,
}

impl BackupRegisters {
    pub(crate) fn new(accesses: Rc<RefCell<VecDeque<Access>>>) -> Self {
        Self {
            values: [0; 8],
            accesses,
        }
    }

    fn index(address: u32) -> Result<usize, MemoryAccessError> {
        if address & 3 != 0 {
            return Err(MemoryAccessError::InvalidAlignment);
        }
        if address >= 32 {
            return Err(MemoryAccessError::InvalidAddress);
        }
        Ok((address / 4) as usize)
    }

    fn record(&self, address: u32, value: u32, write: bool, env: &Env) {
        let mut accesses = self.accesses.borrow_mut();
        if accesses.len() == 64 {
            accesses.pop_front();
        }
        accesses.push_back(Access {
            instruction: env.cycles,
            address: 0x400e_1890 + address,
            value,
            write,
        });
    }
}

impl MemoryInterface for BackupRegisters {
    fn read_u32le(&mut self, address: u32, env: &mut Env) -> MemoryReadResult<u32> {
        let value = self.values[Self::index(address)?];
        self.record(address, value, false, env);
        Ok(value)
    }

    fn write_u32le(&mut self, address: u32, value: u32, env: &mut Env) -> MemoryWriteResult {
        self.values[Self::index(address)?] = value;
        self.record(address, value, true, env);
        Ok(())
    }

    fn size(&self) -> u32 {
        32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_registers_do_not_alias_and_diagnostics_have_a_fixed_capacity() {
        let accesses = Rc::new(RefCell::new(VecDeque::new()));
        let mut registers = BackupRegisters::new(accesses.clone());
        let mut env = Env::new(0, true);
        assert_eq!(registers.size(), 32);
        for index in 0..8 {
            registers
                .write_u32le(index * 4, 0x12340000 + index, &mut env)
                .unwrap();
        }
        for index in 0..8 {
            assert_eq!(
                registers.read_u32le(index * 4, &mut env).unwrap(),
                0x12340000 + index
            );
        }
        for index in 0..8 {
            let accesses = accesses.borrow();
            assert_eq!(accesses[index].address, 0x400e1890 + index as u32 * 4);
            assert_eq!(accesses[index].value, 0x12340000 + index as u32);
            assert!(accesses[index].write);
            assert_eq!(accesses[index + 8].address, 0x400e1890 + index as u32 * 4);
            assert!(!accesses[index + 8].write);
        }
        assert_eq!(
            registers.read_u32le(1, &mut env),
            Err(MemoryAccessError::InvalidAlignment)
        );
        assert_eq!(
            registers.read_u32le(32, &mut env),
            Err(MemoryAccessError::InvalidAddress)
        );
        assert_eq!(
            registers.write_u32le(33, 0, &mut env),
            Err(MemoryAccessError::InvalidAlignment)
        );
        assert!(registers.read_u16le(0, &mut env).is_err());
        accesses.borrow_mut().clear();
        for instruction in 0..65 {
            env.cycles = instruction;
            registers.read_u32le(0, &mut env).unwrap();
        }
        let accesses = accesses.borrow();
        assert_eq!(accesses.len(), 64);
        assert_eq!(accesses.front().unwrap().instruction, 1);
        assert_eq!(accesses.back().unwrap().instruction, 64);
    }
}
