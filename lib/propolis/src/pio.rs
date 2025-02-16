use std::sync::{Arc, Mutex};

use crate::common::*;
use crate::util::aspace::ASpace;
pub use crate::util::aspace::{Error, Result};

use byteorder::{ByteOrder, LE};

#[usdt::provider(provider = "propolis")]
mod probes {
    fn pio_in(port: u16, bytes: u8, value: u32, was_handled: u8) {}
    fn pio_out(port: u16, bytes: u8, value: u32, was_handled: u8) {}
}

pub type PioFn = dyn Fn(u16, RWOp<'_, '_>) + Send + Sync + 'static;

/// Port IO bus.
pub struct PioBus {
    map: Mutex<ASpace<Arc<PioFn>>>,
}

impl PioBus {
    pub fn new() -> Self {
        Self { map: Mutex::new(ASpace::new(0, u16::MAX as usize)) }
    }

    pub fn register(
        &self,
        start: u16,
        len: u16,
        func: Arc<PioFn>,
    ) -> Result<()> {
        self.map.lock().unwrap().register(start as usize, len as usize, func)
    }
    pub fn unregister(&self, start: u16) -> Result<()> {
        self.map.lock().unwrap().unregister(start as usize).map(|_| ())
    }

    pub fn handle_out(&self, port: u16, bytes: u8, val: u32) -> Result<()> {
        let buf = val.to_le_bytes();
        let data = match bytes {
            1 => &buf[0..1],
            2 => &buf[0..2],
            4 => &buf[0..],
            _ => panic!(),
        };
        let handled = self.do_pio(port, |a, o, func| {
            let mut wo = WriteOp::from_buf(o as usize, data);
            func(a, RWOp::Write(&mut wo))
        });
        probes::pio_out!(|| (port, bytes, val, handled.is_ok() as u8));
        handled
    }

    pub fn handle_in(&self, port: u16, bytes: u8) -> Result<u32> {
        let mut buf = [0xffu8; 4];
        let mut data = match bytes {
            1 => &mut buf[0..1],
            2 => &mut buf[0..2],
            4 => &mut buf[0..],
            _ => panic!(),
        };
        let handled = self.do_pio(port, |a, o, func| {
            let mut ro = ReadOp::from_buf(o as usize, &mut data);
            func(a, RWOp::Read(&mut ro))
        });

        let val = LE::read_u32(&buf);
        probes::pio_in!(|| (port, bytes, val, handled.is_ok() as u8));
        handled.map(|_| val)
    }

    fn do_pio<F>(&self, port: u16, f: F) -> Result<()>
    where
        F: FnOnce(u16, u16, &Arc<PioFn>),
    {
        let map = self.map.lock().unwrap();
        let (start, _len, func) = map.region_at(port as usize)?;
        let func = Arc::clone(func);
        // unlock map before entering handler
        drop(map);
        f(start as u16, port - start as u16, &func);
        Ok(())
    }

    pub(crate) fn clear(&self) {
        let mut map = self.map.lock().unwrap();
        map.clear();
    }
}
