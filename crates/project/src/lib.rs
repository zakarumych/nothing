//! Provides types to crete, save, open and manipulate Arcana projects.
//!
//!
#![allow(warnings)]

use std::{
    fmt,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    ops::Deref,
    path::{Path, PathBuf, MAIN_SEPARATOR},
    process::Child,
};

use arcana_names::{Ident, Name};
use camino::{Utf8Path, Utf8PathBuf};

mod dependency;
mod generator;
mod manifest;
mod path;
mod plugin;
mod wrapper;

use generator::init_workspace;
use manifest::serialize_manifest;
use miette::{Context, IntoDiagnostic};
use path::{normalized_path, normalizing_join};

pub use self::{
    dependency::Dependency,
    generator::new_plugin_crate,
    manifest::ProjectManifest,
    path::{make_relative, real_path},
    plugin::Plugin,
    wrapper::{game_bin_path, BuildProcess, Profile},
};

const MANIFEST_NAME: &'static str = "Arcana.toml";
const CARGO_TOML_NAME: &'static str = "Cargo.toml";
const WORKSPACE_DIR_NAME: &'static str = "crates";

/// An open project object.
///
/// It contains project manifest,
/// manifest file path
/// and project root path.
///
/// Manifest file is a TOML file and is written when project is synced.
/// When new project is created file with initial manifest is created.
///
/// If file is edited or deleted project will silently overwrite it on sync.
///
/// TODO: Figure out why not to lock the file?
pub struct Project {
    /// Actual project manifest.
    manifest: ProjectManifest,

    // Contains path assigned to the project.
    // It will sync with the manifest file at the path both ways.
    // Whenever changes happen to the manifest file, the user will be asked what to do:
    // reload or overwrite.
    // If file is deleted the user will be notified on save.
    // On save the file will be created if it doesn't exist.
    manifest_path: PathBuf,

    /// Project root path.
    /// Typically it is parent directory of the manifest file.
    root_path: PathBuf,
}

impl fmt::Debug for Project {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Project")
            .field("manifest", &self.manifest_path)
            .finish()
    }
}

impl Project {
    /// Creates new project with the given name at the given path.
    /// Path will become project root directory.
    /// Manifest file will be `Arcana.toml` in the root directory.
    ///
    /// # Errors
    ///
    /// * If `engine` dependency is invalid.
    ///   Path dependency is invalid if it is not a valid path to directory containing `Cargo.toml`.
    /// * If `new` is true and `path` is already exists.
    /// * If `path` already contains Arcana project.
    pub fn new(
        name: Ident,
        path: &Path,
        mut engine: Dependency,
        new: bool,
    ) -> miette::Result<Self> {
        if let Ok(m) = path.metadata() {
            if new {
                miette::bail!(
                    "Cannot create new project. Path '{}' already exists",
                    path.display()
                );
            }

            if !m.is_dir() {
                miette::bail!(
                    "Cannot create new project. Path '{}' is not a directory",
                    path.display()
                );
            }

            if path.join(MANIFEST_NAME).exists() {
                miette::bail!(
                    "Cannot create new project. Path '{}' is already an Arcana project",
                    path.display()
                );
            }
        }

        let path = match real_path(&path) {
            Some(path) => path,
            None => {
                miette::bail!(
                    "Cannot create new project. Failed to resolve path '{}'",
                    path.display()
                );
            }
        };

        let engine = match engine {
            Dependency::Path { path: engine_path } if !engine_path.is_absolute() => {
                let real_engine_path = match real_path(engine_path.as_std_path()) {
                    Some(path) => path,
                    None => {
                        miette::bail!(
                            "Cannot create new project. Failed to resolve engine path '{engine_path}'"
                        );
                    }
                };

                let relative_engine_path = make_relative(&real_engine_path, &path);

                let relative_engine_path = match Utf8PathBuf::from_path_buf(relative_engine_path) {
                    Ok(path) => path,
                    Err(err) => {
                        miette::bail!(
                            "Cannot create new project. Resolved engine path contains non-utf8 symbols '{engine_path}'",
                        );
                    }
                };

                let cargo_toml_path = real_engine_path.join(CARGO_TOML_NAME);

                let manifest = match cargo_toml::Manifest::from_path(cargo_toml_path) {
                    Ok(manifest) => manifest,
                    Err(err) => {
                        miette::bail!(
                            "Failed to read engine manifest '{engine_path}/{CARGO_TOML_NAME}': {err:?}",
                        );
                    }
                };

                let package = match &manifest.package {
                    Some(package) => package,
                    None => {
                        miette::bail!(
                            "Engine manifest '{engine_path}/{CARGO_TOML_NAME}' does not contain package section",
                        );
                    }
                };

                if package.name != "arcana" {
                    miette::bail!("'{engine_path}' is not an Arcana engine");
                }

                // Rewrite engine dependency to relative path.
                Dependency::Path {
                    path: relative_engine_path,
                }
            }
            engine => engine,
        };
        /// Construct project manifest.
        let manifest = ProjectManifest {
            name,
            engine,
            plugins: Vec::new(),
        };

        let manifest_str = match toml::to_string(&manifest) {
            Ok(s) => s,
            Err(err) => {
                miette::bail!("Failed to serialize project manifest. {err:?}");
            }
        };

        if let Err(err) = std::fs::create_dir_all(&path) {
            miette::bail!(
                "Cannot create new project. Failed to create directory '{}': {err:?}",
                path.display()
            );
        }

        let manifest_path = path.join(MANIFEST_NAME);
        if let Err(err) = std::fs::write(&*manifest_path, &*manifest_str) {
            miette::bail!(
                "Cannot create new project. Failed to write manifest to '{}': {err:?}",
                manifest_path.display()
            );
        }

        tracing::info!("Created project {name} at '{}'", path.display());

        Ok(Project {
            root_path: path,
            manifest_path,
            manifest,
        })
    }

    /// Opens existing Arcana project from the given path.
    /// The path must a manifest file.
    ///
    /// # Errors
    ///
    /// * If `path` is not a valid path to Arcana project.
    pub fn open(path: &Path) -> miette::Result<Self> {
        let path = match real_path(&path.join(MANIFEST_NAME)) {
            Some(path) => path,
            None => {
                miette::bail!(
                    "Cannot open project at '{}': failed to resolve path",
                    path.display()
                );
            }
        };

        let m = match path.metadata() {
            Ok(m) => m,
            Err(err) => {
                miette::bail!("Cannot open project at '{}': {err:?}", path.display());
            }
        };

        if m.is_symlink() {
            miette::bail!(
                "Cannot open project at '{}': failed to follow symlink",
                path.display()
            );
        }

        if m.is_dir() {
            miette::bail!(
                "Cannot open project with manifest at '{}': path is a directory",
                path.display()
            );
        }

        let (manifest_path, root_path) = {
            let root_path = match path.parent() {
                Some(path) => path.to_owned(),
                None => {
                    miette::bail!(
                        "Cannot open project at '{}': failed to resolve parent directory",
                        path.display()
                    );
                }
            };
            (path.to_owned(), root_path)
        };

        let mut arcana_toml = match std::fs::read_to_string(&manifest_path) {
            Ok(s) => s,
            Err(err) => {
                miette::bail!(
                    "Cannot open project at '{}': failed to read project manifest: {err:?}",
                    path.display()
                );
            }
        };

        let manifest: ProjectManifest = match toml::from_str(&arcana_toml) {
            Ok(manifest) => manifest,
            Err(err) => {
                miette::bail!("Cannot deserialize project manifest from \"Arcana.toml\": {err:?}");
            }
        };

        let project = Project {
            root_path,
            manifest_path,
            manifest,
        };

        Ok(project)
    }

    /// Searches for Arcana project in the given path or any parent directory.
    ///
    /// # Errors
    ///
    /// * If `path` is not a valid path.
    /// * If project is not found in `path` or any parent directory.
    /// * If project is found but cannot be opened.
    pub fn find(path: &Path) -> miette::Result<Self> {
        let mut candidate = match real_path(path) {
            Some(path) => path,
            None => {
                miette::bail!(
                    "Cannot find project at '{}': failed to resolve path",
                    path.display()
                );
            }
        };

        loop {
            candidate.push(MANIFEST_NAME);
            if candidate.exists() {
                return Project::open(&candidate);
            }
            if !candidate.pop() {
                break;
            }
            if !candidate.pop() {
                break;
            }
        }

        miette::bail!(
            "Cannot find project in '{}' or any parent directory",
            path.display()
        );
    }

    /// Returns path to the project.
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    pub fn sync(&mut self) -> miette::Result<()> {
        // let serialized_manifest = toml::to_string(&self.manifest).map_err(|err| {
        //     miette::miette!("Cannot serialize project manifest to \"Arcana.toml\": {err:?}")
        // })?;

        let serialized_manifest = serialize_manifest(&self.manifest).map_err(|err| {
            miette::miette!("Cannot serialize project manifest to \"Arcana.toml\": {err:?}")
        })?;

        match std::fs::write(&self.manifest_path, serialized_manifest) {
            Ok(()) => Ok(()),
            Err(err) => {
                miette::bail!(
                    "Cannot write project manifest to \"Arcana.toml\": {err:?}",
                    err = err
                );
            }
        }
    }

    /// Initializes all plugin wrapper libs and workspace.
    pub fn init_workspace(&self) -> miette::Result<()> {
        init_workspace(
            &self.root_path,
            &self.manifest.name,
            &self.manifest.engine,
            &self.manifest.plugins,
        )
    }

    pub fn build_plugins_library(&self, profile: Profile) -> miette::Result<BuildProcess> {
        self.init_workspace()?;
        wrapper::build_plugins(&self.root_path, profile)
    }

    pub fn manifest(&self) -> &ProjectManifest {
        &self.manifest
    }

    pub fn manifest_mut(&mut self) -> &mut ProjectManifest {
        &mut self.manifest
    }

    /// Returns name of the project.
    pub fn name(&self) -> Ident {
        self.manifest.name
    }

    pub fn engine(&self) -> &Dependency {
        &self.manifest.engine
    }

    pub fn engine_mut(&mut self) -> &mut Dependency {
        &mut self.manifest.engine
    }

    pub fn plugins(&self) -> &[Plugin] {
        &self.manifest.plugins
    }

    pub fn plugins_mut(&mut self) -> &mut [Plugin] {
        &mut self.manifest.plugins
    }

    pub fn run_editor(self, profile: Profile) -> miette::Result<()> {
        self.init_workspace()?;
        let status = wrapper::run_editor(&self.root_path, profile)
            .status()
            .map_err(|err| {
                miette::miette!(
                    "Cannot run \"ed\" on \"{}\": {err:?}",
                    self.root_path.display()
                )
            })?;

        match status.code() {
            Some(0) => Ok(()),
            Some(code) => miette::bail!("\"ed\" exited with code {}", code),
            None => miette::bail!("\"ed\" terminated by signal"),
        }
    }

    pub fn build_editor_non_blocking(&self, profile: Profile) -> miette::Result<Child> {
        self.init_workspace()?;
        match wrapper::build_editor(&self.root_path, profile).spawn() {
            Ok(child) => Ok(child),
            Err(err) => {
                miette::bail!(
                    "Cannot build \"ed\" on \"{}\": {err:?}",
                    self.root_path.display()
                )
            }
        }
    }

    pub fn run_editor_non_blocking(&self, profile: Profile) -> miette::Result<Child> {
        self.init_workspace()?;
        match wrapper::run_editor(&self.root_path, profile).spawn() {
            Ok(child) => Ok(child),
            Err(err) => {
                miette::bail!(
                    "Cannot run \"ed\" on \"{}\": {err:?}",
                    self.root_path.display()
                )
            }
        }
    }

    pub fn build_game(self, profile: Profile) -> miette::Result<PathBuf> {
        self.init_workspace()?;
        let status = wrapper::build_game(&self.root_path, profile)
            .status()
            .map_err(|err| {
                miette::miette!(
                    "Cannot build game \"{}\": {err:?}",
                    self.root_path.display()
                )
            })?;

        match status.code() {
            Some(0) => {}
            Some(code) => miette::bail!("Game build exited with code {}", code),
            None => miette::bail!("Game build terminated by signal"),
        }

        Ok(game_bin_path(&self.manifest.name, &self.root_path))
    }

    pub fn run_game(self, profile: Profile) -> miette::Result<()> {
        self.init_workspace()?;
        let status = wrapper::run_game(&self.root_path, profile)
            .status()
            .map_err(|err| {
                miette::miette!(
                    "Cannot run game on \"{}\": {err:?}",
                    self.root_path.display()
                )
            })?;

        match status.code() {
            Some(0) => Ok(()),
            Some(code) => miette::bail!("Game exited with code {}", code),
            None => miette::bail!("Game terminated by signal"),
        }
    }

    pub fn has_plugin(&self, name: Ident) -> bool {
        self.manifest.has_plugin(name)
    }

    pub fn add_plugin(&mut self, mut plugin: Plugin) -> miette::Result<bool> {
        if self.manifest.has_plugin(plugin.name) {
            return Ok(false);
        }

        plugin.dependency = plugin.dependency.make_relative(&self.root_path)?;

        tracing::info!("Plugin '{}' added", plugin.name);

        self.manifest.plugins.push(plugin);
        Ok(true)
    }
}

fn is_in_cargo_workspace(path: &Path) -> bool {
    for a in path.ancestors() {
        if a.exists() {
            let mut candidate = a.to_owned();

            loop {
                candidate.push("Cargo.toml");
                if candidate.exists() {
                    return true;
                }
                assert!(candidate.pop());
                if !candidate.pop() {
                    break;
                }
            }
            return false;
        }
    }
    false
}

pub fn process_path_ident(path: &Path, name: Option<Ident>) -> miette::Result<(PathBuf, Ident)> {
    let path = match real_path(&path) {
        Some(path) => path,
        None => miette::bail!(
            "Failed to get project destination path from {}",
            path.display()
        ),
    };

    let name = match name {
        None => {
            let Some(file_name) = path.file_name() else {
                miette::bail!("Failed to get project name destination path");
            };

            if file_name.is_empty() || file_name == "." || file_name == ".." {
                miette::bail!("Failed to get project name destination path");
            }

            let Some(file_name) = file_name.to_str() else {
                miette::bail!("Failed to get project name destination path");
            };

            let Ok(file_name) = Ident::from_str(file_name) else {
                miette::bail!(
                    "Project's directory name cannot be used as project name is it is not valid identifier. Specify name manually"
                );
            };

            file_name.to_owned()
        }
        Some(name) => name,
    };

    Ok((path, name))
}
