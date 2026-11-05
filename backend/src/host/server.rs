// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! `bluink server`: one application, the HTTP listener and the terminal
//! console, stopped once by a signal, the console's Quit or a `stop` line.

use std::io::BufRead;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Map, Value};

use super::{Lifecycle, Outcome, memory, output, resources};
use crate::application::ApplicationOptions;
use crate::console::{BluinkConsole, ConsoleOptions};
use crate::host::session::ApplicationHandle;

/// The console outlives the application so final diagnostics still render.
pub(super) struct Console {
    console: Arc<BluinkConsole>,
    sampling: Arc<AtomicBool>,
}

impl Console {
    pub(super) fn stop(self, runtime: &tokio::runtime::Runtime) -> anyhow::Result<()> {
        self.sampling.store(false, Ordering::Release);
        let result = runtime.block_on(self.console.stop());
        output::set_console_active(false);
        result
    }
}

fn project_root(values: &Map<String, Value>) -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    Ok(match values.get("project").and_then(Value::as_str) {
        Some(project) => cwd.join(project),
        None => std::env::var_os("BLUINK_PROJECT_DIR").map_or(cwd, PathBuf::from),
    })
}

fn options(values: &Map<String, Value>) -> Result<ApplicationOptions, String> {
    let resources = resources::Resources::locate()?;
    Ok(ApplicationOptions {
        project_root: project_root(values)?.to_string_lossy().into_owned(),
        workspace_template_path: resources.workspace_starter().to_string_lossy().into_owned(),
        timezone: None,
        public_dir: Some(resources.web_directory()?),
        user_data_dir: Some(resources::user_data_dir()?),
        web_font_folder: Some(resources.font_directory("web")?),
        pdf_font_folder: Some(resources.font_directory("pdf")?),
        docx_font_folder: Some(resources.font_directory("docx")?),
        typst_binary: Some(resources.typst_binary()?),
    })
}

/// The port is validated before anything is opened.
fn port(values: &Map<String, Value>) -> Result<u32, String> {
    values
        .get("port")
        .and_then(Value::as_str)
        .and_then(|port| port.parse::<u16>().ok())
        .map(u32::from)
        .ok_or_else(|| "Choose a port number from 0 to 65535.".to_owned())
}

/// A clearer error when the listener's port is taken.
fn explain_port_in_use(error: &str, port: u32) -> String {
    let taken = ["os error 10048", "os error 98", "os error 48"]
        .iter()
        .any(|code| {
            error
                .split(code)
                .nth(1)
                .is_some_and(|rest| !rest.starts_with(|character: char| character.is_ascii_digit()))
        })
        || error
            .to_ascii_lowercase()
            .contains("address already in use");
    if !taken {
        return error.to_owned();
    }
    let (shell, command) = if cfg!(windows) {
        (
            "PowerShell",
            format!(
                "Get-NetTCPConnection -LocalPort {port} -State Listen | ForEach-Object {{ Stop-Process -Id $_.OwningProcess }}"
            ),
        )
    } else {
        (
            "a terminal",
            format!("lsof -ti tcp:{port} -sTCP:LISTEN | xargs kill"),
        )
    };
    format!(
        "Port {port} is already in use, probably by an earlier Bluink server.\nStop it, then start again. In {shell}, run:\n  {command}\nOr choose another port with --port."
    )
}

/// OS launcher arguments are passed directly; URLs are never shell commands.
fn browser_command(url: &str) -> Result<(&'static str, Vec<String>), String> {
    let address = url::Url::parse(url).map_err(|error| error.to_string())?;
    if !matches!(address.scheme(), "http" | "https") {
        return Err("The browser address must use HTTP or HTTPS.".to_owned());
    }
    let href = address.to_string();
    Ok(if cfg!(target_os = "macos") {
        ("open", vec![href])
    } else if cfg!(windows) {
        (
            "rundll32",
            vec!["url.dll,FileProtocolHandler".to_owned(), href],
        )
    } else {
        ("xdg-open", vec![href])
    })
}

fn open_browser(url: &str) {
    let opened = browser_command(url).and_then(|(command, arguments)| {
        std::process::Command::new(command)
            .args(arguments)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|error| error.to_string())
            .and_then(|status| {
                status
                    .success()
                    .then_some(())
                    .ok_or_else(|| "Browser launcher failed.".to_owned())
            })
    });
    if opened.is_err() {
        tracing::error!(target: "bluink.host", event = "browser_open_failed",
            "Could not open the browser; use the displayed address");
    }
}

/// This project's identity, read without opening it for writing. None for a
/// folder that holds no Bluink project yet.
fn project_id(options: &ApplicationOptions) -> Option<String> {
    let database = std::path::Path::new(&options.project_root).join(crate::store::DATABASE_NAME);
    let connection =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    connection
        .query_row(
            "SELECT project_uuid FROM project_state WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .ok()
}

/// The address of a Bluink already serving this project on `port`, if any.
/// Anything else on the port is left to the normal port-in-use explanation.
fn running_instance(options: &ApplicationOptions, port: u32) -> Option<String> {
    if port == 0 {
        return None;
    }
    let expected = project_id(options)?;
    let url = format!("http://127.0.0.1:{port}");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .ok()?;
    let context: Value = client
        .post(format!("{url}/api/read_workspace_context"))
        .body("{}")
        .send()
        .ok()?
        .json()
        .ok()?;
    (context["projectId"].as_str() == Some(expected.as_str())).then_some(url)
}

/// Without an interactive terminal, a `stop` line on stdin stops the server
/// gracefully. Windows cannot signal a child that does not share its console.
fn watch_stdin(lifecycle: &Arc<Lifecycle>) {
    let owner = Arc::clone(lifecycle);
    let _ = std::thread::Builder::new()
        .name("bluink-stdin".to_owned())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { return };
                if line.trim() == "stop" {
                    owner.request(true);
                }
            }
        });
}

fn sample_memory(console: &Arc<BluinkConsole>, sampling: &Arc<AtomicBool>) {
    console.update_rss(memory::resident_bytes());
    let console = Arc::clone(console);
    let sampling = Arc::clone(sampling);
    let _ = std::thread::Builder::new()
        .name("bluink-memory".to_owned())
        .spawn(move || {
            while sampling.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_secs(5));
                console.update_rss(memory::resident_bytes());
            }
        });
}

pub(super) fn run(
    runtime: &tokio::runtime::Runtime,
    lifecycle: &Arc<Lifecycle>,
    values: &Map<String, Value>,
) -> Outcome {
    let port = match port(values) {
        Ok(port) => port,
        Err(error) => return Outcome::done(Some(anyhow::Error::msg(error)), false),
    };
    let options = match options(values) {
        Ok(options) => options,
        Err(error) => return Outcome::done(Some(anyhow::Error::msg(error)), false),
    };
    let open = values.get("no-open") != Some(&Value::Bool(true));
    // A second launch for the same project reuses the running Bluink rather
    // than failing on its port.
    if let Some(url) = running_instance(&options, port) {
        if open {
            open_browser(&format!("{url}/"));
        }
        output::line(&format!(
            "Bluink is already running for this project at {url}/{}",
            if open {
                " and is now open in your browser."
            } else {
                "."
            }
        ));
        return Outcome::done(None, false);
    }
    let plain = values.get("no-tui") == Some(&Value::Bool(true));
    let console = match BluinkConsole::new(ConsoleOptions {
        cli_version: resources::version(),
        mode: Some(if plain { "plain" } else { "auto" }.to_owned()),
    }) {
        Ok(console) => Arc::new(console),
        Err(error) => return Outcome::done(Some(error), false),
    };
    output::set_console_active(true);
    let sampling = Arc::new(AtomicBool::new(true));
    sample_memory(&console, &sampling);
    if !console.interactive() {
        watch_stdin(lifecycle);
    }
    // Only an interactive console opens the browser; scripts and supervisors
    // use plain mode and decide for themselves.
    let open = open && console.interactive();
    let mut outcome = runtime.block_on(serve(lifecycle, &console, options, port, open));
    outcome.console = Some(Console { console, sampling });
    outcome
}

async fn serve(
    lifecycle: &Arc<Lifecycle>,
    console: &Arc<BluinkConsole>,
    options: ApplicationOptions,
    port: u32,
    open: bool,
) -> Outcome {
    let application = match ApplicationHandle::new(options) {
        Ok(application) => Arc::new(application),
        Err(error) => return Outcome::done(Some(error), false),
    };
    if let Err(error) = console.attach_application(&application) {
        return Outcome::done(Some(error), false);
    }
    // Stop is armed before initialization, so a signal during startup still
    // drains the project and the listener exactly once.
    let stopper = {
        let application = Arc::clone(&application);
        let lifecycle = Arc::clone(lifecycle);
        tokio::spawn(async move {
            lifecycle.requested().await;
            application.stop().await
        })
    };
    let failure = Arc::new(AtomicBool::new(false));
    let actions = {
        let lifecycle = Arc::clone(lifecycle);
        let failure = Arc::clone(&failure);
        let console = Arc::clone(console);
        tokio::spawn(async move {
            while !lifecycle.stopping() {
                match console.next_action().await {
                    Some(action) if action.kind == "shutdown" => {
                        lifecycle.request(false);
                    }
                    Some(action) if action.kind == "openBrowser" => {
                        if let Some(url) = action.url {
                            tokio::task::spawn_blocking(move || open_browser(&url));
                        }
                    }
                    Some(_) => {}
                    None => {
                        if !lifecycle.stopping() {
                            tracing::error!(target: "bluink.host", event = "console_action_failed",
                                "Console action failed; stopping Bluink");
                            failure.store(true, Ordering::Release);
                            lifecycle.request(false);
                        }
                        return;
                    }
                }
            }
        })
    };
    let started = async {
        application.initialize(true).await?;
        if lifecycle.stopping() {
            return Ok::<(), anyhow::Error>(());
        }
        let address = application
            .start_http(Some(port))
            .await
            .map_err(|error| anyhow::Error::msg(explain_port_in_use(&error.to_string(), port)))?;
        if open {
            let url = format!("http://{address}/");
            tokio::task::spawn_blocking(move || open_browser(&url));
        }
        Ok(())
    };
    let error = match started.await {
        Ok(()) => {
            lifecycle.requested().await;
            None
        }
        Err(error) => {
            if output::console_active() && !lifecycle.stopping() {
                output::error(&format!("{error:#}"));
            }
            Some(error)
        }
    };
    lifecycle.request(false);
    let cleanup = stopper.await;
    actions.abort();
    let cleanup_failed = !matches!(cleanup, Ok(Ok(())));
    let error = error.or_else(|| {
        failure
            .load(Ordering::Acquire)
            .then(|| anyhow::Error::msg("The console stopped unexpectedly."))
    });
    Outcome::done(error, cleanup_failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_conflicts_explain_how_to_free_the_port() {
        let message = explain_port_in_use(
            "Only one usage of each socket address is normally permitted. (os error 10048)",
            9000,
        );
        assert!(message.starts_with("Port 9000 is already in use"));
        assert!(message.ends_with("Or choose another port with --port."));
        assert_eq!(explain_port_in_use("os error 980", 1), "os error 980");
        assert_eq!(
            explain_port_in_use("Permission denied", 1),
            "Permission denied"
        );
    }

    #[test]
    fn browser_launch_accepts_only_web_addresses() {
        let (_, arguments) = browser_command("http://127.0.0.1:9000/").unwrap();
        assert_eq!(arguments.last().unwrap(), "http://127.0.0.1:9000/");
        assert!(browser_command("file:///etc/passwd").is_err());
        assert!(browser_command("javascript:alert(1)").is_err());
    }

    #[test]
    fn ports_must_fit_sixteen_bits() {
        let values =
            |port: &str| Map::from_iter([("port".to_owned(), Value::String(port.to_owned()))]);
        assert_eq!(port(&values("0")).unwrap(), 0);
        assert!(port(&values("65536")).is_err());
        assert!(port(&values("http")).is_err());
    }
}
