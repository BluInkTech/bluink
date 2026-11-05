// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Command results go to stdout; host failures to stderr. While the server
//! console owns the terminal, host failures become diagnostic events instead.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};

static CONSOLE_ACTIVE: AtomicBool = AtomicBool::new(false);

pub(super) fn set_console_active(active: bool) {
    CONSOLE_ACTIVE.store(active, Ordering::Release);
}

pub(super) fn console_active() -> bool {
    CONSOLE_ACTIVE.load(Ordering::Acquire)
}

/// One line of command output or help.
pub(super) fn line(text: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{text}");
}

/// A host failure the user must see. Colour follows the stderr terminal and
/// the `NO_COLOR` convention.
pub(super) fn error(message: &str) {
    if console_active() {
        tracing::error!(target: "bluink.host", event = "host_error", error = message);
        return;
    }
    let stderr = std::io::stderr();
    let colour = stderr.is_terminal() && std::env::var_os("NO_COLOR").is_none();
    let mut stderr = stderr.lock();
    let _ = if colour {
        writeln!(stderr, "❌ \u{1b}[31m{message}\u{1b}[39m")
    } else {
        writeln!(stderr, "❌ {message}")
    };
}

/// Plain progress for non-console runs, such as the cleanup notice.
pub(super) fn notice(message: &str) {
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}
