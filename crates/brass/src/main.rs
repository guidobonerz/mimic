//! `brass`: compile and run Brass programs (BRASS.md 12).

use std::path::PathBuf;
use std::process::ExitCode;

use asm_ada::run::{run, Board, End};
use brass::{build, DirSources, LOAD};

const USAGE: &str = "usage:
  brass build DIR -o PROG.bin [--asm PROG.s] [--listing PROG.lst]
  brass run DIR [--cycles N]
      (DIR holds package main; imported packages are its subfolders or the
       built-in library. The binary loads at 0040:0000: put it on a card
       with `ada-asm sd card.img --boot PROG.bin`.)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((cmd, rest)) = args.split_first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let mut dir = None;
    let (mut out, mut asm, mut listing, mut cycles) = (None, None, None, 200_000_000u64);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned();
        match a.as_str() {
            "-o" => out = value(),
            "--asm" => asm = value(),
            "--listing" => listing = value(),
            "--cycles" => match value().and_then(|v| v.parse().ok()) {
                Some(n) => cycles = n,
                None => {
                    eprintln!("--cycles N\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
            s if s.starts_with('-') => {
                eprintln!("unknown option {s}\n{USAGE}");
                return ExitCode::from(2);
            }
            _ => dir = Some(PathBuf::from(a)),
        }
    }
    let Some(dir) = dir else {
        eprintln!("no program folder\n{USAGE}");
        return ExitCode::from(2);
    };
    let program = match build(&DirSources { root: dir }) {
        Ok(p) => p,
        Err(errors) => {
            for e in errors {
                eprintln!("{e}");
            }
            return ExitCode::FAILURE;
        }
    };
    let write = |path: &Option<String>, bytes: &[u8]| -> bool {
        match path {
            Some(p) => match std::fs::write(p, bytes) {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("{p}: {e}");
                    false
                }
            },
            None => true,
        }
    };
    if !write(&asm, program.asm.as_bytes()) || !write(&listing, program.listing.as_bytes()) {
        return ExitCode::FAILURE;
    }
    match cmd.as_str() {
        "build" => {
            if out.is_none() {
                eprintln!("-o PROG.bin missing\n{USAGE}");
                return ExitCode::from(2);
            }
            if !write(&out, &program.binary) {
                return ExitCode::FAILURE;
            }
            println!("{}: {} bytes", out.unwrap(), program.binary.len());
            ExitCode::SUCCESS
        }
        "run" => {
            let mut b = Board::default();
            if let Err(e) = b.load(LOAD, &program.binary) {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
            let r = run(&mut b, LOAD, cycles);
            print!("{}", String::from_utf8_lossy(&r.output));
            match r.end {
                End::Exit(0) => ExitCode::SUCCESS,
                End::Exit(c) => {
                    eprintln!("exit {c}");
                    ExitCode::from(c.min(255) as u8)
                }
                End::Limit => {
                    eprintln!("no end within {cycles} cycles");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!("unknown command {cmd}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}
