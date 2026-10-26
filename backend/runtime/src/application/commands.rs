// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! CLI domain work. Argument parsing and stdout presentation remain in Node.
use std::path::{Component, PathBuf};

use crate::engine::publication::{self, page::plan_page, plan::PublicationFormat};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::Application;
use super::error::{Result, internal_error, invalid_request};
use crate::export;

#[derive(Deserialize)]
pub(crate) struct CommandRequest {
    pub(crate) positionals: Vec<String>,
    pub(crate) values: Map<String, Value>,
    pub(crate) cwd: PathBuf,
}

#[derive(Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(crate) enum CommandResult {
    PagePlan(Box<publication::page::PagePlan>),
    PublicationPlan(Box<publication::plan::PublicationPlan>),
    PublicationGenerated(export::Generation),
    Item(Value),
    Text(String),
}

impl CommandRequest {
    fn flag(&self, name: &str) -> Result<bool> {
        match self.values.get(name) {
            None => Ok(false),
            Some(Value::Bool(value)) => Ok(*value),
            _ => Err(invalid_request(format!("--{name} must be a boolean."))),
        }
    }

    fn string(&self, name: &str, message: &str) -> Result<Option<&str>> {
        self.values
            .get(name)
            .map(|value| value.as_str().ok_or_else(|| invalid_request(message)))
            .transpose()
    }

    fn required(&self, name: &str, message: &str) -> Result<&str> {
        self.string(name, message)?
            .filter(|value| !value.is_empty())
            .ok_or_else(|| invalid_request(message))
    }

    fn trust(&self) -> Result<Option<&str>> {
        let message = "--trust-theme-sha256 needs a 64-character lowercase hash.";
        let hash = self.string("trust-theme-sha256", message)?;
        if hash.is_some_and(|hash| {
            hash.len() != 64
                || !hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }) {
            return Err(invalid_request(message));
        }
        Ok(hash)
    }

    fn path(&self, value: &str) -> PathBuf {
        let path = self.cwd.join(value);
        let mut result = PathBuf::new();
        for component in path.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    result.pop();
                }
                component => result.push(component.as_os_str()),
            }
        }
        result
    }
}

impl Application {
    pub(crate) fn export_resources(&self) -> export::Resources<'_> {
        export::Resources {
            web_fonts: self.options.web_font_folder.as_deref(),
            pdf_fonts: self.options.pdf_font_folder.as_deref(),
            docx_fonts: self.options.docx_font_folder.as_deref(),
            typst: self.options.typst_binary.as_deref(),
        }
    }

    #[tracing::instrument(level = "debug", skip_all, fields(command))]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn execute_command(
        &self,
        command: &str,
        request: CommandRequest,
    ) -> Result<CommandResult> {
        self.check_running()?;
        if !matches!(command, "export" | "publication") {
            return Err(invalid_request("Unknown application command."));
        }
        if !request.cwd.is_absolute() {
            return Err(invalid_request(
                "The command working folder must be absolute.",
            ));
        }
        request.string("project", "--project needs a folder path.")?;
        if !self.root.join("bluink.json").is_file() {
            return Err(invalid_request(format!(
                "No Bluink project exists at {}.",
                self.root.display()
            )));
        }
        self.initialize_sync(false)?;
        match command {
            "export" => self.command_export(&request),
            "publication" => self.command_publication(&request),
            _ => unreachable!(),
        }
    }

    fn command_export(&self, request: &CommandRequest) -> Result<CommandResult> {
        let _operation = self.begin_operation()?;
        let format = match request.positionals.as_slice() {
            [format] if matches!(format.as_str(), "pdf" | "docx" | "epub" | "html") => {
                PublicationFormat::parse(format).map_err(invalid_request)?
            }
            _ => {
                return Err(invalid_request(
                    "Usage: bluink export <pdf|docx|epub|html> --note <page-id> [options]. Use bluink publication for books and sites.",
                ));
            }
        };
        let id = request.required("note", "Standalone export requires --note with a page ID.")?;
        if !crate::engine::project::identity::is_item_id(id) {
            return Err(invalid_request(format!("Invalid Bluink item id: {id}")));
        }
        let outside = export::outside_links(
            request
                .string(
                    "outside-links",
                    "--outside-links must be error or text-only.",
                )?
                .unwrap_or("error"),
        )?;
        let theme = request
            .string("theme", "--theme needs a theme ID.")?
            .unwrap_or("classic");
        if theme.is_empty() {
            return Err(invalid_request("--theme needs a theme ID."));
        }
        let profile = if format == PublicationFormat::Pdf {
            export::pdf_profile(
                request
                    .string("profile", "--profile must be default or accessible.")?
                    .unwrap_or("default"),
            )?
        } else {
            if request.values.contains_key("profile") {
                return Err(invalid_request("--profile applies only to PDF."));
            }
            publication::PdfProfile::Default
        };
        let output = if request.flag("check")? {
            None
        } else {
            Some(
                request
                    .path(request.required("out", "--out must specify a new output file path.")?),
            )
        };
        let trust = request.trust()?;
        let linked = request.flag("include-linked")?;
        if linked {
            let format = if format == PublicationFormat::Html {
                PublicationFormat::Site
            } else {
                format
            };
            let plan = export::plan_linked_pages(&self.root, id, format, theme, outside, profile)?;
            let Some(output) = output else {
                return Ok(CommandResult::PublicationPlan(Box::new(plan)));
            };
            let generated = export::render_publication(
                &self.root,
                &plan,
                &output,
                trust,
                None,
                &self.export_resources(),
            )?;
            return Ok(CommandResult::Text(generated.output));
        }
        let plan =
            plan_page(&self.root, id, theme, outside, format, profile).map_err(internal_error)?;
        let Some(output) = output else {
            return Ok(CommandResult::PagePlan(Box::new(plan)));
        };
        let resources = self.export_resources();
        let path = match format {
            PublicationFormat::Html => {
                export::render_article(&self.root, &plan, Some(&output), trust, &resources)?
                    .output
                    .ok_or_else(|| {
                        internal_error("HTML generation did not return its output path.")
                    })?
            }
            PublicationFormat::Pdf => {
                export::render_note_pdf(&self.root, &plan, &output, trust, &resources)?.output
            }
            PublicationFormat::Epub => {
                export::render_note_epub(&self.root, &plan, &output, &resources)?.output
            }
            PublicationFormat::Docx => {
                export::render_note_docx(&self.root, &plan, &output, trust, &resources)?.output
            }
            PublicationFormat::Site => unreachable!("standalone formats were validated"),
        };
        Ok(CommandResult::Text(path))
    }

    fn command_publication(&self, request: &CommandRequest) -> Result<CommandResult> {
        let action = match request.positionals.as_slice() {
            [action] if matches!(action.as_str(), "create" | "inspect" | "plan" | "generate") => {
                action.as_str()
            }
            _ => {
                return Err(invalid_request(
                    "Usage: bluink publication <create|inspect|plan|generate> [options]",
                ));
            }
        };
        if action == "create" {
            let operation = self.begin_operation()?;
            let input = request.required("input", "Use --input with a Publication JSON file.")?;
            let value: Value = serde_json::from_slice(&std::fs::read(request.path(input))?)
                .map_err(invalid_request)?;
            let title = value["title"]
                .as_str()
                .filter(|title| !title.trim().is_empty())
                .ok_or_else(|| invalid_request("The Publication input needs a title."))?;
            let description = value.get("description");
            if description.is_some_and(|description| !description.is_string()) {
                return Err(invalid_request("The Publication description must be text."));
            }
            if !value["content"].is_object() {
                return Err(invalid_request(
                    "The Publication input needs a content object.",
                ));
            }
            let mut meta = Map::from_iter([
                ("type".to_owned(), Value::from("publication")),
                ("title".to_owned(), Value::from(title.trim())),
            ]);
            if let Some(description) = description {
                meta.insert("description".to_owned(), description.clone());
            }
            let input = super::mutations::CreateItemRequest::new(meta, value["content"].clone());
            // create_item owns canonical validation, activity and projection invalidation.
            drop(operation);
            return Ok(CommandResult::Text(self.create_item(input)?.id));
        }
        let id = request.required("id", "Use --id with a Publication item ID.")?;
        if action == "inspect" {
            let _operation = self.begin_operation()?;
            if !crate::engine::project::identity::is_item_id(id) {
                return Err(invalid_request(format!("Invalid Bluink item id: {id}")));
            }
            let item = crate::engine::project::item::read_project_item(&self.root, id)?
                .filter(|item| item["meta"]["type"] == "publication")
                .ok_or_else(|| invalid_request(format!("Publication {id} does not exist.")))?;
            return Ok(CommandResult::Item(item));
        }
        let format_message = if action == "plan" {
            "Use --format pdf, docx, epub, html, or site when planning."
        } else {
            "Use --format pdf, docx, epub, html, or site when generating a Publication."
        };
        let format = request.required("format", format_message)?;
        if !matches!(format, "pdf" | "docx" | "epub" | "html" | "site") {
            return Err(invalid_request(format_message));
        }
        let format = PublicationFormat::parse(format).map_err(invalid_request)?;
        let (output, trust) = if action == "generate" {
            (
                Some(request.path(request.required(
                    "output",
                    "Use --output with a new PDF or HTML filename, or site folder.",
                )?)),
                request.trust()?,
            )
        } else {
            (None, None)
        };
        // Initialization needs exclusive lifecycle admission. Never upgrade while
        // holding an operation read guard. Recheck the source after initialization.
        let needs_index = {
            let _operation = self.begin_operation()?;
            export::needs_index(&self.root, id)?
        };
        if needs_index {
            self.initialize_sync_deadline(
                true,
                Some(std::time::Instant::now() + std::time::Duration::from_mins(5)),
            )?;
        }
        let _operation = self.begin_operation()?;
        let index = self.state()?.index.as_ref().map(|index| index.path.clone());
        let plan = export::plan_publication(&self.root, id, format, index.as_deref())?;
        if let Some(output) = output {
            return Ok(CommandResult::PublicationGenerated(
                export::render_publication(
                    &self.root,
                    &plan,
                    &output,
                    trust,
                    index.as_deref(),
                    &self.export_resources(),
                )?,
            ));
        }
        Ok(CommandResult::PublicationPlan(Box::new(plan)))
    }
}
