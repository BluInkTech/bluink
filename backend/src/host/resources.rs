// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Resources beside the executable. A checkout holds `resources/` and an npm
//! install holds `vendor/`. Installed Web and Fonts resolve through ancestor
//! `node_modules` folders exactly as Node would resolve them.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::Value;

const BUILD_INFO: &str = concat!(
    "BLUINK_NATIVE_BUILD_INFO_V1:",
    env!("BLUINK_NATIVE_BUILD_INFO"),
    ":END_BLUINK_NATIVE_BUILD_INFO"
);

/// Embedded in every executable so packaging can reject stale binaries without
/// running a foreign architecture on the build machine. The JSON is sliced from
/// inside the marked block, so each marker string appears once in the binary.
pub(super) fn build_identity() -> &'static str {
    let start = BUILD_INFO.find('{').unwrap_or(BUILD_INFO.len());
    let end = BUILD_INFO.rfind('}').map_or(start, |end| end + 1);
    BUILD_INFO.get(start..end).unwrap_or_default()
}

fn embedded_version() -> Option<String> {
    let info: Value = serde_json::from_str(env!("BLUINK_NATIVE_BUILD_INFO")).ok()?;
    info["version"].as_str().map(str::to_owned)
}

/// Source checkouts carry Cargo.toml and first-party resources. Assembled npm
/// packages retain their installed vendor/ layout and omit the Cargo manifest.
fn resource_directory(root: &Path) -> PathBuf {
    root.join(if root.join("Cargo.toml").is_file() {
        "resources"
    } else {
        "vendor"
    })
}

/// Set once by a host that loaded Bluink, whose own executable is not Bluink's.
static LOADED_FROM: OnceLock<PathBuf> = OnceLock::new();

pub(super) fn use_package_root(root: PathBuf) {
    let _ = LOADED_FROM.set(root);
}

/// Set once by Node's launcher script, which a renderer child runs again.
static LAUNCHER: OnceLock<PathBuf> = OnceLock::new();

pub(super) fn use_launcher(launcher: PathBuf) {
    let _ = LAUNCHER.set(launcher);
}

/// The PDF renderer child: this executable, or Node running the launcher, so
/// the child also loads Bluink's addon through Node.
pub(super) fn pdf_renderer() -> Result<crate::export_jobs::pdf::Renderer, String> {
    let program = std::env::current_exe()
        .map_err(|error| format!("Cannot locate the Bluink executable: {error}"))?;
    Ok(crate::export_jobs::pdf::Renderer::Child {
        program,
        arguments: LAUNCHER
            .get()
            .map(|launcher| vec![launcher.clone().into_os_string()])
            .unwrap_or_default(),
    })
}

/// The package root: `BLUINK_PACKAGE_ROOT`, the package a host loaded Bluink
/// from, or the nearest ancestor of the executable holding `package.json` and
/// the workspace starter.
pub(super) fn package_root() -> Result<PathBuf, String> {
    if let Some(root) = std::env::var_os("BLUINK_PACKAGE_ROOT") {
        return Ok(PathBuf::from(root));
    }
    if let Some(root) = LOADED_FROM.get() {
        return Ok(root.clone());
    }
    let executable = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|error| format!("Cannot locate the Bluink executable: {error}"))?;
    root_for_executable(&executable).ok_or_else(|| {
        "Bluink's installed resources are missing beside its executable. Reinstall Bluink."
            .to_owned()
    })
}

fn root_for_executable(executable: &Path) -> Option<PathBuf> {
    executable
        .ancestors()
        .skip(1)
        .find(|directory| {
            directory.join("package.json").is_file()
                && resource_directory(directory)
                    .join("workspace-starter.json")
                    .is_file()
        })
        .map(Path::to_path_buf)
}

fn read_metadata(manifest: &Path) -> Result<Value, String> {
    std::fs::read(manifest)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or_else(|| format!("{} is missing or invalid.", manifest.display()))
}

fn package_version(directory: &Path) -> Result<String, String> {
    let manifest = directory.join("package.json");
    read_metadata(&manifest)?["version"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{} has no version.", manifest.display()))
}

/// The launcher version belongs to the product manifest. Native build identity
/// is reported separately by `build-info`.
pub(super) fn version() -> String {
    package_root()
        .ok()
        .and_then(|root| package_version(&root).ok())
        .or_else(embedded_version)
        .unwrap_or_else(|| "0.0.0-dev".to_owned())
}

/// Node's resolution for a scoped package: the nearest `node_modules` copy.
fn node_package(root: &Path, name: &str) -> Option<PathBuf> {
    root.ancestors()
        .map(|directory| directory.join("node_modules").join(name))
        .find(|candidate| candidate.join("package.json").is_file())
}

pub(super) struct Resources {
    pub(super) root: PathBuf,
    fonts: PathBuf,
}

impl Resources {
    pub(super) fn locate() -> Result<Self, String> {
        let root = package_root()?;
        let missing =
            |name: &str| format!("{name} is not installed beside Bluink. Reinstall Bluink.");
        Ok(Self {
            fonts: node_package(&root, "@bluink/fonts").ok_or_else(|| missing("@bluink/fonts"))?,
            root,
        })
    }

    pub(super) fn workspace_starter(&self) -> PathBuf {
        resource_directory(&self.root).join("workspace-starter.json")
    }

    pub(super) fn font_directory(&self, format: &str) -> Result<String, String> {
        let directory = self.fonts.join("assets").join(format);
        if !directory.is_dir() {
            return Err(
                "@bluink/fonts assets are missing. Run npm run build --workspace @bluink/fonts."
                    .to_owned(),
            );
        }
        Ok(directory.to_string_lossy().into_owned())
    }

    /// Check the installed Web package against the product pin and wire contract.
    pub(super) fn web_directory(&self) -> Result<String, String> {
        let checkout = self.root.join("Cargo.toml").is_file();
        let web = if checkout {
            self.root.join("web")
        } else {
            node_package(&self.root, "@bluink/web")
                .ok_or("@bluink/web is missing. Reinstall Bluink.")?
        };
        let directory = web.join("assets");
        let metadata = web.join("release.json");
        if !metadata.is_file() || !directory.join("index.html").is_file() {
            return Err("Bundled frontend assets are missing. Reinstall Bluink; in a development checkout run npm run build:web.".to_owned());
        }
        let metadata: Value = std::fs::read(&metadata)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or("The bundled frontend release metadata is invalid.")?;
        if metadata["format"] != "bluink.web" || metadata["formatVersion"] != 1 {
            return Err("The bundled frontend release metadata is invalid.".to_owned());
        }
        if !checkout {
            let manifest = read_metadata(&self.root.join("package.json"))?;
            let release = read_metadata(&self.root.join("release.json"))?;
            let expected = manifest["dependencies"]["@bluink/web"].as_str();
            let found = package_version(&web)?;
            if expected != Some(found.as_str()) || metadata["version"].as_str() != expected {
                return Err(
                    "Installed Web version differs from the Bluink package pin. Reinstall Bluink."
                        .into(),
                );
            }
            if release["contractHash"].as_str().is_none_or(|hash| {
                hash.len() != 64 || metadata["contractHash"].as_str() != Some(hash)
            }) {
                return Err("Installed Web wire contract differs from this Bluink release. Reinstall Bluink.".into());
            }
        }
        let fonts = package_version(&self.fonts)?;
        let required = metadata["fontsVersion"].as_str().unwrap_or_default();
        if required != fonts {
            return Err(format!(
                "The bundled frontend requires @bluink/fonts {required}; found {fonts}. Reinstall Bluink."
            ));
        }
        Ok(directory.to_string_lossy().into_owned())
    }
}

/// Per-user state (`~/.bluink`), as Node's `os.homedir()` would locate it.
pub(super) fn user_data_dir() -> Result<String, String> {
    let home = if cfg!(windows) {
        std::env::var_os("USERPROFILE")
    } else {
        std::env::var_os("HOME")
    }
    .filter(|home| !home.is_empty())
    .ok_or("Cannot locate your home folder for Bluink's user settings.")?;
    Ok(PathBuf::from(home)
        .join(".bluink")
        .to_string_lossy()
        .into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkout_and_installed_executables_find_their_own_resource_layouts() {
        let temporary = tempfile::tempdir().unwrap();
        for (name, executable, folder, checkout) in [
            (
                "checkout",
                "dist/native/development/bluink-win32-x64.exe",
                "resources",
                true,
            ),
            ("installed", "bin/bluink.exe", "vendor", false),
            (
                "installed-other-target",
                "runtime/bluink-linux-x64",
                "vendor",
                false,
            ),
        ] {
            let root = temporary.path().join(name);
            let resources = root.join(folder);
            std::fs::create_dir_all(&resources).unwrap();
            std::fs::write(root.join("package.json"), "{}").unwrap();
            std::fs::write(resources.join("workspace-starter.json"), name).unwrap();
            if checkout {
                std::fs::write(root.join("Cargo.toml"), "[package]").unwrap();
            }
            let found = root_for_executable(&root.join(executable)).unwrap();
            assert_eq!(found, root);
            assert_eq!(
                std::fs::read_to_string(resource_directory(&found).join("workspace-starter.json"))
                    .unwrap(),
                name
            );
        }
    }

    #[test]
    fn build_identity_is_the_embedded_json() {
        let identity: Value = serde_json::from_str(build_identity()).unwrap();
        assert!(identity.is_object());
    }

    #[test]
    fn packages_resolve_through_ancestor_node_modules() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("node_modules/@bluink/fonts");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("package.json"), r#"{"version":"1.2.3"}"#).unwrap();
        let nested = root.path().join("node_modules/bluink");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(
            node_package(&nested, "@bluink/fonts"),
            Some(package.clone())
        );
        assert_eq!(node_package(&nested, "@bluink/missing"), None);
        assert_eq!(package_version(&package).unwrap(), "1.2.3");
    }

    #[test]
    fn web_assets_must_match_the_release_and_its_fonts() {
        let root = tempfile::tempdir().unwrap();
        let fonts = root.path().join("fonts");
        std::fs::create_dir_all(&fonts).unwrap();
        std::fs::write(fonts.join("package.json"), r#"{"version":"7"}"#).unwrap();
        let resources = Resources {
            root: root.path().to_path_buf(),
            fonts,
        };
        assert!(
            resources
                .web_directory()
                .unwrap_err()
                .contains("@bluink/web is missing")
        );
        let web = root.path().join("node_modules/@bluink/web");
        std::fs::create_dir_all(web.join("assets")).unwrap();
        std::fs::write(web.join("package.json"), r#"{"version":"2.0.0"}"#).unwrap();
        std::fs::write(
            root.path().join("package.json"),
            r#"{"dependencies":{"@bluink/web":"2.0.0"}}"#,
        )
        .unwrap();
        let contract = "a".repeat(64);
        std::fs::write(
            root.path().join("release.json"),
            format!(r#"{{"contractHash":"{contract}"}}"#),
        )
        .unwrap();
        std::fs::write(web.join("assets/index.html"), "").unwrap();
        let metadata = |version: &str, fonts: &str| {
            std::fs::write(
                web.join("release.json"),
                format!(r#"{{"format":"bluink.web","formatVersion":1,"version":"{version}","fontsVersion":"{fonts}","contractHash":"{contract}"}}"#),
            )
            .unwrap();
        };
        metadata("1.0.0", "7");
        assert!(
            resources
                .web_directory()
                .unwrap_err()
                .contains("Web version differs")
        );
        metadata("2.0.0", "6");
        assert!(
            resources
                .web_directory()
                .unwrap_err()
                .contains("@bluink/fonts 6")
        );
        metadata("2.0.0", "7");
        assert!(resources.web_directory().unwrap().ends_with("assets"));
    }
}
