// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! One-shot commands. Offline storage commands work on explicit folders;
//! application commands open the project, run once and stop it again.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value};

use super::{Lifecycle, Outcome, output, resources};
use crate::application::ApplicationOptions;
use crate::host::session::ApplicationHandle;
use crate::{store, versioning::checkpoints::portable::GitSource};

fn folder(values: &Map<String, Value>, key: &str) -> Result<PathBuf, String> {
    let value = values
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("Provide --{key} with a folder path"))?;
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    Ok(cwd.join(value))
}

/// `--project`, or the current folder.
fn project(values: &Map<String, Value>) -> Result<PathBuf, String> {
    if values.contains_key("project") {
        folder(values, "project")
    } else {
        std::env::current_dir().map_err(|error| error.to_string())
    }
}

fn import(values: &Map<String, Value>) -> Result<String, String> {
    let source = folder(values, "source")?;
    let destination = folder(values, "destination")?;
    let imported = if values.get("git") == Some(&Value::Bool(true)) {
        GitSource::open(&source)
            .map_err(|error| error.to_string())
            .and_then(|git| {
                store::portable::import(&git, &destination, |_, _, _| {})
                    .map_err(|error| error.to_string())
            })
    } else {
        store::portable::Folder::open(&source)
            .and_then(|folder| store::portable::import(&folder, &destination, |_, _, _| {}))
            .map_err(|error| error.to_string())
    };
    imported.map(|()| destination.display().to_string())
}

fn json(value: Result<impl serde::Serialize, store::Error>) -> Result<String, String> {
    value
        .and_then(|value| Ok(serde_json::to_string(&value)?))
        .map_err(|error| error.to_string())
}

fn storage(name: &str, values: &Map<String, Value>) -> Result<String, String> {
    match name {
        "import" => import(values),
        "backup" => {
            let destination = folder(values, "destination")?;
            let store =
                store::Store::shared(&project(values)?).map_err(|error| error.to_string())?;
            json(store::backup::create(&store, &destination))
        }
        "export-project" => {
            let destination = folder(values, "destination")?;
            let store =
                store::Store::shared(&project(values)?).map_err(|error| error.to_string())?;
            json(store::portable::export(&store, &destination))
        }
        "restore-backup" => {
            let source = folder(values, "source")?;
            let destination = folder(values, "destination")?;
            json(store::backup::restore(&source, &destination))
        }
        "rebuild-projections" => store::rebuild_project(&project(values)?)
            .map(|generation| format!("Rebuilt query projections at generation {generation}."))
            .map_err(|error| error.to_string()),
        _ => Err(format!("Unknown command: '{name}'")),
    }
}

fn application_options(
    name: &str,
    values: &Map<String, Value>,
) -> Result<ApplicationOptions, String> {
    let resources = resources::Resources::locate()?;
    let publishing = matches!(name, "export" | "publication");
    let font = |format: &str| {
        publishing
            .then(|| resources.font_directory(format))
            .transpose()
    };
    Ok(ApplicationOptions {
        project_root: project(values)?.to_string_lossy().into_owned(),
        workspace_template_path: resources.workspace_starter().to_string_lossy().into_owned(),
        timezone: None,
        public_dir: None,
        user_data_dir: Some(resources::user_data_dir()?),
        web_font_folder: font("web")?,
        pdf_font_folder: font("pdf")?,
        docx_font_folder: font("docx")?,
        pdf_renderer: publishing.then(resources::pdf_renderer).transpose()?,
    })
}

async fn application_command(
    lifecycle: &Arc<Lifecycle>,
    name: &str,
    values: Map<String, Value>,
    positionals: Vec<String>,
) -> Outcome {
    let options = match application_options(name, &values) {
        Ok(options) => options,
        Err(error) => return Outcome::done(Some(anyhow::Error::msg(error)), false),
    };
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => return Outcome::done(Some(error.into()), false),
    };
    let application = match ApplicationHandle::new(options) {
        Ok(application) => Arc::new(application),
        Err(error) => return Outcome::done(Some(error), false),
    };
    // A signal stops the application, which ends a running command with the
    // shutdown error; that interruption is a clean exit.
    let stopper = {
        let application = Arc::clone(&application);
        let lifecycle = Arc::clone(lifecycle);
        tokio::spawn(async move {
            lifecycle.requested().await;
            application.stop().await
        })
    };
    let request = crate::application::commands::CommandRequest {
        positionals,
        values,
        cwd,
    };
    let result = application.run_command(name, request).await;
    lifecycle.request(false);
    let cleanup_failed = !matches!(stopper.await, Ok(Ok(())));
    match result {
        Ok(output) => {
            output::line(&output);
            Outcome::done(None, cleanup_failed)
        }
        Err(error) => Outcome::done(Some(error), cleanup_failed),
    }
}

pub(super) fn run(
    runtime: &tokio::runtime::Runtime,
    lifecycle: &Arc<Lifecycle>,
    name: &str,
    values: Map<String, Value>,
    positionals: Vec<String>,
) -> Outcome {
    if matches!(
        name,
        "export" | "publication" | "history" | "restore" | "sync"
    ) {
        return runtime.block_on(application_command(lifecycle, name, values, positionals));
    }
    // Storage commands finish their atomic work even when a signal arrives.
    match storage(name, &values) {
        Ok(result) => {
            output::line(&result);
            Outcome::done(None, false)
        }
        Err(error) => Outcome::done(Some(anyhow::Error::msg(error)), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_resolve_from_the_working_directory_and_require_a_value() {
        let values = Map::from_iter([("destination".to_owned(), Value::String("out".into()))]);
        let resolved = folder(&values, "destination").unwrap();
        assert!(resolved.is_absolute() && resolved.ends_with("out"));
        assert_eq!(
            folder(&values, "source").unwrap_err(),
            "Provide --source with a folder path"
        );
        let blank = Map::from_iter([("project".to_owned(), Value::String(" ".into()))]);
        assert!(project(&blank).is_err());
        assert_eq!(
            project(&Map::new()).unwrap(),
            std::env::current_dir().unwrap()
        );
    }
}
