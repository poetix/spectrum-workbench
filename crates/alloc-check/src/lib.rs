//! Counting allocations, so that "this does not allocate" can be a test rather
//! than a comment.
//!
//! ADR-0007 puts a hard rule on the emulation thread: nothing there allocates.
//! A rule that is only written down drifts, and the symptom — a `Vec` grown
//! once per instruction — is invisible until it is eating the headroom the
//! whole design is built around. So the rule gets an assertion, and the
//! assertion needs an allocator that counts.
//!
//! The count is per thread. A process-wide counter would be measuring the test
//! harness's other threads as much as the code under test, and would fail
//! intermittently for reasons having nothing to do with the code.
//!
//! Install it in a test binary and measure a closure:
//!
//! ```
//! #[global_allocator]
//! static ALLOC: alloc_check::Counting = alloc_check::Counting;
//!
//! let (sum, allocations) = alloc_check::count(|| (1..=10).sum::<u32>());
//! assert_eq!(sum, 55);
//! assert_eq!(allocations, 0);
//!
//! let (_, allocations) = alloc_check::count(|| String::from("this one does"));
//! assert!(allocations > 0, "the allocator is not installed");
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    // `const` initialisation: a lazily initialised thread-local would allocate
    // on first touch, from inside the allocator.
    static COUNT: Cell<usize> = const { Cell::new(0) };
}

fn bump() {
    // `try_with`, not `with`: a thread that allocates while tearing down has
    // no thread-locals left, and panicking inside `malloc` helps nobody.
    let _ = COUNT.try_with(|n| n.set(n.get() + 1));
}

/// The system allocator, counting every allocation the calling thread makes.
///
/// Deallocation is not counted: what matters here is whether a code path
/// touches the heap at all.
pub struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // A `Vec` growing is exactly the thing being looked for, so a
        // reallocation counts as an allocation.
        bump();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// Run `f`, returning its value and how many allocations it made on this
/// thread.
///
/// Meaningless unless [`Counting`] is the binary's `#[global_allocator]`; a
/// test should assert that something known to allocate does, so that an
/// uninstalled allocator fails loudly rather than passing everything.
pub fn count<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = COUNT.with(Cell::get);
    let out = f();
    let after = COUNT.with(Cell::get);
    (out, after - before)
}
