//! The shell: scripts, `source`, the read loop, and the exit code.
//!
//! All of it against a `&str` and a `Vec<u8>`, because the only thing that
//! makes a REPL hard to test is the terminal, and there isn't one in here.

use std::io::BufReader;
use std::path::PathBuf;

use rkw_cli::{Flow, Shell, load};
use rkw_debug::cmd::Session;
use rkw_debug::emu::Config;
use z80::{Cpu, FlatMemory};

const ORG: u16 = 0x8000;

/// ```text
/// 8000  3E 2A     LD A,42
/// 8002  06 07     LD B,7
/// 8004  76        HALT
/// ```
const PROGRAM: &[u8] = &[0x3E, 0x2A, 0x06, 0x07, 0x76];

fn shell() -> Shell<FlatMemory> {
    let mut mem = FlatMemory::new();
    mem.load(ORG, PROGRAM);
    let mut cpu = Cpu::new();
    cpu.regs.pc = ORG;
    cpu.regs.sp = 0xFF00;
    Shell::new(Session::new(cpu, mem, Config::default()))
}

fn script(shell: &mut Shell<FlatMemory>, text: &str) -> String {
    let mut out = Vec::new();
    shell.script(text, &mut out).expect("writing to a Vec");
    String::from_utf8(out).expect("the formatter writes UTF-8")
}

/// A temporary file with a name unique to this process and this test.
fn temp(name: &str, contents: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("rkw-cli-{}-{name}", std::process::id()));
    std::fs::write(&path, contents).expect("writing a temporary file");
    path
}

#[test]
fn a_script_is_a_session_without_a_terminal() {
    let mut shell = shell();
    let text = script(&mut shell, "step\nstep\nregs\n");
    assert!(text.contains("AF=2A"), "{text}");
    assert_eq!(shell.session().regs().b, 7);
    assert_eq!(shell.errors(), 0);
}

#[test]
fn blank_lines_and_comments_produce_nothing() {
    let mut shell = shell();
    assert_eq!(script(&mut shell, "\n; nothing here\n\n"), "");
    assert_eq!(shell.errors(), 0);
}

#[test]
fn an_error_is_reported_counted_and_survived() {
    let mut shell = shell();
    let text = script(&mut shell, "brake $8002\nstep\n");
    assert!(text.contains("error: unknown command `brake`"), "{text}");
    assert!(text.contains("^"), "with a caret: {text}");
    assert_eq!(shell.errors(), 1);
    assert_eq!(
        shell.session().regs().pc,
        ORG + 2,
        "and the next command still ran"
    );
}

#[test]
fn quit_ends_a_script_where_it_stands() {
    let mut shell = shell();
    let mut out = Vec::new();
    let flow = shell
        .script("step\nquit\nstep\n", &mut out)
        .expect("writing to a Vec");
    assert_eq!(flow, Flow::Quit);
    assert_eq!(
        shell.session().regs().pc,
        ORG + 2,
        "the third line did not run"
    );
}

#[test]
fn source_reads_a_file_and_echoes_what_it_runs() {
    let path = temp("inner.rkw", "break $8004\ncontinue\n");
    let mut shell = shell();
    let text = script(&mut shell, &format!("source {}\nregs\n", path.display()));

    assert!(text.contains("(rkw) break $8004"), "echoed: {text}");
    assert!(text.contains("Breakpoint 1 at $8004"), "{text}");
    assert!(text.contains("AF=2A"), "{text}");
    assert_eq!(shell.errors(), 0);
    std::fs::remove_file(path).ok();
}

#[test]
fn a_file_that_sources_itself_stops_at_the_depth_limit() {
    let path = temp("loop.rkw", "");
    std::fs::write(&path, format!("source {}\n", path.display())).unwrap();

    let mut shell = shell();
    let text = script(&mut shell, &format!("source {}\n", path.display()));
    assert!(text.contains("sourcing itself"), "{text}");
    assert_eq!(shell.errors(), 1, "reported once, at the bottom");
    std::fs::remove_file(path).ok();
}

#[test]
fn a_missing_file_is_an_error_and_not_the_end_of_the_session() {
    let mut shell = shell();
    let text = script(&mut shell, "source /nowhere/at/all.rkw\nstep\n");
    assert!(text.contains("error: /nowhere/at/all.rkw"), "{text}");
    assert_eq!(shell.errors(), 1);
    assert_eq!(shell.session().regs().pc, ORG + 2);
}

#[test]
fn an_empty_line_repeats_the_last_command() {
    let mut shell = shell();
    let mut input = BufReader::new(&b"step\n\n\nquit\n"[..]);
    let mut out = Vec::new();
    shell.repl(&mut input, &mut out).expect("writing to a Vec");

    assert_eq!(
        shell.session().regs().pc,
        ORG + 4,
        "one typed step and two repeats"
    );
    let text = String::from_utf8(out).unwrap();
    assert_eq!(
        text.matches("(rkw) ").count(),
        4,
        "one prompt per line read"
    );
}

#[test]
fn end_of_input_ends_the_read_loop() {
    let mut shell = shell();
    let mut input = BufReader::new(&b"step\n"[..]);
    let mut out = Vec::new();
    shell.repl(&mut input, &mut out).expect("writing to a Vec");
    assert_eq!(shell.session().regs().pc, ORG + 2);
}

/// The whole reason ADR-0013 wanted scriptability: assemble, run to a place,
/// assert on a register — without a terminal, and without a second harness.
#[test]
fn assemble_run_and_assert() {
    let source = temp(
        "harness.asm",
        "        org $8000\n\
         start:  ld a,7\n\
         loop:   dec a\n\
                 jr nz,loop\n\
                 halt\n",
    );
    let mut mem = FlatMemory::new();
    let program = load::assemble_file(&mut mem, &source).expect("it assembles");
    assert_eq!(program.entry, Some(0x8000));
    assert_eq!(program.loaded[0].len, 6);

    let mut cpu = Cpu::new();
    cpu.regs.pc = program.entry.unwrap();
    let mut shell = Shell::new(Session::new(cpu, mem, Config::default()));
    let text = script(&mut shell, "continue\nregs\n");

    assert!(text.contains("Halted with interrupts disabled."), "{text}");
    assert_eq!(shell.session().regs().a, 0, "the loop ran to zero");
    assert_eq!(shell.errors(), 0);
    std::fs::remove_file(source).ok();
}

#[test]
fn a_source_file_that_does_not_assemble_says_so() {
    let source = temp("broken.asm", "    ld a,(hl\n");
    let mut mem = FlatMemory::new();
    let error = load::assemble_file(&mut mem, &source).expect_err("it does not assemble");
    assert!(error.to_string().contains("expected `)`"), "{error}");
    std::fs::remove_file(source).ok();
}

#[test]
fn a_raw_binary_loads_where_it_is_told() {
    let path = temp("raw.bin", "");
    std::fs::write(&path, PROGRAM).unwrap();
    let mut mem = FlatMemory::new();
    let program = load::binary_file(&mut mem, &path, 0x9000).expect("it loads");
    assert_eq!(program.entry, Some(0x9000));
    assert_eq!(&mem.ram[0x9000..0x9005], PROGRAM);
    std::fs::remove_file(path).ok();
}

#[test]
fn command_line_numbers_take_the_same_spellings_as_commands() {
    assert_eq!(load::number("$8000"), Some(0x8000));
    assert_eq!(load::number("0x8000"), Some(0x8000));
    assert_eq!(load::number("32768"), Some(0x8000));
    assert_eq!(load::number("%1010"), Some(10));
    assert_eq!(load::number("$10000"), None, "beyond sixteen bits");
    assert_eq!(load::number("nonsense"), None);
}

/// The same loop as `assemble_run_and_assert`, said in source terms: break on
/// a label, break on a line, and read the source back — which is ticket 0011's
/// half of what ADR-0013 wanted scriptability for.
#[test]
fn assemble_and_debug_in_source_terms() {
    // Written with its indentation intact: a name in the first column is a
    // label, so an invocation has to be indented like the assembler's own.
    let source = temp(
        "sourced.asm",
        concat!(
            "        org $8000\n",
            "twice   macro\n",
            "        nop\n",
            "        endm\n",
            "start:  ld a,7\n",
            "        twice\n",
            "        twice\n",
            "done:   halt\n",
        ),
    );
    let name = source
        .file_name()
        .expect("a file name")
        .to_string_lossy()
        .to_string();
    let mut mem = FlatMemory::new();
    let program = load::assemble_file(&mut mem, &source).expect("it assembles");
    let sources = program.sources.clone().expect("assembling brings its own");
    assert!(sources.stale().is_empty(), "just assembled, so not stale");

    let mut cpu = Cpu::new();
    cpu.regs.pc = program.entry.unwrap();
    let mut session = Session::new(cpu, mem, Config::default());
    session.set_sources(sources);
    let mut shell = Shell::new(session);

    // The `nop` inside the macro is one line and two addresses.
    // Named by its base name, which the debug info holds as a whole path.
    let text = script(&mut shell, &format!("break {name}:3\n"));
    assert_eq!(text.matches("Breakpoint").count(), 2, "{text}");

    let text = script(&mut shell, "continue\n");
    assert!(text.contains(&format!("{name}:3")), "{text}");
    assert!(text.contains("in macro `twice`"), "{text}");

    // A label is an address, and `list` shows what is around it.
    let text = script(&mut shell, "break done\ncontinue\nlist\n");
    assert!(text.contains("halt"), "{text}");
    assert_eq!(shell.errors(), 0, "{text}");
    std::fs::remove_file(source).ok();
}

#[test]
fn a_binary_and_its_sidecar_come_back_together_and_notice_a_stale_source() {
    let dir = std::env::temp_dir().join(format!("rkw-cli-{}-sidecar", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    let source = dir.join("prog.asm");
    std::fs::write(
        &source,
        "        org $8000\nstart:  ld a,42\n        halt\n",
    )
    .expect("written");

    // Assemble, write the sidecar beside the binary, and throw the session
    // away: what is left is what someone building elsewhere hands the debugger.
    let mut mem = FlatMemory::new();
    let program = load::assemble_file(&mut mem, &source).expect("it assembles");
    let info = program.sources.expect("debug info").info().clone();
    let sidecar = load::sidecar_of(&dir.join("prog.bin"));
    info.write(&sidecar).expect("written");

    let sources = load::debug_file(&sidecar).expect("read back");
    assert_eq!(sources.address_of("start"), Ok(0x8000));
    assert_eq!(
        sources
            .locate(0x8000)
            .expect("covered")
            .text
            .as_deref()
            .map(str::trim),
        Some("start:  ld a,42"),
        "the source was found beside the sidecar"
    );
    assert!(sources.stale().is_empty());

    // Edit the source and it is the same binary with text that no longer
    // describes it, which is the one thing worth being told about.
    std::fs::write(&source, "        org $8000\n        nop\nstart:  ld a,42\n").expect("written");
    let sources = load::debug_file(&sidecar).expect("read back");
    assert_eq!(sources.stale().len(), 1);
    assert!(
        sources.stale()[0].ends_with("prog.asm"),
        "{:?}",
        sources.stale()
    );
    assert!(sources.locate(0x8000).expect("covered").stale);

    let _ = std::fs::remove_dir_all(&dir);
}
