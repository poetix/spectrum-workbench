//! The watchpoint check, in the one place every memory access passes through.
//!
//! The CPU is generic over [`Bus`], so watching memory does not need the core
//! to know anything about debugging: the debugger wraps the real bus in one of
//! these for the duration of an instruction and the core is monomorphised
//! against it.
//!
//! Every method forwards to the inner bus, including the machine-cycle
//! wrappers that have default bodies. That is deliberate and load-bearing: a
//! contended Spectrum bus implements its timing by overriding `read_cycle` and
//! friends, and a decorator that only overrode the raw `read` would silently
//! route around it, taking the contention with it.

use z80::Bus;
use z80::disasm::Peek;

use crate::StopReason;
use crate::breakpoints::{Access, Breakpoints, PortAccess};

pub(crate) struct DebugBus<'a, B: Bus + Peek> {
    inner: &'a mut B,
    breaks: &'a mut Breakpoints,
    /// The first watchpoint to fire during this instruction.
    ///
    /// An instruction can touch several watched addresses — `LDIR` touches two
    /// per iteration — and stopping is a thing that happens between
    /// instructions, so the first hit is the one reported and the rest of the
    /// instruction still runs to completion. Stopping half way through would
    /// leave the machine in a state the Z80 has no way to represent.
    hit: Option<StopReason>,
}

impl<'a, B: Bus + Peek> DebugBus<'a, B> {
    pub(crate) fn new(inner: &'a mut B, breaks: &'a mut Breakpoints) -> Self {
        DebugBus {
            inner,
            breaks,
            hit: None,
        }
    }

    /// The hit, if there was one, and reset for the next instruction.
    pub(crate) fn take_hit(&mut self) -> Option<StopReason> {
        self.hit.take()
    }

    /// The two things the between-instruction check needs, handed out
    /// together because they are both borrowed from here.
    pub(crate) fn split(&mut self) -> (&mut Breakpoints, &B) {
        (self.breaks, self.inner)
    }

    fn record(&mut self, reason: StopReason) {
        if self.hit.is_none() {
            self.hit = Some(reason);
        }
    }

    fn watch_read(&mut self, addr: u16, value: u8) {
        if !self.breaks.read_maybe(addr) {
            return;
        }
        if let Some(id) = self.breaks.fired_watch(addr, Access::Read, value, value) {
            self.record(StopReason::Watchpoint {
                id,
                addr,
                access: Access::Read,
                old: value,
                new: value,
            });
        }
    }

    fn watch_port(&mut self, port: u16, access: PortAccess, value: u8) {
        if !self.breaks.port_maybe(port) {
            return;
        }
        if let Some(id) = self.breaks.fired_port(port, access) {
            self.record(StopReason::PortWatchpoint {
                id,
                port,
                access,
                value,
            });
        }
    }
}

impl<B: Bus + Peek> Bus for DebugBus<'_, B> {
    fn read(&mut self, addr: u16) -> u8 {
        self.inner.read(addr)
    }

    fn write(&mut self, addr: u16, value: u8) {
        self.inner.write(addr, value);
    }

    fn input(&mut self, port: u16) -> u8 {
        self.inner.input(port)
    }

    fn output(&mut self, port: u16, value: u8) {
        self.inner.output(port, value);
    }

    fn tick(&mut self, t: u32) {
        self.inner.tick(t);
    }

    fn tick_at(&mut self, addr: u16, t: u32) {
        self.inner.tick_at(addr, t);
    }

    /// Not a watched read. An instruction fetch is execution, and execution is
    /// what breakpoints are for; a read watchpoint that fired on the bytes of
    /// the instruction sitting at the watched address would be useless.
    fn fetch_opcode(&mut self, addr: u16) -> u8 {
        self.inner.fetch_opcode(addr)
    }

    fn read_cycle(&mut self, addr: u16) -> u8 {
        let v = self.inner.read_cycle(addr);
        self.watch_read(addr, v);
        v
    }

    fn write_cycle(&mut self, addr: u16, value: u8) {
        if !self.breaks.write_maybe(addr) {
            self.inner.write_cycle(addr, value);
            return;
        }
        // The old byte is read through `Peek`, not through the bus, so that
        // looking is guaranteed not to be an access the machine can notice.
        let old = self.inner.peek(addr);
        self.inner.write_cycle(addr, value);
        if let Some(id) = self.breaks.fired_watch(addr, Access::Write, old, value) {
            self.record(StopReason::Watchpoint {
                id,
                addr,
                access: Access::Write,
                old,
                new: value,
            });
        }
    }

    fn input_cycle(&mut self, port: u16) -> u8 {
        let v = self.inner.input_cycle(port);
        self.watch_port(port, PortAccess::In, v);
        v
    }

    fn output_cycle(&mut self, port: u16, value: u8) {
        self.inner.output_cycle(port, value);
        self.watch_port(port, PortAccess::Out, value);
    }

    fn interrupt_data(&mut self) -> u8 {
        self.inner.interrupt_data()
    }

    fn interrupt_pending(&self) -> bool {
        self.inner.interrupt_pending()
    }

    fn nmi_pending(&mut self) -> bool {
        self.inner.nmi_pending()
    }
}
