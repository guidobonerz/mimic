//! The terminal behind the window.
//!
//! `mimic-desktop` is built as a Windows GUI program (`windows_subsystem =
//! "windows"` in `main.rs`), so a double-click or a shortcut opens no
//! console window next to the emulator. Started from a terminal, it
//! attaches to that terminal instead, so `--help`, the gamepad and server
//! messages and errors still appear there. The shell does not wait for a
//! GUI program, so the output lands after its prompt; `mimic-headless`
//! stays a console program.
//!
//! Without a terminal the messages go nowhere; [`report`] then shows a
//! dialog instead, so a start that fails by double-click is not silent.

use std::sync::atomic::{AtomicBool, Ordering};

static TERMINAL: AtomicBool = AtomicBool::new(false);

/// Attach to the parent process's console, if it has one, and remember
/// whether standard output and error now reach a terminal.
pub fn attach_parent() {
    TERMINAL.store(attach(), Ordering::Relaxed);
}

/// Whether messages printed to stdout/stderr reach a terminal.
pub fn has_terminal() -> bool {
    TERMINAL.load(Ordering::Relaxed)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn attach() -> bool {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };
    // SAFETY: GetStdHandle and AttachConsole take constants and touch no
    // memory of ours. A GUI program starts without standard handles unless
    // the parent redirected them (a pipe or a file): then the output has a
    // reader and no console is needed. Otherwise AttachConsole fails
    // harmlessly (returns 0) when the parent has no console. Rust's standard
    // streams look their handles up on every use, so they pick up the
    // attached console afterwards.
    unsafe {
        let redirected = [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE].into_iter().any(|h| {
            let handle = GetStdHandle(h);
            !handle.is_null() && handle != INVALID_HANDLE_VALUE
        });
        redirected || AttachConsole(ATTACH_PARENT_PROCESS) != 0
    }
}

/// Other platforms keep the terminal they were started from.
#[cfg(not(windows))]
fn attach() -> bool {
    true
}

/// Report a message on the terminal when there is one, in a dialog
/// otherwise (`title` names the dialog).
pub fn report(title: &str, message: &str, error: bool) {
    if has_terminal() {
        if error {
            eprintln!("{message}");
        } else {
            println!("{message}");
        }
        return;
    }
    rfd::MessageDialog::new()
        .set_title(title)
        .set_description(message)
        .set_level(if error {
            rfd::MessageLevel::Error
        } else {
            rfd::MessageLevel::Info
        })
        .show();
}
