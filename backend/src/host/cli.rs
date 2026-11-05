// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Command-line grammar and help. Commands, options and their help text are
//! the user-facing contract; parsing follows Node's strict `parseArgs`.

use serde_json::{Map, Value};

use super::output;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Flag,
    Text,
}

pub(super) struct Opt {
    pub(super) name: &'static str,
    pub(super) short: Option<char>,
    pub(super) kind: Kind,
    pub(super) default: Option<&'static str>,
    pub(super) help: &'static str,
}

pub(super) struct Command {
    pub(super) name: &'static str,
    pub(super) description: &'static str,
    pub(super) options: &'static [Opt],
    /// The positional placeholder shown in usage, when the command takes one.
    pub(super) positional: Option<&'static str>,
}

const fn flag(name: &'static str, help: &'static str) -> Opt {
    Opt {
        name,
        short: None,
        kind: Kind::Flag,
        default: None,
        help,
    }
}

const fn text(name: &'static str, help: &'static str) -> Opt {
    Opt {
        name,
        short: None,
        kind: Kind::Text,
        default: None,
        help,
    }
}

const PROJECT: Opt = Opt {
    short: Some('P'),
    ..text(
        "project",
        "Existing Bluink project folder (default: current directory)",
    )
};
const EXPORT_PROJECT: Opt = Opt {
    short: Some('P'),
    ..text("project", "Existing Bluink project folder")
};
const SOURCE: Opt = text("source", "Source folder");
const DESTINATION: Opt = text("destination", "New destination folder");
const TRUST_THEME: Opt = text(
    "trust-theme-sha256",
    "Exact reviewed project-theme package hash",
);

pub(super) const GLOBAL: &[Opt] = &[
    Opt {
        short: Some('h'),
        default: Some("false"),
        ..flag("help", "Show help")
    },
    Opt {
        short: Some('v'),
        default: Some("false"),
        ..flag("version", "Show version")
    },
];

pub(super) const COMMANDS: &[Command] = &[
    Command {
        name: "server",
        description: "Start Bluink and open it in your browser",
        options: &[
            Opt {
                default: Some("false"),
                ..flag(
                    "no-tui",
                    "Stream plain server output without interactive controls",
                )
            },
            Opt {
                default: Some("false"),
                ..flag(
                    "no-open",
                    "Don't open Bluink in your browser when it's ready",
                )
            },
            Opt {
                short: Some('P'),
                ..text(
                    "project",
                    "Bluink project folder (default: current directory)",
                )
            },
            Opt {
                short: Some('p'),
                default: Some("9000"),
                ..text("port", "Port number to listen on (default: 9000)")
            },
        ],
        positional: None,
    },
    Command {
        name: "import",
        description: "Install a portable project into a new local database",
        options: &[
            SOURCE,
            DESTINATION,
            flag(
                "git",
                "Read the source as a Git repository and preserve its history",
            ),
        ],
        positional: None,
    },
    Command {
        name: "backup",
        description: "Create a full backup of the database, local history and attachments",
        options: &[PROJECT, DESTINATION],
        positional: None,
    },
    Command {
        name: "rebuild-projections",
        description: "Rebuild search and query projections from saved project data",
        options: &[PROJECT],
        positional: None,
    },
    Command {
        name: "export-project",
        description: "Export current saved records, settings and attachments as a portable folder",
        options: &[PROJECT, DESTINATION],
        positional: None,
    },
    Command {
        name: "restore-backup",
        description: "Restore a verified full backup into a new local project",
        options: &[SOURCE, DESTINATION],
        positional: None,
    },
    Command {
        name: "history",
        description: "List project checkpoints, with optional page scope and pagination",
        options: &[
            PROJECT,
            text("item", "Only checkpoints affecting this item ID"),
            text("before", "Continue before this checkpoint ID"),
            text("limit", "Maximum checkpoints to return"),
        ],
        positional: None,
    },
    Command {
        name: "restore",
        description: "Preview a checkpoint restore; apply only with the reviewed token",
        options: &[
            PROJECT,
            text("checkpoint", "Checkpoint commit ID to restore"),
            text("item", "Restore only this item ID"),
            flag(
                "confirm",
                "Apply the restore identified by the review token",
            ),
            text(
                "review-token",
                "reviewToken from the reviewed restore preview",
            ),
        ],
        positional: None,
    },
    Command {
        name: "sync",
        description: "Sync with the configured destination, configure a destination, or open a separate project copy",
        options: &[
            PROJECT,
            flag("status", "Show current sync state without syncing"),
            text(
                "origin",
                "Connect an absolute local repository path, SSH address or HTTPS URL",
            ),
            flag(
                "create",
                "With --origin, create an empty local bare repository",
            ),
            flag(
                "disconnect",
                "Disconnect the destination, retaining local history",
            ),
            text(
                "open-from",
                "Clone this origin instead of syncing the current project",
            ),
            text("destination", "New project folder for --open-from"),
        ],
        positional: None,
    },
    Command {
        name: "export",
        description: "Export one note as a PDF, Word, EPUB or HTML file",
        options: &[
            EXPORT_PROJECT,
            text("note", "Page ID to export as an article"),
            flag(
                "include-linked",
                "Follow internal page links; HTML produces a website",
            ),
            text(
                "link-depth",
                "With --include-linked: direct (default), two-levels or reachable",
            ),
            text(
                "outside-links",
                "Links outside the export: error or text-only",
            ),
            text("profile", "PDF profile: default or accessible (PDF/UA-1)"),
            Opt {
                short: Some('o'),
                ..text("out", "Output file path")
            },
            flag("check", "Validate without generating a file"),
            text("theme", "Publication theme for this note"),
            TRUST_THEME,
        ],
        positional: Some("format"),
    },
    Command {
        name: "publication",
        description: "Create, inspect, plan, or generate a Publication",
        options: &[
            EXPORT_PROJECT,
            text("id", "Publication item ID for inspect, plan, or generate"),
            text("input", "JSON file for create"),
            text(
                "format",
                "pdf, docx, epub, html, or site, required for plan or generate",
            ),
            text("output", "New PDF/Word/HTML file or site folder"),
            TRUST_THEME,
        ],
        positional: Some("create|inspect|plan|generate"),
    },
    Command {
        name: "build-info",
        description: "Print this executable's embedded build identity",
        options: &[],
        positional: None,
    },
];

/// What the arguments ask for, before any project or resource is touched.
pub(super) enum Invocation {
    Version,
    Help(Option<&'static Command>),
    /// Usage was printed because no command was named; exit with failure.
    Missing,
    Run {
        command: &'static Command,
        values: Map<String, Value>,
        positionals: Vec<String>,
    },
}

pub(super) fn parse(arguments: &[String]) -> Result<Invocation, String> {
    // Global flags count only before the command name, so `--help` after a
    // command shows that command's help rather than the overall usage.
    let index = arguments
        .iter()
        .position(|argument| !argument.starts_with('-'));
    let name = match index {
        None if arguments.is_empty() => Some("server"),
        None => None,
        Some(index) => Some(arguments[index].as_str()),
    };
    let before = index.map_or(arguments, |index| &arguments[..index]);
    let global = lenient(before);
    if global.contains("version") {
        return Ok(Invocation::Version);
    }
    let Some(name) = name else {
        return Ok(if global.contains("help") {
            Invocation::Help(None)
        } else {
            Invocation::Missing
        });
    };
    if global.contains("help") {
        return Ok(Invocation::Help(None));
    }
    let command = COMMANDS
        .iter()
        .find(|command| command.name == name)
        .ok_or_else(|| format!("Unknown command: '{name}'"))?;
    let rest = index.map_or(&[][..], |index| &arguments[index + 1..]);
    let (values, positionals) = strict(command, rest)?;
    if values.get("help") == Some(&Value::Bool(true)) {
        return Ok(Invocation::Help(Some(command)));
    }
    Ok(Invocation::Run {
        command,
        values,
        positionals,
    })
}

/// Pre-command flags are matched leniently: unknown flags are ignored.
fn lenient(arguments: &[String]) -> std::collections::BTreeSet<&'static str> {
    arguments
        .iter()
        .filter_map(|argument| {
            GLOBAL
                .iter()
                .find(|option| {
                    argument.strip_prefix("--") == Some(option.name)
                        || option
                            .short
                            .is_some_and(|short| argument == &format!("-{short}"))
                })
                .map(|option| option.name)
        })
        .collect()
}

fn find<'a>(command: &'static Command, matches: impl Fn(&Opt) -> bool) -> Option<&'a Opt> {
    command
        .options
        .iter()
        .chain(GLOBAL)
        .find(|option| matches(option))
}

fn strict(
    command: &'static Command,
    arguments: &[String],
) -> Result<(Map<String, Value>, Vec<String>), String> {
    let mut values = Map::new();
    let mut positionals = Vec::new();
    let mut pending = arguments.iter();
    while let Some(argument) = pending.next() {
        if argument == "--" {
            positionals.extend(pending.by_ref().cloned());
            break;
        }
        let (option, inline) = if let Some(long) = argument.strip_prefix("--") {
            let (name, inline) = long
                .split_once('=')
                .map_or((long, None), |(name, value)| (name, Some(value.to_owned())));
            let option = find(command, |option| option.name == name)
                .ok_or_else(|| unknown(argument.split('=').next().unwrap_or(argument)))?;
            (option, inline)
        } else if argument.len() > 1 && argument.starts_with('-') {
            let mut characters = argument[1..].chars();
            let short = characters.next();
            let option = find(command, |option| option.short == short)
                .ok_or_else(|| unknown(&argument[..2.min(argument.len())]))?;
            let inline = Some(characters.as_str())
                .filter(|rest| !rest.is_empty())
                .map(str::to_owned);
            (option, inline)
        } else {
            if command.positional.is_none() {
                return Err(format!(
                    "Unexpected argument '{argument}'. This command does not take positional arguments"
                ));
            }
            positionals.push(argument.clone());
            continue;
        };
        let value = match (option.kind, inline) {
            (Kind::Flag, None) => Value::Bool(true),
            (Kind::Flag, Some(_)) => {
                return Err(format!(
                    "Option '--{}' does not take an argument",
                    option.name
                ));
            }
            (Kind::Text, Some(value)) => Value::String(value),
            (Kind::Text, None) => match pending.next() {
                Some(value) if !value.starts_with('-') || value == "-" => {
                    Value::String(value.clone())
                }
                _ => {
                    return Err(format!(
                        "Option '{}--{} <value>' argument missing",
                        option
                            .short
                            .map_or_else(String::new, |short| format!("-{short}, ")),
                        option.name
                    ));
                }
            },
        };
        values.insert(option.name.to_owned(), value);
    }
    for option in command.options.iter().chain(GLOBAL) {
        if let Some(default) = option.default
            && !values.contains_key(option.name)
        {
            let value = match option.kind {
                Kind::Flag => Value::Bool(default == "true"),
                Kind::Text => Value::String(default.to_owned()),
            };
            values.insert(option.name.to_owned(), value);
        }
    }
    Ok((values, positionals))
}

fn unknown(option: &str) -> String {
    format!(
        "Unknown option '{option}'. To specify a positional argument starting with a '-', place it at the end of the command after '--', as in '-- \"{option}\"'"
    )
}

fn options(options: &[Opt], version: &str) {
    for option in options {
        let short = option
            .short
            .map_or_else(|| "    ".to_owned(), |short| format!("-{short}, "));
        let help = if option.name == "version" {
            format!("{} ({version})", option.help)
        } else {
            option.help.to_owned()
        };
        output::line(&format!("  {short}--{:<14}{help}", option.name));
    }
}

pub(super) fn overall_help(version: &str) {
    output::line("Usage: bluink [options] <command> [command-options]");
    output::line("");
    output::line("Global options:");
    options(GLOBAL, version);
    output::line("");
    output::line("Commands:");
    for command in COMMANDS {
        output::line(&format!("  {:<21}{}", command.name, command.description));
    }
    output::line("");
    output::line("Run 'bluink <command> --help' for command-specific options.");
}

pub(super) fn command_help(command: &Command, version: &str) {
    let positional = command
        .positional
        .map_or_else(String::new, |name| format!(" <{name}>"));
    output::line(&format!(
        "Usage: bluink {}{positional} [options]",
        command.name
    ));
    output::line("");
    output::line(command.description);
    if !command.options.is_empty() {
        output::line("");
        output::line("Options:");
        options(command.options, version);
    }
    output::line("");
    output::line("Global options:");
    options(GLOBAL, version);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(arguments: &[&str]) -> Result<Invocation, String> {
        parse(
            &arguments
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn no_arguments_start_the_server_with_defaults() {
        let Ok(Invocation::Run {
            command, values, ..
        }) = run(&[])
        else {
            panic!("expected the server command");
        };
        assert_eq!(command.name, "server");
        assert_eq!(values["port"], "9000");
        assert_eq!(values["no-tui"], false);
        assert_eq!(values["help"], false);
    }

    #[test]
    fn global_flags_apply_only_before_the_command() {
        assert!(matches!(run(&["--version"]), Ok(Invocation::Version)));
        assert!(matches!(run(&["-h"]), Ok(Invocation::Help(None))));
        assert!(matches!(run(&["--unknown"]), Ok(Invocation::Missing)));
        assert!(matches!(
            run(&["export", "--help"]),
            Ok(Invocation::Help(Some(command))) if command.name == "export"
        ));
    }

    #[test]
    fn strict_options_accept_inline_and_separate_values() {
        let Ok(Invocation::Run {
            values,
            positionals,
            ..
        }) = run(&[
            "export",
            "pdf",
            "--note=abc",
            "-o",
            "out.pdf",
            "--check",
            "--",
            "--raw",
        ])
        else {
            panic!("expected export");
        };
        assert_eq!(values["note"], "abc");
        assert_eq!(values["out"], "out.pdf");
        assert_eq!(values["check"], true);
        assert_eq!(positionals, ["pdf", "--raw"]);
        let Ok(Invocation::Run { values, .. }) = run(&["server", "-p0"]) else {
            panic!("expected server");
        };
        assert_eq!(values["port"], "0");
    }

    #[test]
    fn strict_parsing_reports_unknown_missing_and_unexpected_arguments() {
        let error = |arguments: &[&str]| run(arguments).err().unwrap_or_default();
        assert!(error(&["unknown-command"]).contains("Unknown command: 'unknown-command'"));
        assert!(error(&["server", "--bogus"]).starts_with("Unknown option '--bogus'"));
        assert!(error(&["server", "--port"]).contains("argument missing"));
        assert!(error(&["server", "extra"]).starts_with("Unexpected argument 'extra'"));
        assert!(error(&["sync", "--status=yes"]).contains("does not take an argument"));
    }
}
