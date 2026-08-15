//! Breakpoint and watchpoint storage, in the three tiers of ADR-0008.
//!
//! Every instruction asks "is there a breakpoint here?", and every memory
//! access asks it about three more addresses. At ~6 ns per instruction a hash
//! probe would cost more than the instruction, and would cost it whether or
//! not anything is set — penalising the ordinary case of just running the
//! machine. So the question is asked in three stages, each only reached if the
//! last said maybe:
//!
//! 1. **Is anything armed at all?** A `bool`. Free when the answer is no,
//!    because the branch predicts perfectly.
//! 2. **Is *this* address armed?** One bit test in an 8 KB bitmap.
//! 3. **What is armed there, and does it fire?** A `HashMap` probe, reached
//!    only when the bitmap said yes.
//!
//! Conditions, hit counts and ignore counts all live in tier 3, so they cost
//! nothing until a breakpoint actually hits. The price is that the same fact
//! is recorded in three places and they have to be kept in step; that is what
//! [`Breakpoints::rearm_exec`] and its siblings are for, and every mutation
//! goes through them.

use std::collections::HashMap;

use z80::Regs;
use z80::disasm::Peek;

use crate::bitmap::Bitmap;
use crate::condition::Condition;

pub type Id = u32;

/// Which side of a memory access a watchpoint is watching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

/// Which direction of an I/O access a port watchpoint is watching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortAccess {
    In,
    Out,
}

/// An execution breakpoint: tier 3 for one address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Breakpoint {
    pub id: Id,
    pub addr: u16,
    pub enabled: bool,
    /// Checked before the hit is counted, so a conditional breakpoint's hit
    /// count is the number of times it *fired*, not the number of times
    /// execution passed the address.
    pub condition: Option<Condition>,
    pub hits: u64,
    /// Pass this many qualifying hits before stopping, decrementing each time.
    /// gdb's `ignore`.
    pub ignore: u32,
}

/// A memory watchpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watchpoint {
    pub id: Id,
    pub addr: u16,
    pub enabled: bool,
    pub on_read: bool,
    pub on_write: bool,
    /// Stop on a write only when it changes the byte. The usual reason to
    /// watch an address is that something is corrupting it, and a routine that
    /// rewrites the same value every frame is noise.
    pub on_change_only: bool,
    pub hits: u64,
}

/// A port watchpoint, matching `port & mask == value`.
///
/// Spectrum ports are partially decoded — the ULA answers to every even port —
/// so watching "port 0xFE" means watching 32768 addresses. The mask is how
/// that is said once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortWatch {
    pub id: Id,
    pub mask: u16,
    pub value: u16,
    pub enabled: bool,
    pub on_in: bool,
    pub on_out: bool,
    pub hits: u64,
}

/// A breakpoint the debugger set for itself: the landing site of a step-over,
/// step-out or run-to-cursor.
///
/// Kept apart from the user's breakpoints rather than sharing the map, so that
/// stepping over an address that already has a breakpoint on it cannot disturb
/// it, and so that "delete all breakpoints" has nothing to say about an
/// in-flight step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Temporary {
    pub addr: u16,
    /// Only fire when `SP` is at least this. A recursive call reaching the
    /// same return address has a lower `SP`, and stepping over one call should
    /// not stop inside the next one down.
    pub sp_at_least: Option<u16>,
}

/// Everything armed, and the tiers that make asking cheap.
pub struct Breakpoints {
    exec: HashMap<u16, Breakpoint>,
    exec_bits: Bitmap,
    exec_armed: bool,
    pub(crate) temporaries: Vec<Temporary>,

    watches: HashMap<u16, Watchpoint>,
    read_bits: Bitmap,
    write_bits: Bitmap,
    mem_armed: bool,

    ports: Vec<PortWatch>,
    port_bits: Bitmap,
    port_armed: bool,

    next_id: Id,
    probes: u64,
}

impl Default for Breakpoints {
    fn default() -> Self {
        Self::new()
    }
}

impl Breakpoints {
    pub fn new() -> Self {
        Breakpoints {
            exec: HashMap::new(),
            exec_bits: Bitmap::new(),
            exec_armed: false,
            temporaries: Vec::new(),
            watches: HashMap::new(),
            read_bits: Bitmap::new(),
            write_bits: Bitmap::new(),
            mem_armed: false,
            ports: Vec::new(),
            port_bits: Bitmap::new(),
            port_armed: false,
            next_id: 1,
            probes: 0,
        }
    }

    fn take_id(&mut self) -> Id {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    // ---- Tier 1 and 2: the hot path ---------------------------------------

    /// Tier 1 for execution: is anything armed at all?
    #[inline]
    pub fn exec_armed(&self) -> bool {
        self.exec_armed
    }

    /// Tiers 1 and 2 together, which is the whole per-instruction cost when
    /// nothing fires.
    #[inline]
    pub fn exec_maybe(&self, pc: u16) -> bool {
        self.exec_armed && self.exec_bits.test(pc)
    }

    /// Tier 1 for the bus: is anything watching memory or ports?
    ///
    /// Asked once per run rather than once per access, because the answer
    /// decides whether the bus is wrapped at all — and wrapping it costs
    /// considerably more than the check it carries. See the throughput test.
    #[inline]
    pub fn bus_armed(&self) -> bool {
        self.mem_armed || self.port_armed
    }

    #[inline]
    pub fn read_maybe(&self, addr: u16) -> bool {
        self.mem_armed && self.read_bits.test(addr)
    }

    #[inline]
    pub fn write_maybe(&self, addr: u16) -> bool {
        self.mem_armed && self.write_bits.test(addr)
    }

    #[inline]
    pub fn port_maybe(&self, port: u16) -> bool {
        self.port_armed && self.port_bits.test(port)
    }

    /// How many times tier 3 has been reached. Zero while nothing is hit, and
    /// asserted on in the throughput test: "armed but not hit" must not be
    /// paying for a hash probe.
    pub fn detail_probes(&self) -> u64 {
        self.probes
    }

    // ---- Execution breakpoints --------------------------------------------

    /// Arm an address, or return the existing breakpoint's id if it is already
    /// armed. One breakpoint per address: two breakpoints on one address that
    /// differ only in condition are a confusion to display and to delete, and
    /// the conditions can be combined with [`Condition::Any`].
    ///
    /// [`Condition::Any`]: crate::condition::Condition::Any
    pub fn add_exec(&mut self, addr: u16) -> Id {
        if let Some(bp) = self.exec.get(&addr) {
            return bp.id;
        }
        let id = self.take_id();
        self.exec.insert(
            addr,
            Breakpoint {
                id,
                addr,
                enabled: true,
                condition: None,
                hits: 0,
                ignore: 0,
            },
        );
        self.rearm_exec(addr);
        id
    }

    pub fn breakpoint(&self, addr: u16) -> Option<&Breakpoint> {
        self.exec.get(&addr)
    }

    pub fn breakpoint_by_id(&self, id: Id) -> Option<&Breakpoint> {
        self.exec.values().find(|b| b.id == id)
    }

    /// Change a breakpoint by id. The closure sees it mutably; the bitmap is
    /// brought back into step afterwards, which is why this is not a plain
    /// `&mut` accessor.
    pub fn edit(&mut self, id: Id, f: impl FnOnce(&mut Breakpoint)) -> bool {
        let Some(addr) = self.exec.values().find(|b| b.id == id).map(|b| b.addr) else {
            return false;
        };
        if let Some(bp) = self.exec.get_mut(&addr) {
            f(bp);
        }
        self.rearm_exec(addr);
        true
    }

    pub fn set_condition(&mut self, id: Id, condition: Option<Condition>) -> bool {
        self.edit(id, |bp| bp.condition = condition)
    }

    pub fn set_ignore(&mut self, id: Id, ignore: u32) -> bool {
        self.edit(id, |bp| bp.ignore = ignore)
    }

    pub fn set_enabled(&mut self, id: Id, enabled: bool) -> bool {
        self.edit(id, |bp| bp.enabled = enabled)
    }

    /// Remove a breakpoint, watchpoint or port watchpoint by id.
    pub fn remove(&mut self, id: Id) -> bool {
        if let Some(addr) = self.exec.values().find(|b| b.id == id).map(|b| b.addr) {
            self.exec.remove(&addr);
            self.rearm_exec(addr);
            return true;
        }
        if let Some(addr) = self.watches.values().find(|w| w.id == id).map(|w| w.addr) {
            self.watches.remove(&addr);
            self.rearm_watch(addr);
            return true;
        }
        if let Some(i) = self.ports.iter().position(|p| p.id == id) {
            self.ports.remove(i);
            self.rearm_ports();
            return true;
        }
        false
    }

    /// Everything the user set, in address order. Temporaries are the
    /// debugger's own and are not listed.
    pub fn breakpoints(&self) -> Vec<&Breakpoint> {
        let mut v: Vec<&Breakpoint> = self.exec.values().collect();
        v.sort_by_key(|b| b.addr);
        v
    }

    pub fn watchpoints(&self) -> Vec<&Watchpoint> {
        let mut v: Vec<&Watchpoint> = self.watches.values().collect();
        v.sort_by_key(|w| w.addr);
        v
    }

    pub fn port_watchpoints(&self) -> &[PortWatch] {
        &self.ports
    }

    pub fn clear(&mut self) {
        self.exec.clear();
        self.watches.clear();
        self.ports.clear();
        self.exec_bits.clear();
        self.read_bits.clear();
        self.write_bits.clear();
        self.port_bits.clear();
        self.exec_armed = !self.temporaries.is_empty();
        self.mem_armed = false;
        self.port_armed = false;
        for t in &self.temporaries {
            self.exec_bits.set(t.addr, true);
        }
    }

    /// Tier 3 for execution. Called only when [`Breakpoints::exec_maybe`] has
    /// already said yes.
    ///
    /// Returns the breakpoint that fired, if one did: the bitmap can say yes
    /// for a breakpoint whose condition is false, whose ignore count has not
    /// run out, or that is disabled.
    pub(crate) fn fired_exec<P: Peek>(&mut self, pc: u16, regs: &Regs, mem: &P) -> Option<Id> {
        self.probes += 1;
        let bp = self.exec.get_mut(&pc)?;
        if !bp.enabled {
            return None;
        }
        match &bp.condition {
            Some(c) if !c.eval(regs, mem) => return None,
            _ => {}
        }
        bp.hits += 1;
        if bp.ignore > 0 {
            bp.ignore -= 1;
            return None;
        }
        Some(bp.id)
    }

    /// True if a temporary breakpoint at `pc` should fire, given the current
    /// `SP`.
    pub(crate) fn fired_temporary(&self, pc: u16, sp: u16) -> bool {
        self.temporaries
            .iter()
            .any(|t| t.addr == pc && t.sp_at_least.is_none_or(|floor| sp >= floor))
    }

    pub(crate) fn add_temporary(&mut self, t: Temporary) {
        self.temporaries.push(t);
        self.exec_bits.set(t.addr, true);
        self.exec_armed = true;
    }

    /// Popping one at a time rather than clearing and then repairing: the
    /// repair reads the list it is repairing from, and the common case — there
    /// are none — has to touch nothing at all, because it is on the path every
    /// resume takes.
    pub(crate) fn clear_temporaries(&mut self) {
        while let Some(t) = self.temporaries.pop() {
            self.rearm_exec(t.addr);
        }
    }

    /// Recompute one address's bit and the armed flag from the two things that
    /// can arm it. Every mutation ends here; nothing sets a bit directly.
    fn rearm_exec(&mut self, addr: u16) {
        let armed = self.exec.get(&addr).is_some_and(|b| b.enabled)
            || self.temporaries.iter().any(|t| t.addr == addr);
        self.exec_bits.set(addr, armed);
        self.exec_armed = self.exec.values().any(|b| b.enabled) || !self.temporaries.is_empty();
    }

    // ---- Memory watchpoints -----------------------------------------------

    /// Watch an address for reads, writes or both. Watching an address already
    /// watched updates it in place and keeps its id.
    pub fn watch_mem(&mut self, addr: u16, on_read: bool, on_write: bool) -> Id {
        let id = match self.watches.get_mut(&addr) {
            Some(w) => {
                w.on_read = on_read;
                w.on_write = on_write;
                w.enabled = true;
                w.id
            }
            None => {
                let id = self.take_id();
                self.watches.insert(
                    addr,
                    Watchpoint {
                        id,
                        addr,
                        enabled: true,
                        on_read,
                        on_write,
                        on_change_only: false,
                        hits: 0,
                    },
                );
                id
            }
        };
        self.rearm_watch(addr);
        id
    }

    pub fn watchpoint(&self, addr: u16) -> Option<&Watchpoint> {
        self.watches.get(&addr)
    }

    pub fn edit_watch(&mut self, id: Id, f: impl FnOnce(&mut Watchpoint)) -> bool {
        let Some(addr) = self.watches.values().find(|w| w.id == id).map(|w| w.addr) else {
            return false;
        };
        if let Some(w) = self.watches.get_mut(&addr) {
            f(w);
        }
        self.rearm_watch(addr);
        true
    }

    fn rearm_watch(&mut self, addr: u16) {
        let w = self.watches.get(&addr);
        self.read_bits
            .set(addr, w.is_some_and(|w| w.enabled && w.on_read));
        self.write_bits
            .set(addr, w.is_some_and(|w| w.enabled && w.on_write));
        self.mem_armed = self
            .watches
            .values()
            .any(|w| w.enabled && (w.on_read || w.on_write));
    }

    /// Tier 3 for a memory access.
    pub(crate) fn fired_watch(
        &mut self,
        addr: u16,
        access: Access,
        old: u8,
        new: u8,
    ) -> Option<Id> {
        self.probes += 1;
        let w = self.watches.get_mut(&addr)?;
        if !w.enabled {
            return None;
        }
        match access {
            Access::Read if !w.on_read => return None,
            Access::Write if !w.on_write => return None,
            _ => {}
        }
        if access == Access::Write && w.on_change_only && old == new {
            return None;
        }
        w.hits += 1;
        Some(w.id)
    }

    // ---- Port watchpoints -------------------------------------------------

    /// Watch every port matching `port & mask == value`.
    ///
    /// The bitmap is filled by walking all 65536 ports once, here, at command
    /// rate — which is the same trade the address bitmaps make, moving work
    /// off the access path and onto the rare mutation.
    pub fn watch_port(&mut self, mask: u16, value: u16, on_in: bool, on_out: bool) -> Id {
        let id = self.take_id();
        self.ports.push(PortWatch {
            id,
            mask,
            value,
            enabled: true,
            on_in,
            on_out,
            hits: 0,
        });
        self.rearm_ports();
        id
    }

    pub fn edit_port_watch(&mut self, id: Id, f: impl FnOnce(&mut PortWatch)) -> bool {
        let Some(p) = self.ports.iter_mut().find(|p| p.id == id) else {
            return false;
        };
        f(p);
        self.rearm_ports();
        true
    }

    fn rearm_ports(&mut self) {
        let live: Vec<(u16, u16)> = self
            .ports
            .iter()
            .filter(|p| p.enabled && (p.on_in || p.on_out))
            .map(|p| (p.mask, p.value))
            .collect();
        self.port_bits.clear();
        self.port_armed = !live.is_empty();
        for (mask, value) in live {
            for port in 0..=0xFFFFu16 {
                if port & mask == value {
                    self.port_bits.set(port, true);
                }
            }
        }
    }

    /// Tier 3 for an I/O access. Port watches are a short list rather than a
    /// map, because a masked watch covers tens of thousands of ports and there
    /// are never many of them.
    pub(crate) fn fired_port(&mut self, port: u16, access: PortAccess) -> Option<Id> {
        self.probes += 1;
        for p in &mut self.ports {
            if !p.enabled || port & p.mask != p.value {
                continue;
            }
            let wanted = match access {
                PortAccess::In => p.on_in,
                PortAccess::Out => p.on_out,
            };
            if !wanted {
                continue;
            }
            p.hits += 1;
            return Some(p.id);
        }
        None
    }

    // ---- Introspection, for tests and for `info` --------------------------

    /// How many addresses are armed for execution, counted from the bitmap
    /// rather than the map — so a test can catch the two disagreeing.
    pub fn armed_exec_addresses(&self) -> u32 {
        self.exec_bits.count()
    }

    pub fn armed_read_addresses(&self) -> u32 {
        self.read_bits.count()
    }

    pub fn armed_write_addresses(&self) -> u32 {
        self.write_bits.count()
    }

    pub fn armed_ports(&self) -> u32 {
        self.port_bits.count()
    }
}
