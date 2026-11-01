//! Where the emulated printers' jobs go on the host: a folder, and, if the
//! user asks for it, a real printer.
//!
//! [`PrintFolder`] is a [`FileHost`] over a folder that hands every page
//! picture to the host's print system when its file is closed: on Windows
//! through .NET's `PrintDocument` in PowerShell, elsewhere through `lp`
//! (CUPS). The machine only writes files (`devices-printer`), so it sees
//! nothing of the host, and an input replay, which the journal answers,
//! prints no paper. Sending is fire-and-forget on a child process: a
//! printer that is off or missing shows in the host's queue and with the
//! child's error on the frontend's terminal, never in the emulation.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use emu_core::host::{DirEntry, FileHandle, FileHost, FileStat, FsError, OpenMode};

use crate::host_fs::FsFolder;

/// The mount name; `devices_printer::FOLDER` is this under the root.
pub const MOUNT: &str = "Printer";

/// Draws the page (env MIMIC_PRINT_FILE) to fit the printable area of the
/// printer in env MIMIC_PRINTER, or the default printer.
const WINDOWS_SCRIPT: &str = r#"
Add-Type -AssemblyName System.Drawing
$d = New-Object System.Drawing.Printing.PrintDocument
if ($env:MIMIC_PRINTER) { $d.PrinterSettings.PrinterName = $env:MIMIC_PRINTER }
if ($env:MIMIC_PRINT_TO_FILE) {
    $d.PrinterSettings.PrintToFile = $true
    $d.PrinterSettings.PrintFileName = $env:MIMIC_PRINT_TO_FILE
}
$d.DocumentName = 'mimic'
$d.add_PrintPage({
    param($sender, $e)
    $i = [System.Drawing.Image]::FromFile($env:MIMIC_PRINT_FILE)
    $b = $e.MarginBounds
    $k = [Math]::Min($b.Width / $i.Width, $b.Height / $i.Height)
    $e.Graphics.InterpolationMode = 'NearestNeighbor'
    $e.Graphics.DrawImage($i, $b.X, $b.Y, [int]($i.Width * $k), [int]($i.Height * $k))
    $i.Dispose()
})
$d.Print()
"#;

/// The command that prints `file` on `printer` (`None`: the default one).
pub fn print_command(file: &Path, printer: Option<&str>) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-NonInteractive", "-Command", WINDOWS_SCRIPT])
            .env("MIMIC_PRINT_FILE", file);
        if let Some(p) = printer {
            c.env("MIMIC_PRINTER", p);
        }
        c
    } else {
        let mut c = Command::new("lp");
        if let Some(p) = printer {
            c.args(["-d", p]);
        }
        c.arg(file);
        c
    }
}

/// How the frontends configure the printer's output.
#[derive(Clone, Debug)]
pub struct PrinterOutput {
    pub folder: PathBuf,
    /// Send pages to a real printer: its name, or `None` inside for the
    /// default one.
    pub host: Option<Option<String>>,
}

impl PrinterOutput {
    /// From the machine option `printer` and the frontends' `--printer-dir`
    /// (default `./printer`) and `--host-printer NAME` (`default`: the
    /// default printer); `None` without a printer.
    pub fn for_options(
        printer: Option<&str>,
        folder: Option<PathBuf>,
        host: Option<String>,
    ) -> Option<Self> {
        printer.filter(|v| *v != "off")?;
        Some(Self {
            folder: folder.unwrap_or_else(|| "printer".into()),
            host: host.map(|n| Some(n).filter(|n| n != "default")),
        })
    }

    /// The file host, with its folder created.
    pub fn into_file_host(self) -> Result<Box<dyn FileHost>, String> {
        std::fs::create_dir_all(&self.folder)
            .map_err(|e| format!("{}: {e}", self.folder.display()))?;
        let inner = FsFolder::new(self.folder.clone());
        Ok(match self.host {
            Some(printer) => Box::new(PrintFolder {
                inner,
                folder: self.folder,
                printer,
                open: Vec::new(),
            }),
            None => Box::new(inner),
        })
    }
}

/// A folder that prints the page pictures closed in it.
pub struct PrintFolder {
    inner: FsFolder,
    folder: PathBuf,
    printer: Option<String>,
    /// Pictures being written.
    open: Vec<(FileHandle, String)>,
}

impl PrintFolder {
    fn print(&self, path: &str) {
        let file = self.folder.join(path.trim_start_matches('/'));
        let spawned = print_command(&file, self.printer.as_deref())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn();
        if let Err(e) = spawned {
            eprintln!("printer: {}: {e}", file.display());
        }
    }
}

impl FileHost for PrintFolder {
    fn open(&mut self, path: &str, mode: OpenMode) -> Result<FileHandle, FsError> {
        let h = self.inner.open(path, mode)?;
        if mode.write && path.ends_with(".png") {
            self.open.push((h, path.to_string()));
        }
        Ok(h)
    }
    fn read(&mut self, h: FileHandle, buf: &mut [u8]) -> Result<usize, FsError> {
        self.inner.read(h, buf)
    }
    fn write(&mut self, h: FileHandle, data: &[u8]) -> Result<usize, FsError> {
        self.inner.write(h, data)
    }
    fn seek(&mut self, h: FileHandle, pos: u64) -> Result<(), FsError> {
        self.inner.seek(h, pos)
    }
    fn fstat(&mut self, h: FileHandle) -> Result<FileStat, FsError> {
        self.inner.fstat(h)
    }
    fn close(&mut self, h: FileHandle) {
        self.inner.close(h);
        if let Some(i) = self.open.iter().position(|(o, _)| *o == h) {
            let (_, path) = self.open.remove(i);
            self.print(&path);
        }
    }
    fn stat(&mut self, path: &str) -> Result<FileStat, FsError> {
        self.inner.stat(path)
    }
    fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, FsError> {
        self.inner.list_dir(path)
    }
    fn mkdir(&mut self, path: &str) -> Result<(), FsError> {
        self.inner.mkdir(path)
    }
    fn delete(&mut self, path: &str) -> Result<(), FsError> {
        self.inner.delete(path)
    }
    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        self.inner.rename(from, to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_names_the_file_and_the_printer() {
        let c = print_command(Path::new("page.png"), Some("Laser"));
        if cfg!(windows) {
            assert_eq!(c.get_program(), "powershell");
            let env: Vec<_> = c.get_envs().collect();
            assert!(env
                .iter()
                .any(|(k, v)| *k == "MIMIC_PRINTER" && v.is_some_and(|v| v == "Laser")));
            assert!(env.iter().any(|(k, _)| *k == "MIMIC_PRINT_FILE"));
        } else {
            assert_eq!(c.get_program(), "lp");
            let args: Vec<_> = c.get_args().collect();
            assert_eq!(args, ["-d", "Laser", "page.png"]);
        }
    }
}
