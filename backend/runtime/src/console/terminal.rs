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

use super::model::{Model, Status, View, summary, text};
use super::{Core, interactive};
use crate::diagnostics::DiagnosticEvent;

static RAW: AtomicBool = AtomicBool::new(false);
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
        if height > self.height {
            // Make room by advancing the primary screen, retaining the log
            // lines displaced by an expanded view in ordinary scrollback.
            execute!(
                io::stderr(),
                cursor::MoveTo(0, dimensions.1.saturating_sub(1))
            )?;
            write!(
                io::stderr(),
                "{}",
                "\r\n".repeat(usize::from(height - self.height))
            )?;
        }
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

fn footer(core: &Core, status: &Status, width: u16) -> Vec<Line<'static>> {
    let address = core.address();
    let state = status.label(address.is_some(), core.started);
    let errors = core.errors.load(Ordering::Relaxed);
    let warnings = core.warnings.load(Ordering::Relaxed);
    let rss = core.rss.load(Ordering::Relaxed);
    let memory = if rss == 0 {
        "Memory —".to_owned()
    } else {
        format!("Memory {} MiB", rss / 1_048_576)
    };
    let dropped = core.dropped.load(Ordering::Relaxed);
    let counts = format!("Errors {errors}  Warnings {warnings}");
    let health_color = if status.failed {
        Color::Yellow
    } else if address.is_some() {
        Color::Green
    } else {
        Color::Blue
    };
    let status_line = columns(&format!("● {state}"), &counts, width);
    let status_prefix = status_line
        .strip_suffix(&counts)
        .unwrap_or(&status_line)
        .to_owned();
    let location = columns(
        &address.unwrap_or_else(|| "Browser available when ready".to_owned()),
        &memory,
        width,
    );
    let shortcuts = if dropped > 0 {
        format!("{dropped} diagnostic events dropped · l Logs · ? Help · q Stop")
    } else if width >= 80 {
        "o Open browser   e Errors   l Logs   i Info   ? Help   q Stop".to_owned()
    } else {
        "o Open  e Errors  l Logs  i Info  ? Help  q Stop".to_owned()
    };
    vec![
        Line::styled("─".repeat(usize::from(width)), style(Color::DarkGray)),
        Line::from(vec![
            Span::styled(status_prefix, style(health_color)),
            Span::styled(
                format!("Errors {errors}"),
                if errors > 0 {
                    severity("error")
                } else {
                    style(Color::DarkGray)
                },
            ),
            Span::raw("  "),
            Span::styled(
                format!("Warnings {warnings}"),
                if warnings > 0 {
                    severity("warn")
                } else {
                    style(Color::DarkGray)
                },
            ),
        ]),
        Line::raw(location),
        Line::styled(shortcuts, style(Color::DarkGray)),
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

fn information(core: &Core, model: &Model, status: &Status) -> Vec<Line<'static>> {
    let diagnostic_file =
        std::env::var("BLUINK_DIAGNOSTICS_FILE").unwrap_or_else(|_| "Not configured".to_owned());
    let diagnostic_file =
        if diagnostic_file != "Not configured" && !crate::diagnostics::file_capture_active() {
            "Unavailable · file logging disabled".to_owned()
        } else {
            diagnostic_file
        };
    vec![
        Line::styled("Session information · Esc Close", style(Color::Blue)),
        Line::raw(format!(
            "Bluink {} · Node {} · {} / {}",
            text(&core.options.cli_version),
            text(&core.options.node_version),
            std::env::consts::OS,
            std::env::consts::ARCH
        )),
        Line::raw(format!(
            "PID {} · Uptime {} s",
            std::process::id(),
            core.started.elapsed().as_secs()
        )),
        Line::raw(format!("Project {}", text(&status.project))),
        Line::raw(format!(
            "Address {}",
            core.address().unwrap_or_else(|| "Not listening".to_owned())
        )),
        Line::raw(format!(
            "{} · {}/{} · {} pending",
            status.phase, status.processed, status.total, status.pending
        )),
        Line::raw(format!("Diagnostics {}", text(&diagnostic_file))),
        Line::raw("Memory is process RSS: Node + Rust; separate exporters excluded."),
        Line::raw("Memory samples arrive every five seconds."),
        Line::raw(format!(
            "History: {} entries evicted; {} events dropped",
            model.history.evicted,
            core.dropped.load(Ordering::Relaxed)
        )),
    ]
}

fn body(core: &Core, model: &Model, status: &Status, rows: usize) -> Vec<Line<'static>> {
    match model.view {
        View::Status => vec![],
        View::Help => [
            "Keyboard shortcuts",
            "o  Open browser                 q / Ctrl+C  Stop Bluink",
            "l  Recent logs                  e  Recent errors",
            "i  Session information          ?  Help",
            "↑ / ↓  Select / scroll          Page Up / Down  Move a page",
            "Home / End  First / newest      Enter  Toggle event details",
            "Esc  Return to status",
            "Counts cover this session; old errors do not imply server failure.",
        ]
        .into_iter()
        .map(Line::raw)
        .collect(),
        View::Info => information(core, model, status),
        View::Logs | View::Errors => {
            let entries = model.entries();
            let title = if model.view == View::Errors {
                "Errors"
            } else {
                "Logs"
            };
            let mut lines = vec![Line::styled(
                format!("{title} · ↑↓ Select · Enter Details · Esc Close"),
                style(Color::Blue),
            )];
            if entries.is_empty() {
                lines.push(Line::raw(format!(
                    "No {} recorded in retained history",
                    title.to_lowercase()
                )));
                return lines;
            }
            let index = entries
                .iter()
                .position(|entry| Some(entry.id) == model.selected)
                .unwrap_or(entries.len() - 1);
            if model.details {
                lines.push(Line::raw(entries[index].summary.clone()));
                // Paragraph wrapping below keeps long fields inside the viewport.
                lines.push(Line::raw(entries[index].details.clone()));
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
        KeyCode::Char('i') => model.open(View::Info),
        KeyCode::Char('?') => model.open(View::Help),
        KeyCode::Esc => model.open(View::Status),
        KeyCode::Enter if matches!(model.view, View::Logs | View::Errors) => {
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
    let mut renderer = if interactive(&core.options) {
        size()
            .ok()
            .filter(|(width, height)| *width >= 60 && *height >= 12)
            .and_then(|dimensions| Renderer::new(4, dimensions).ok())
    } else {
        None
    };
    let mut model = Model::default();
    let mut previous_phase = String::new();
    let mut project_shown = false;
    let mut previous_address = None;
    while !core.stop_requested.load(Ordering::Acquire) || !incoming.is_empty() {
        for event in incoming.try_iter().take(128) {
            let line = summary(&event);
            model.push(&event);
            if let Some(renderer) = &mut renderer {
                renderer.log(&line, &event.level)?;
            } else {
                writeln!(output, "{line}")?;
            }
        }
        let status = core.status();
        let address = core.address();
        if !project_shown && !status.project.is_empty() {
            let project = std::path::Path::new(&status.project)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            let introduction = format!("Project: {}", text(&project));
            if let Some(renderer) = &mut renderer {
                renderer.log(&introduction, "info")?;
            } else {
                writeln!(output, "{introduction}")?;
            }
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
            let phase = if !status.failed && status.total > 0 && status.processed >= status.total {
                format!(
                    "{}: {}/{} (100%)",
                    status.phase, status.processed, status.total
                )
            } else {
                status.phase.clone()
            };
            if !phase.is_empty() && phase != previous_phase {
                writeln!(output, "{phase}")?;
                previous_phase = phase;
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
        renderer.finish("Bluink stopped")?;
    } else {
        writeln!(output, "Bluink stopped")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    #[test]
    fn footer_keeps_address_counts_memory_and_controls_visible_at_supported_widths() {
        let (core, _) = super::super::tests::fixture();
        *core.address.lock().unwrap() = Some("http://127.0.0.1:9000".to_owned());
        let status = Status {
            ready: true,
            phase: "Search up to date".to_owned(),
            ..Status::default()
        };
        for width in [60, 80, 120] {
            let mut terminal = Terminal::new(TestBackend::new(width, 4)).unwrap();
            terminal
                .draw(|frame| draw(frame, &core, &Model::default(), &status))
                .unwrap();
            let output = format!("{:?}", terminal.backend());
            for expected in [
                "Running",
                "Errors 0",
                "Warnings 0",
                "Memory 184 MiB",
                "http://127.0.0.1:9000",
                "q Stop",
            ] {
                assert!(output.contains(expected), "{width}: {output}");
            }
        }
    }
}
