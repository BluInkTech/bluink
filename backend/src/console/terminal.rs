// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::io::{self, Stderr, Write};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossbeam_channel::Receiver;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    style::ResetColor,
    terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode, size},
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};

use super::Core;
use super::model::{Entry, Model, Status, View, summary, text};
use crate::diagnostics::DiagnosticEvent;

static RAW: AtomicBool = AtomicBool::new(false);
const STOPPED: &str = "Bluink has stopped. You can close this window.";
static PANIC_HOOK: Once = Once::new();

fn restore() {
    if RAW.swap(false, Ordering::AcqRel) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stderr(),
            cursor::Show,
            cursor::MoveToColumn(0),
            ResetColor
        );
    }
}

struct RawGuard;
impl Drop for RawGuard {
    fn drop(&mut self) {
        restore();
    }
}

struct Renderer {
    terminal: Terminal<CrosstermBackend<Stderr>>,
    dimensions: (u16, u16),
    height: u16,
    _raw: RawGuard,
}

impl Renderer {
    fn new(height: u16, dimensions: (u16, u16)) -> io::Result<Self> {
        PANIC_HOOK.call_once(|| {
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                restore();
                previous(info);
            }));
        });
        enable_raw_mode()?;
        RAW.store(true, Ordering::Release);
        let raw = RawGuard;
        // Advance the primary screen rather than entering an alternate buffer.
        // Existing output remains available in the terminal's scrollback.
        write!(io::stderr(), "{}", "\r\n".repeat(usize::from(dimensions.1)))?;
        execute!(
            io::stderr(),
            cursor::MoveTo(0, dimensions.1.saturating_sub(height))
        )?;
        let terminal = Terminal::with_options(
            CrosstermBackend::new(io::stderr()),
            TerminalOptions {
                viewport: Viewport::Inline(height),
            },
        )?;
        Ok(Self {
            terminal,
            dimensions,
            height,
            _raw: raw,
        })
    }

    fn resize(&mut self, height: u16, dimensions: (u16, u16)) -> io::Result<()> {
        self.terminal.clear()?;
        // Reposition the panel without appending blank lines on every toggle.
        // Covered diagnostics remain available in the retained history views.
        execute!(
            io::stderr(),
            cursor::MoveTo(0, dimensions.1.saturating_sub(height))
        )?;
        self.terminal = Terminal::with_options(
            CrosstermBackend::new(io::stderr()),
            TerminalOptions {
                viewport: Viewport::Inline(height),
            },
        )?;
        // The new inline viewport can cover rows containing older logs. A new
        // diff buffer considers spaces unchanged, so clear those rows first.
        self.terminal.clear()?;
        self.dimensions = dimensions;
        self.height = height;
        Ok(())
    }

    fn log(&mut self, value: &str, priority: &str) -> io::Result<()> {
        let line = Line::styled(value, severity(priority));
        let paragraph = Paragraph::new(line).wrap(Wrap { trim: false });
        let rows = u16::try_from(paragraph.line_count(self.dimensions.0))
            .unwrap_or(u16::MAX)
            .max(1);
        self.terminal.insert_before(rows, |buffer| {
            paragraph.render(buffer.area, buffer);
        })
    }

    fn finish(&self, message: &str) -> io::Result<()> {
        // The viewport origin is already known and its buffer will not be reused.
        // Terminal::clear queries the cursor first, which can time out during
        // shutdown and prevent otherwise successful work from exiting cleanly.
        execute!(
            io::stderr(),
            cursor::MoveTo(0, self.dimensions.1.saturating_sub(self.height)),
            Clear(ClearType::FromCursorDown),
            cursor::Show
        )?;
        write!(io::stderr(), "{message}\r\n")?;
        io::stderr().flush()
    }
}

use ratatui::widgets::Widget;

fn style(color: Color) -> Style {
    if std::env::var_os("NO_COLOR").is_some() {
        Style::default()
    } else {
        Style::default().fg(color)
    }
}

fn severity(level: &str) -> Style {
    style(match level {
        "error" => Color::Red,
        "warn" => Color::Yellow,
        _ => Color::Reset,
    })
}

/// Resident memory in whole megabytes, as Task Manager and Activity Monitor show it.
fn megabytes(core: &Core) -> Option<u64> {
    let rss = core.rss.load(Ordering::Relaxed);
    (rss > 0).then_some(rss / 1_048_576)
}

fn plural(count: u64, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn shortcut(key: &str, label: &str) -> [Span<'static>; 2] {
    [
        Span::styled(
            key.to_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" {label}   ")),
    ]
}

fn footer(core: &Core, status: &Status, width: u16) -> Vec<Line<'static>> {
    let address = core.address();
    let state = status.label(address.is_some());
    let problems = core.errors.load(Ordering::Relaxed);
    let health_color = if status.failed {
        Color::Red
    } else if address.is_some() && status.ready {
        Color::Green
    } else {
        Color::Blue
    };
    let notice = if problems > 0 {
        format!(" · ▲ {} · press e", plural(problems, "problem", "problems"))
    } else {
        String::new()
    };
    let memory = megabytes(core)
        .map(|megabytes| format!("Using {megabytes} MB of memory"))
        .unwrap_or_default();
    let status_line = columns(&format!("● {state}{notice}"), &memory, width);
    let (left, right) = status_line.split_at(status_line.len() - memory.len());
    let (state_text, notice_text) = left.split_at(left.find(" · ▲").unwrap_or(left.len()));
    let keep_open = if width >= 100 {
        "Keep this window open while you write"
    } else {
        "Keep this window open"
    };
    let location = address.map_or_else(
        || "Your browser opens when Bluink is ready".to_owned(),
        |address| format!("{address} · {keep_open}"),
    );
    let shortcuts: Vec<Span<'static>> = if width >= 80 {
        [
            shortcut("o", "Open in browser"),
            shortcut("q", "Quit Bluink"),
            shortcut("?", "Help and details"),
        ]
    } else {
        [
            shortcut("o", "Open"),
            shortcut("q", "Quit"),
            shortcut("?", "Help"),
        ]
    }
    .into_iter()
    .flatten()
    .collect();
    let project = std::path::Path::new(&status.project)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let label = if project.is_empty() {
        "── Bluink ─".to_owned()
    } else {
        columns(&format!("── Bluink · {} ─", text(&project)), "", width)
            .trim_end()
            .to_owned()
    };
    let heading = format!(
        "{label}{}",
        "─".repeat(usize::from(width).saturating_sub(Line::raw(label.as_str()).width()))
    );
    vec![
        Line::styled(heading, style(Color::DarkGray)),
        Line::from(vec![
            Span::styled(state_text.to_owned(), style(health_color)),
            Span::styled(notice_text.to_owned(), severity("error")),
            Span::styled(right.to_owned(), style(Color::Cyan)),
        ]),
        Line::raw(columns(&location, "", width).trim_end().to_owned()),
        Line::from(shortcuts),
    ]
}

fn columns(left: &str, right: &str, width: u16) -> String {
    let available = usize::from(width).saturating_sub(Line::raw(right).width() + 2);
    let mut fitted = String::new();
    for character in left.chars() {
        if Line::raw(format!("{fitted}{character}")).width() > available {
            break;
        }
        fitted.push(character);
    }
    let padding =
        usize::from(width).saturating_sub(Line::raw(&fitted).width() + Line::raw(right).width());
    format!("{fitted}{}{right}", " ".repeat(padding))
}

fn duration(seconds: u64) -> String {
    match (seconds / 3_600, seconds / 60 % 60) {
        (0, 0) => "less than a minute".to_owned(),
        (0, minutes) => plural(minutes, "minute", "minutes"),
        (hours, minutes) => format!(
            "{} {}",
            plural(hours, "hour", "hours"),
            plural(minutes, "minute", "minutes")
        ),
    }
}

fn information(core: &Core, model: &Model, status: &Status) -> Vec<Line<'static>> {
    let diagnostic_file =
        std::env::var("BLUINK_DIAGNOSTICS_FILE").unwrap_or_else(|_| "Not configured".to_owned());
    let diagnostic_file =
        if diagnostic_file != "Not configured" && !crate::diagnostics::file_capture_active() {
            "Unavailable · file logging disabled".to_owned()
        } else {
            diagnostic_file
        };
    let memory = megabytes(core).map_or_else(
        || "Not measured yet".to_owned(),
        |megabytes| format!("{megabytes} MB, measured every five seconds"),
    );
    vec![
        Line::styled("About this project · Esc Close", style(Color::Blue)),
        Line::raw(format!("Project folder   {}", text(&status.project))),
        Line::raw(format!(
            "Address          {}",
            core.address().unwrap_or_else(|| "Not ready yet".to_owned())
        )),
        Line::raw(format!("Memory           {memory}")),
        Line::raw(format!(
            "Bluink           {} · {} / {}",
            text(&core.options.cli_version),
            std::env::consts::OS,
            std::env::consts::ARCH
        )),
        Line::raw(""),
        Line::styled("Technical details", style(Color::Blue)),
        Line::raw(format!(
            "Process {} · running for {}",
            std::process::id(),
            duration(core.started.elapsed().as_secs())
        )),
        Line::raw(format!("Startup step: {}", text(&status.phase))),
        Line::raw(format!("Diagnostics file {}", text(&diagnostic_file))),
        Line::raw(format!(
            "History: {} older entries removed; {} events skipped",
            model.history.evicted,
            core.dropped.load(Ordering::Relaxed)
        )),
    ]
}

/// A selected entry: what happened, what it means, then the raw event.
fn details(entry: &Entry) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw(entry.summary.clone())];
    if !entry.explanation.is_empty() {
        lines.extend([Line::raw(""), Line::raw(entry.explanation)]);
    }
    lines.extend([
        Line::raw(""),
        Line::styled("Technical details", style(Color::DarkGray)),
        // Paragraph wrapping keeps long fields inside the viewport.
        Line::raw(entry.details.clone()),
    ]);
    lines
}

/// The single line printed above the panel for a problem. Routine activity
/// stays in the Activity log rather than scrolling past the writer.
fn problem_line(event: &DiagnosticEvent) -> String {
    let time = summary(event).chars().take(5).collect::<String>();
    let explanation = super::model::explain(event);
    let sentence = explanation
        .split_inclusive(". ")
        .next()
        .unwrap_or(explanation)
        .trim_end();
    format!("{time}  ▲ {sentence} Press e for details.")
}

fn body(core: &Core, model: &Model, status: &Status, rows: usize) -> Vec<Line<'static>> {
    match model.view {
        View::Status => vec![],
        View::Help => [
            "Keyboard shortcuts · Esc Close",
            "o  Open Bluink in your browser   q  Quit Bluink (Ctrl+C also works)",
            "e  Problems this session         i  About this project",
            "w  Technical notes               l  Activity log",
            "↑ / ↓  Select or scroll          Page Up / Down  Move a page",
            "Home / End  First / newest       Enter  Show or hide details",
            "Problems are from this session only; Bluink may already have recovered.",
        ]
        .into_iter()
        .map(Line::raw)
        .collect(),
        View::Info => information(core, model, status),
        View::Logs | View::Errors | View::Warnings => {
            let entries = model.entries();
            let (title, empty) = match model.view {
                View::Errors => ("Problems", "No problems so far this session."),
                View::Warnings => ("Technical notes", "No technical notes so far this session."),
                _ => ("Activity log", "Nothing recorded yet."),
            };
            let mut lines = vec![Line::styled(
                format!("{title} · ↑↓ Select · Enter Details · Esc Close"),
                style(Color::Blue),
            )];
            if entries.is_empty() {
                lines.push(Line::raw(empty));
                return lines;
            }
            let index = entries
                .iter()
                .position(|entry| Some(entry.id) == model.selected)
                .unwrap_or(entries.len() - 1);
            if model.details {
                lines.extend(details(entries[index]));
            } else {
                let capacity = rows.saturating_sub(1).max(1);
                let start = index.saturating_sub(capacity - 1);
                for (offset, entry) in entries.iter().enumerate().skip(start).take(capacity) {
                    let color = match entry.level.as_str() {
                        "error" => Color::Red,
                        "warn" => Color::Yellow,
                        _ => Color::Reset,
                    };
                    let selected = offset == index;
                    let entry_style = if selected {
                        style(Color::Blue).add_modifier(Modifier::BOLD)
                    } else {
                        style(color)
                    };
                    lines.push(Line::styled(
                        format!("{} {}", if selected { "›" } else { " " }, entry.summary),
                        entry_style,
                    ));
                }
            }
            lines
        }
    }
}

fn handle_key(
    core: &Core,
    model: &mut Model,
    status: &Status,
    renderer: &Renderer,
    event: event::KeyEvent,
) {
    let body_rows = renderer.height.saturating_sub(4);
    let lines: Vec<_> = body(core, model, status, usize::from(body_rows))
        .into_iter()
        .skip(1)
        .collect();
    let limit = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .line_count(renderer.dimensions.0)
        .saturating_sub(usize::from(body_rows.saturating_sub(1)));
    key(
        core,
        model,
        event.code,
        event.modifiers,
        u16::try_from(limit).unwrap_or(u16::MAX),
    );
}

fn draw(frame: &mut Frame<'_>, core: &Core, model: &Model, status: &Status) {
    let area = frame.area();
    let body_height = area.height.saturating_sub(4);
    if body_height > 0 {
        let body_area = Rect::new(area.x, area.y, area.width, body_height);
        let mut lines = body(core, model, status, usize::from(body_height)).into_iter();
        if let Some(heading) = lines.next() {
            frame.render_widget(
                Paragraph::new(heading),
                Rect {
                    height: 1,
                    ..body_area
                },
            );
        }
        let content = Paragraph::new(lines.collect::<Vec<_>>()).wrap(Wrap { trim: false });
        let limit = content
            .line_count(area.width)
            .saturating_sub(usize::from(body_height.saturating_sub(1)));
        let limit = u16::try_from(limit).unwrap_or(u16::MAX);
        frame.render_widget(
            content.scroll((model.scroll.min(limit), 0)),
            Rect {
                y: body_area.y + 1,
                height: body_area.height.saturating_sub(1),
                ..body_area
            },
        );
    }
    let footer_area = Rect::new(area.x, area.y + body_height, area.width, 4.min(area.height));
    frame.render_widget(
        Paragraph::new(footer(core, status, area.width)),
        footer_area,
    );
}

fn key(
    core: &Core,
    model: &mut Model,
    key_code: KeyCode,
    modifiers: KeyModifiers,
    scroll_limit: u16,
) {
    if model.details || matches!(model.view, View::Info | View::Help) {
        match key_code {
            KeyCode::Up => model.scroll_content(-1, None, scroll_limit),
            KeyCode::Down => model.scroll_content(1, None, scroll_limit),
            KeyCode::PageUp => model.scroll_content(-10, None, scroll_limit),
            KeyCode::PageDown => model.scroll_content(10, None, scroll_limit),
            KeyCode::Home => model.scroll_content(0, Some(false), scroll_limit),
            KeyCode::End => model.scroll_content(0, Some(true), scroll_limit),
            _ => {}
        }
    } else {
        match key_code {
            KeyCode::Up => model.navigate(-1, None),
            KeyCode::Down => model.navigate(1, None),
            KeyCode::PageUp => model.navigate(-10, None),
            KeyCode::PageDown => model.navigate(10, None),
            KeyCode::Home => model.navigate(0, Some(false)),
            KeyCode::End => model.navigate(0, Some(true)),
            _ => {}
        }
    }
    match key_code {
        KeyCode::Char('q') => core.action("shutdown"),
        KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => core.action("shutdown"),
        KeyCode::Char('o') => core.action("openBrowser"),
        KeyCode::Char('l') => model.open(View::Logs),
        KeyCode::Char('e') => model.open(View::Errors),
        KeyCode::Char('w') => model.open(View::Warnings),
        KeyCode::Char('i') => model.open(View::Info),
        KeyCode::Char('?') => model.open(View::Help),
        KeyCode::Esc => model.open(View::Status),
        KeyCode::Enter if matches!(model.view, View::Logs | View::Errors | View::Warnings) => {
            model.details = !model.details;
            model.scroll = 0;
            model.following = false;
        }
        _ => {}
    }
}

pub(super) fn run(core: &Core, incoming: &Receiver<DiagnosticEvent>) -> io::Result<()> {
    let mut output = io::stderr();
    writeln!(output, "Bluink · {}", text(&core.options.cli_version))?;
    let mut renderer = if core.interactive {
        size()
            .ok()
            .filter(|(width, height)| *width >= 60 && *height >= 12)
            .and_then(|dimensions| Renderer::new(4, dimensions).ok())
    } else {
        None
    };
    let mut model = Model::default();
    let mut previous_phase = (String::new(), None);
    let mut project_shown = false;
    let mut previous_address = None;
    while !core.stop_requested.load(Ordering::Acquire) || !incoming.is_empty() {
        for event in incoming.try_iter().take(128) {
            model.push(&event);
            if let Some(renderer) = &mut renderer {
                if event.level == "error" {
                    renderer.log(&problem_line(&event), "error")?;
                }
            } else {
                writeln!(output, "{}", summary(&event))?;
            }
        }
        let status = core.status();
        let address = core.address();
        if renderer.is_none() && !project_shown && !status.project.is_empty() {
            let project = std::path::Path::new(&status.project)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            let introduction = format!("Project: {}", text(&project));
            writeln!(output, "{introduction}")?;
            project_shown = true;
        }
        let mut too_small = false;
        if let Some(renderer) = &mut renderer {
            let dimensions = size()?;
            if dimensions.0 < 60 || dimensions.1 < 12 {
                renderer.finish("Console resized; switching to plain output")?;
                // Dropping the guard restores input before streaming plain logs.
                renderer.terminal.show_cursor()?;
                restore();
                too_small = true;
            } else {
                let height = if model.view == View::Status {
                    4
                } else {
                    4 + 12.min(dimensions.1.saturating_sub(6))
                };
                if dimensions != renderer.dimensions || height != renderer.height {
                    renderer.resize(height, dimensions)?;
                }
                renderer
                    .terminal
                    .draw(|frame| draw(frame, core, &model, &status))?;
            }
        }
        if too_small {
            renderer = None;
        }
        if renderer.is_none() {
            let percent = status.percent().filter(|_| !status.failed);
            let milestone = percent.map(|p| p / 10);
            if !status.phase.is_empty()
                && (status.phase != previous_phase.0 || milestone != previous_phase.1)
            {
                writeln!(output, "{}", status.activity())?;
                previous_phase = (status.phase.clone(), milestone);
            }
            if address != previous_address
                && let Some(address) = &address
            {
                writeln!(output, "Server started at {address}/")?;
                previous_address = Some(address.clone());
            }
            output.flush()?;
            std::thread::sleep(Duration::from_millis(100));
        } else if event::poll(Duration::from_millis(100))?
            && let Event::Key(event) = event::read()?
            && event.kind != KeyEventKind::Release
            && let Some(renderer) = &renderer
        {
            handle_key(core, &mut model, &status, renderer, event);
        }
    }
    if let Some(renderer) = renderer {
        renderer.finish(STOPPED)?;
    } else {
        writeln!(output, "{STOPPED}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    /// A quiescent renderer lets the parent disable cursor reports at shutdown.
    #[cfg(unix)]
    #[test]
    #[ignore = "PTY child process invoked by the shutdown acceptance test"]
    fn shutdown_child() {
        let mut renderer = Renderer::new(4, size().unwrap()).unwrap();
        renderer
            .terminal
            .draw(|frame| {
                frame.render_widget(Paragraph::new("READY_FOR_SHUTDOWN"), frame.area());
            })
            .unwrap();
        loop {
            if let Event::Key(event) = event::read().unwrap()
                && event.code == KeyCode::Char('q')
                && event.kind != KeyEventKind::Release
            {
                break;
            }
        }
        renderer.finish(STOPPED).unwrap();
        drop(renderer);
        println!("TERMINAL_SESSION_CLOSED");
    }

    #[test]
    fn footer_keeps_status_memory_address_and_controls_visible_at_supported_widths() {
        let (core, _) = super::super::tests::fixture();
        *core.address.lock().unwrap() = Some("http://127.0.0.1:9000".to_owned());
        let status = Status {
            project: "example-project".to_owned(),
            ready: true,
            phase: "Project ready".to_owned(),
            ..Status::default()
        };
        let render = |width| {
            let mut terminal = Terminal::new(TestBackend::new(width, 4)).unwrap();
            terminal
                .draw(|frame| draw(frame, &core, &Model::default(), &status))
                .unwrap();
            format!("{:?}", terminal.backend())
        };
        for width in [60, 80, 120] {
            let output = render(width);
            for expected in [
                "● Ready",
                "Using 184 MB of memory",
                "http://127.0.0.1:9000 · Keep this window open",
                "o Open",
                "q Quit",
                "? Help",
                "── Bluink · example-project ─",
            ] {
                assert!(output.contains(expected), "{width}: {output}");
            }
            assert!(!output.contains("problem"), "{width}: {output}");
        }
        core.errors.store(2, Ordering::Relaxed);
        assert!(render(80).contains("▲ 2 problems · press e"));
    }

    #[test]
    fn warning_history_can_be_opened_navigated_and_inspected() {
        let (core, _) = super::super::tests::fixture();
        let status = Status::default();
        let mut model = Model::default();
        for (level, message) in [
            ("warn", "earlier warning"),
            ("info", "later activity"),
            ("warn", "latest warning"),
        ] {
            model.push(&DiagnosticEvent {
                timestamp_ms: 0,
                level: level.to_owned(),
                target: "test".to_owned(),
                name: None,
                message: Some(message.to_owned()),
                fields: std::collections::BTreeMap::default(),
                spans: vec![],
                duration_ms: None,
            });
        }
        key(&core, &mut model, KeyCode::Char('w'), KeyModifiers::NONE, 0);
        key(&core, &mut model, KeyCode::Home, KeyModifiers::NONE, 0);
        for width in [60, 80, 120] {
            let mut terminal = Terminal::new(TestBackend::new(width, 16)).unwrap();
            terminal
                .draw(|frame| draw(frame, &core, &model, &status))
                .unwrap();
            let output = format!("{:?}", terminal.backend());
            assert!(output.contains("Technical notes ·"));
            assert!(output.contains("earlier warning"));
            assert!(!output.contains("later activity"));
        }
        key(&core, &mut model, KeyCode::Enter, KeyModifiers::NONE, 0);
        assert!(model.details);
        assert!(!model.following);
        key(&core, &mut model, KeyCode::Esc, KeyModifiers::NONE, 0);
        assert!(model.view == View::Status);
    }
}
