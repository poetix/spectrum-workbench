//! The `.rkwdbg` debug information format, and source resolution over it.
//!
//! This is a contract between two programs. The assembler writes it; the
//! debugger reads it; and neither should have to depend on the other to agree
//! about what is in it — a debugger that linked the assembler in order to read
//! a text sidecar would be one that could not load a program the assembler did
//! not produce. So the format, its two indexes and the questions asked of it
//! live here, in a crate both sides depend on and neither owns (ADR-0019).
//!
//! The format itself is documented in `docs/debug-info.md`. It answers two
//! questions a raw binary cannot: *what source produced the instruction at this
//! address*, and *what addresses did this line of source produce*. The second
//! is one-to-many — a line inside a macro used five times produced five
//! addresses, and "set a breakpoint on this line" means all five.
//!
//! ```
//! use rkw_dbginfo::{DebugInfo, Sources};
//!
//! let info = DebugInfo::parse(concat!(
//!     "rkw-debug\t1\n",
//!     "file\t0\tsrc/main.asm\n",
//!     "line\t32768\t3\t0\t2\t9\t-\n",
//!     "symbol\tmain\t32768\tlabel\t0\t1\t1\n",
//! ))
//! .expect("well-formed");
//!
//! let mut sources = Sources::new(info);
//! sources.set_text_of("src/main.asm", "main:\n        ld hl,$1234\n");
//!
//! // A file spec need only be enough of the path to be unambiguous.
//! let site = sources.site("main.asm", 2).expect("resolves");
//! assert_eq!(site.addresses, [0x8000]);
//! assert_eq!(sources.address_of("main"), Ok(0x8000));
//!
//! let here = sources.locate(0x8001).expect("inside the instruction");
//! assert_eq!(here.line, 2);
//! assert_eq!(here.text.as_deref(), Some("        ld hl,$1234"));
//! ```

pub mod info;
pub mod source;

pub use info::{DebugInfo, Expansion, Kind, Line, Position, Symbol, VERSION};
pub use source::{Frame, Listing, Located, ResolveError, Site, Sources};
