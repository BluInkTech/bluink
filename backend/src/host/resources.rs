// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Resources beside the executable. A checkout holds `resources/` and an npm
//! install holds `vendor/`. Both hold `web/`, and the font and Typst packages
//! resolve through ancestor
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

fn package_version(directory: &Path) -> Result<String, String> {
    let manifest = directory.join("package.json");
    let value: Value = std::fs::read(&manifest)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or_else(|| format!("{} is missing or invalid.", manifest.display()))?;
    value["version"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{} has no version.", manifest.display()))
}

/// The release version: embedded at build time, else the package manifest.
pub(super) fn version() -> String {
    embedded_version()
        .or_else(|| {
            package_root()
                .ok()
                .and_then(|root| package_version(&root).ok())
        })
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
    pub(super) version: String,
    fonts: PathBuf,
    typst: PathBuf,
}

impl Resources {
    pub(super) fn locate() -> Result<Self, String> {
        let root = package_root()?;
        let missing =
            |name: &str| format!("{name} is not installed beside Bluink. Reinstall Bluink.");
        Ok(Self {
            fonts: node_package(&root, "@bluink/fonts").ok_or_else(|| missing("@bluink/fonts"))?,
            typst: node_package(&root, "@bluink/typst").ok_or_else(|| missing("@bluink/typst"))?,
            version: version(),
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

    pub(super) fn typst_binary(&self) -> Result<String, String> {
        let relative = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => "linux-x64/typst",
            ("windows", "x86_64") => "windows-x64/typst.exe",
            ("macos", "aarch64") => "macos-arm64/typst",
            (os, arch) => return Err(format!("Typst is not packaged for {os}/{arch}.")),
        };
        let binary = self.typst.join("assets").join(relative);
        if !binary.is_file() {
            return Err(
                "@bluink/typst assets are missing. Run npm run build --workspace @bluink/typst."
                    .to_owned(),
            );
        }
        Ok(binary.to_string_lossy().into_owned())
    }

    /// The compiled interface, checked against this release and its fonts.
    pub(super) fn web_directory(&self) -> Result<String, String> {
        let web = self.root.join("web");
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
        let found = metadata["version"].as_str().unwrap_or_default();
        if found != self.version {
            return Err(format!(
                "Bluink CLI {0} requires frontend {0}; found {found}. Reinstall Bluink.",
                self.version
            ));
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
        assert_eq!(node_package(&nested, "@bluink/typst"), None);
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
            version: "1.0.0".to_owned(),
            fonts,
            typst: root.path().join("typst"),
        };
        assert!(
            resources
                .web_directory()
                .unwrap_err()
                .contains("assets are missing")
        );
        std::fs::create_dir_all(root.path().join("web/assets")).unwrap();
        std::fs::write(root.path().join("web/assets/index.html"), "").unwrap();
        let metadata = |version: &str, fonts: &str| {
            std::fs::write(
                root.path().join("web/release.json"),
                format!(r#"{{"format":"bluink.web","formatVersion":1,"version":"{version}","fontsVersion":"{fonts}"}}"#),
            )
            .unwrap();
        };
        metadata("2.0.0", "7");
        assert!(
            resources
                .web_directory()
                .unwrap_err()
                .contains("requires frontend 1.0.0")
        );
        metadata("1.0.0", "6");
        assert!(
            resources
                .web_directory()
                .unwrap_err()
                .contains("@bluink/fonts 6")
        );
        metadata("1.0.0", "7");
        assert!(resources.web_directory().unwrap().ends_with("assets"));
    }
}
