//! `ada-asm`: assemble, link and run Ada programs (see the crate docs).

use std::path::Path;
use std::process::ExitCode;

use asm_ada::asm::parse_seg_off;
use asm_ada::run::{run, Board, End};
use asm_ada::{assemble, link, HostFiles, Image, Object, Options};

const USAGE: &str = "usage:
  ada-asm build FILE.s [FILE.s …] -o OUT.bin [--base SEG:OFF] [--listing OUT.lst]
  ada-asm compile FILE.s -o OUT.ao
  ada-asm link FILE.ao [FILE.ao …] -o OUT.bin [--base SEG:OFF] [--listing OUT.lst]
  ada-asm run PROGRAM.bin [--load SEG:OFF] [--entry SEG:OFF] [--cycles N]
  ada-asm sd CARD.img [--size MIB] [--boot PROGRAM.bin] [--add CARD/PATH=HOSTFILE …]
      (a FAT32 SD card for the Logophor 16; --boot puts the program at
       BOOT/BOILER.BIN, which Foundry loads at 0040:0000 and starts)";

struct Args {
    inputs: Vec<String>,
    out: Option<String>,
    base: u32,
    listing: Option<String>,
    load: u32,
    entry: Option<u32>,
    cycles: u64,
    size: u32,
    add: Vec<(String, String)>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args {
        inputs: Vec::new(),
        out: None,
        base: Options::default().base,
        listing: None,
        load: 0x0040_0000,
        entry: None,
        cycles: 120_000_000,
        size: 64,
        add: Vec::new(),
    };
    let mut it = args.iter();
    while let Some(x) = it.next() {
        let mut value = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{x} needs a value"))
        };
        match x.as_str() {
            "-o" => a.out = Some(value()?),
            "--base" => a.base = parse_seg_off(&value()?)?,
            "--listing" => a.listing = Some(value()?),
            "--load" => a.load = parse_seg_off(&value()?)?,
            "--entry" => a.entry = Some(parse_seg_off(&value()?)?),
            "--cycles" => a.cycles = value()?.parse().map_err(|_| "--cycles N".to_string())?,
            "--size" => a.size = value()?.parse().map_err(|_| "--size MIB".to_string())?,
            "--boot" => a.add.push(("BOOT/BOILER.BIN".into(), value()?)),
            "--add" => {
                let v = value()?;
                let (card, host) = v.split_once('=').ok_or("--add CARD/PATH=HOSTFILE")?;
                a.add.push((card.into(), host.into()));
            }
            _ if x.starts_with('-') => return Err(format!("unknown option {x}")),
            _ => a.inputs.push(x.clone()),
        }
    }
    if a.inputs.is_empty() {
        return Err("no input file".into());
    }
    Ok(a)
}

fn read(p: &str) -> Result<String, String> {
    std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))
}

fn write_image(img: &Image, a: &Args) -> Result<(), String> {
    let out = a.out.as_deref().ok_or("-o OUT.bin missing")?;
    let (start, bytes) = img
        .contiguous(0xff, 16 << 20)
        .ok_or("the program spans more than 16 MiB: link its parts separately")?;
    std::fs::write(out, bytes).map_err(|e| format!("{out}: {e}"))?;
    println!(
        "{out}: {:04x}:{:04x}, {} bytes",
        start >> 16,
        start & 0xffff,
        std::fs::metadata(out).map(|m| m.len()).unwrap_or(0)
    );
    if let Some(l) = &a.listing {
        std::fs::write(l, &img.listing).map_err(|e| format!("{l}: {e}"))?;
    }
    Ok(())
}

fn objects(a: &Args, compiled: bool) -> Result<Vec<Object>, Vec<String>> {
    let mut objs = Vec::new();
    let mut errors = Vec::new();
    for f in &a.inputs {
        let r = if compiled {
            std::fs::read(f)
                .map_err(|e| vec![format!("{f}: {e}")])
                .and_then(|b| Object::from_bytes(&b).map_err(|e| vec![format!("{f}: {e}")]))
        } else {
            read(f)
                .map_err(|e| vec![e])
                .and_then(|src| assemble(f, &src, &HostFiles))
        };
        match r {
            Ok(o) => objs.push(o),
            Err(e) => errors.extend(e),
        }
    }
    if errors.is_empty() {
        Ok(objs)
    } else {
        Err(errors)
    }
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some((cmd, rest)) = argv.split_first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let a = match parse(rest) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let result: Result<(), Vec<String>> = match cmd.as_str() {
        "build" | "link" => objects(&a, cmd == "link").and_then(|objs| {
            let img = link(&objs, Options { base: a.base })?;
            write_image(&img, &a).map_err(|e| vec![e])
        }),
        "compile" => objects(&a, false).and_then(|objs| {
            let out = a.out.clone().unwrap_or_else(|| {
                Path::new(&a.inputs[0])
                    .with_extension("ao")
                    .display()
                    .to_string()
            });
            std::fs::write(&out, objs[0].to_bytes()).map_err(|e| vec![format!("{out}: {e}")])
        }),
        "run" => {
            let r = std::fs::read(&a.inputs[0]).map_err(|e| vec![format!("{}: {e}", a.inputs[0])]);
            r.and_then(|bytes| {
                let mut board = Board::default();
                board.load(a.load, &bytes).map_err(|e| vec![e])?;
                let r = run(&mut board, a.entry.unwrap_or(a.load), a.cycles);
                print!("{}", String::from_utf8_lossy(&r.output));
                match r.end {
                    End::Exit(code) => {
                        eprintln!("exit {code} after {} cycles", r.cycles);
                        if code != 0 {
                            return Err(vec![format!("program exit code {code}")]);
                        }
                        Ok(())
                    }
                    End::Limit => Err(vec![format!(
                        "no DBGEXIT within {} cycles (PC {:04x}:{:04x})",
                        a.cycles, r.cpu.s[0], r.cpu.pc
                    )]),
                }
            })
        }
        "sd" => {
            let mut files = Vec::new();
            let mut errors = Vec::new();
            for (card, host) in &a.add {
                match std::fs::read(host) {
                    Ok(b) => files.push((card.clone(), b)),
                    Err(e) => errors.push(format!("{host}: {e}")),
                }
            }
            if !errors.is_empty() {
                Err(errors)
            } else {
                let out = &a.inputs[0];
                asm_ada::sdimage::fat32(a.size, &files)
                    .and_then(|img| std::fs::write(out, img).map_err(|e| format!("{out}: {e}")))
                    .map(|()| println!("{out}: FAT32, {} MiB, {} file(s)", a.size, files.len()))
                    .map_err(|e| vec![e])
            }
        }
        _ => Err(vec![format!("unknown command {cmd}\n{USAGE}")]),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(errors) => {
            for e in errors {
                eprintln!("{e}");
            }
            ExitCode::FAILURE
        }
    }
}
