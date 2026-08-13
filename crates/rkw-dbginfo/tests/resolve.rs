//! Resolution: file specs, lines to addresses, symbols, listings, staleness.

use std::time::{Duration, SystemTime};

use rkw_dbginfo::{DebugInfo, ResolveError, Sources};

/// Two files, a macro expanded twice, and a line in the middle that produced
/// nothing.
fn info() -> DebugInfo {
    DebugInfo::parse(concat!(
        "rkw-debug\t1\n",
        "file\t0\tsrc/main.asm\n",
        "file\t1\tlib/main.asm\n",
        "file\t2\tsrc/plot.asm\n",
        // `plot` invoked at main.asm:8 and 9, defined at plot.asm:2.
        "expansion\t0\tplot\t0\t8\t9\t2\t2\t1\t-\n",
        "expansion\t1\tplot\t0\t9\t9\t2\t2\t1\t-\n",
        "line\t32768\t3\t0\t4\t9\t-\n",
        "line\t32771\t1\t2\t2\t9\t0\n",
        "line\t32772\t1\t2\t2\t9\t1\n",
        "line\t32773\t1\t0\t12\t9\t-\n",
        "symbol\tmain\t32768\tlabel\t0\t4\t1\n",
        "symbol\twidth\t70000\tconstant\t0\t2\t1\n",
    ))
    .expect("well-formed")
}

fn sources() -> Sources {
    let mut sources = Sources::new(info());
    sources.set_text_of(
        "src/main.asm",
        "; a header\nwidth   equ 70000\n\nmain:   ld hl,$1234\n\n\n\n        plot 1\n        plot 2\n\n\n        ret\n",
    );
    sources.set_text_of(
        "src/plot.asm",
        "plot    macro n\n        db n\n        endm\n",
    );
    sources
}

#[test]
fn a_file_spec_need_only_be_enough_of_the_path_to_be_unambiguous() {
    let sources = sources();
    assert_eq!(sources.file_matching("src/main.asm"), Ok(0));
    assert_eq!(sources.file_matching("plot.asm"), Ok(2));

    // `main.asm` is the tail of two of them, and picking one would be wrong
    // half the time without saying so.
    let error = sources.file_matching("main.asm").expect_err("ambiguous");
    assert_eq!(
        error,
        ResolveError::AmbiguousFile {
            spec: "main.asm".into(),
            matches: vec!["src/main.asm".into(), "lib/main.asm".into()],
        }
    );
    assert_eq!(
        sources.file_matching("nowhere.asm"),
        Err(ResolveError::UnknownFile("nowhere.asm".into()))
    );

    // A suffix is whole path components, not any old substring: `ain.asm` is
    // the tail of the text and not of the path.
    assert_eq!(
        sources.file_matching("ain.asm"),
        Err(ResolveError::UnknownFile("ain.asm".into()))
    );
}

#[test]
fn a_line_inside_a_macro_resolves_to_every_address_it_produced() {
    let sources = sources();
    let site = sources.site("plot.asm", 2).expect("resolves");
    assert_eq!(site.addresses, [0x8003, 0x8004]);
    assert_eq!(site.line, 2);
    assert!(!site.moved());
}

#[test]
fn a_line_with_no_code_moves_on_to_the_next_line_that_has_some() {
    let sources = sources();
    // Line 5 of main.asm is blank; the next line that produced anything is 12.
    let site = sources.site("src/main.asm", 5).expect("resolves");
    assert_eq!((site.requested, site.line), (5, 12));
    assert_eq!(site.addresses, [0x8005]);
    assert!(site.moved());

    // Past the last line that produced code there is nothing to move on to.
    assert_eq!(
        sources.site("src/main.asm", 13),
        Err(ResolveError::NoCode {
            file: "src/main.asm".into(),
            line: 13
        })
    );
}

#[test]
fn a_symbol_resolves_to_an_address_unless_it_is_not_one() {
    let sources = sources();
    assert_eq!(sources.address_of("main"), Ok(0x8000));
    assert_eq!(
        sources.address_of("width"),
        Err(ResolveError::NotAnAddress {
            name: "width".into(),
            value: 70000
        })
    );
    assert_eq!(
        sources.address_of("absent"),
        Err(ResolveError::UnknownSymbol("absent".into()))
    );
}

#[test]
fn an_address_inside_an_expansion_carries_the_invocation_that_led_there() {
    let sources = sources();
    let here = sources.locate(0x8004).expect("covered");
    assert_eq!((here.file.as_str(), here.line), ("src/plot.asm", 2));
    assert_eq!(here.text.as_deref(), Some("        db n"));

    // The second invocation, not the first: which expansion an address came
    // from is the whole reason the record carries an index.
    assert_eq!(here.frames.len(), 1);
    assert_eq!(here.frames[0].name, "plot");
    assert_eq!(
        (here.frames[0].file.as_str(), here.frames[0].line),
        ("src/main.asm", 9)
    );

    assert!(sources.locate(0x9000).is_none());
}

#[test]
fn a_line_whose_file_was_never_read_still_resolves_to_addresses() {
    // Text is a convenience for showing; addresses do not depend on it.
    let sources = Sources::new(info());
    assert_eq!(
        sources
            .site("plot.asm", 2)
            .expect("resolves")
            .addresses
            .len(),
        2
    );
    assert_eq!(sources.locate(0x8000).expect("covered").text, None);
    assert_eq!(
        sources.listing("src/main.asm", 4, 2),
        Err(ResolveError::NoText("src/main.asm".into()))
    );
}

#[test]
fn a_listing_is_a_window_clamped_to_the_file() {
    let sources = sources();
    let listing = sources.listing("src/main.asm", 4, 2).expect("has text");
    assert_eq!(listing.first, 2);
    assert_eq!(listing.current, Some(4));
    assert_eq!(listing.lines.len(), 5);
    assert_eq!(listing.lines[2], "main:   ld hl,$1234");

    // The first line of the file, so there is nothing above it to show.
    let listing = sources.listing("src/main.asm", 1, 5).expect("has text");
    assert_eq!((listing.first, listing.lines.len()), (1, 6));

    // Past the end: the window is empty rather than out of bounds.
    let listing = sources.listing("src/plot.asm", 99, 2).expect("has text");
    assert!(listing.lines.is_empty());
    assert_eq!(listing.current, None);
}

#[test]
fn source_newer_than_the_debug_info_is_reported_as_stale() {
    let dir = std::env::temp_dir().join("rkw-dbginfo-test-stale");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("can create a directory");
    std::fs::write(
        dir.join("src/main.asm"),
        "; a header\nwidth   equ 70000\n\nmain:   ld hl,$1234\n\n\n\n        plot 1\n        plot 2\n\n\n        ret\n",
    )
    .expect("can write");
    std::fs::write(dir.join("src/plot.asm"), "plot    macro n\n").expect("can write");

    // The sidecar was written a minute before the source was last touched.
    let written = SystemTime::now() - Duration::from_secs(60);
    let sources = Sources::load(info(), Some(&dir), Some(written));

    assert_eq!(sources.stale(), ["src/main.asm", "src/plot.asm"]);
    // Stale or not, the text is there and says so at the point it is shown.
    let here = sources.locate(0x8000).expect("covered");
    assert_eq!(here.text.as_deref(), Some("main:   ld hl,$1234"));
    assert!(here.stale);
    assert!(
        sources
            .listing("src/main.asm", 1, 2)
            .expect("has text")
            .stale
    );

    // Written before the sidecar, so nothing is stale and the text stands.
    let sources = Sources::load(info(), Some(&dir), Some(SystemTime::now()));
    assert!(sources.stale().is_empty());
    let here = sources.locate(0x8005).expect("covered");
    assert_eq!(here.text.as_deref(), Some("        ret"));
    assert!(!here.stale);

    // A file that is named but not on disk is absent rather than stale, and
    // everything that does not need its text still works.
    let sources = Sources::load(info(), Some(&dir.join("nowhere")), None);
    assert!(sources.stale().is_empty());
    assert_eq!(sources.locate(0x8000).expect("covered").text, None);
    assert_eq!(sources.address_of("main"), Ok(0x8000));
}
