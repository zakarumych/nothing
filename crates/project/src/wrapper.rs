//! This module generates the wrapper libs for plugins libraries.

use std::{
    env::consts::{DLL_PREFIX, DLL_SUFFIX},
    fmt,
    path::{Path, PathBuf},
    process::{Child, Command},
};

use crate::path::RealPath;

use super::Dependency;

macro_rules! make_workspace_cargo_toml {
    ($arcana:expr, $name:ident, $plugins:expr) => {
        format!(
            r#"
# This file is automatically generated for Arcana Project.
# Do not edit manually.
[package]
name = "{name}"
version = "0.0.0"
publish = false

[[bin]]
name = "ed"
path = "src/ed.rs"
required-features = ["arcana/ed"]

[[bin]]
name = "{name}"
path = "src/main.rs"

[dependencies]
arcana = {{ workspace = true }}

[workspace.dependencies]
arcana = {arcana}

[workspace]
members = {members:?}
            "#,
            name = $name,
            arcana = $arcana,
            members = $plugins,
        )
    };
}

macro_rules! make_workspace_main_rs {
    ($name:ident) => {
        format!(
            r#"
//! This file is automatically generated for Arcana Project.
//! Do not edit manually.

fn main() {{
    todo!();
}}
"#
        )
    };
}

macro_rules! make_workspace_ed_rs {
    ($name:expr) => {
        format!(
            r#"
//! This file is automatically generated for Arcana Project.
//! Do not edit manually.

// Runs Arcana Ed.
fn main() {{
    arcana::ed::run(env!("CARGO_MANIFEST_DIR").as_ref());
}}
"#
        )
    };
}

macro_rules! make_plugin_cargo_toml {
    ($name:ident = $dependency:expr) => {
        format!(
            r#"
# This file is automatically generated to wrap '{name}' crate
# into dynamically linked library.
# Do not edit manually.
[package]
name = "{name}-arcana-plugin"
version = "0.0.0"
publish = false

[lib]
crate-type = ["cdylib"]

[dependencies]
arcana = {{ workspace = true }}
{name} = {dependency}
"#,
            name = $name,
            dependency = $dependency
        )
    };
}

macro_rules! make_plugin_lib_rs {
    ($name:ident) => {
        format!(
            r#"
//! This file is automatically generated to wrap '{name}' crate
//! into dynamically linked library.
//! Do not edit manually.

/// Exports plugins for Arcana Engine from plugins library.
#[no_mangle]
pub fn arcana_plugins() -> &'static [&'static dyn arcana::plugin::ArcanaPlugin] {{
    // This method must be generated by the
    // `export_arcana_plugins!` macro in the plugins crate workspace.
    {name}::__arcana_plugins()
}}
"#,
            name = $name
        )
    };
}

struct WorkspaceDependency<'a> {
    dependency: &'a Dependency,
}

impl fmt::Display for WorkspaceDependency<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.dependency {
            Dependency::Crates(version) => write!(f, "\"{}\"", version),
            Dependency::Git { git, branch } => {
                if let Some(branch) = branch {
                    write!(f, "{{ git = \"{git}\", branch = \"{branch}\" }}")
                } else {
                    write!(f, "{{ git = \"{git}\" }}")
                }
            }
            Dependency::Path { path } => {
                // Workspace is currently hardcoded to be one directory down from the root.
                write!(f, "{{ path = \"../{}\" }}", path.as_str().escape_default())
            }
        }
    }
}

pub fn init_workspace<I, S>(
    name: &str,
    arcana: Option<&Dependency>,
    plugins: I,
    root: &RealPath,
) -> miette::Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).map_err(|err| {
        miette::miette!(
            "Failed to create project workspace directory '{}'. {err}",
            workspace.display()
        )
    })?;

    std::fs::write(workspace.join(".gitignore"), b"*").map_err(|err| {
        miette::miette!(
            "Failed to create workspace .gitignore file '{}'. {err}",
            workspace.display()
        )
    })?;

    let src_dir = workspace.join("src");
    std::fs::create_dir_all(&src_dir).map_err(|err| {
        miette::miette!(
            "Failed to create workspace src directory '{}'. {err}",
            src_dir.display()
        )
    })?;

    let plugins = plugins
        .into_iter()
        .map(|plugin| format!("plugins/{}", plugin.as_ref().escape_default()))
        .collect::<Vec<_>>();

    let cargo_toml = match arcana {
        None => {
            make_workspace_cargo_toml!(format!("\"{}\"", env!("CARGO_PKG_VERSION")), name, plugins)
        }
        Some(arcana) => {
            make_workspace_cargo_toml!(WorkspaceDependency { dependency: arcana }, name, plugins)
        }
    };

    std::fs::write(workspace.join("Cargo.toml"), cargo_toml).map_err(|err| {
        miette::miette!(
            "Failed to write workspace Cargo.toml file to '{}'. {err}",
            workspace.display()
        )
    })?;

    std::fs::write(
        workspace.join("rust-toolchain.toml"),
        "[toolchain]\nchannel = \"nightly\"",
    )
    .map_err(|err| {
        miette::miette!(
            "Failed to write workspace rust-toolchain.toml file to '{}'. {err}",
            workspace.display()
        )
    })?;

    let main_rs = make_workspace_main_rs!(name);
    std::fs::write(src_dir.join("main.rs"), main_rs).map_err(|err| {
        miette::miette!(
            "Failed to write workspace main.rs file to '{}'. {err}",
            src_dir.display()
        )
    })?;

    let ed_rs = make_workspace_ed_rs!(name);
    std::fs::write(src_dir.join("ed.rs"), ed_rs).map_err(|err| {
        miette::miette!(
            "Failed to write workspace ed.rs file to '{}'. {err}",
            src_dir.display()
        )
    })?;

    Ok(())
}

struct PluginDependency<'a> {
    dependency: &'a Dependency,
}

impl fmt::Display for PluginDependency<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.dependency {
            Dependency::Crates(version) => write!(f, "\"{}\"", version),
            Dependency::Git { git, branch } => {
                if let Some(branch) = branch {
                    write!(f, "{{ git = \"{git}\", branch = \"{branch}\" }}")
                } else {
                    write!(f, "{{ git = \"{git}\" }}")
                }
            }
            Dependency::Path { path } => {
                // Plugins are currently hardcoded to be in the 'workspace/plugins/<plugin-name>' directory.
                write!(
                    f,
                    "{{ path = \"../../../{}\" }}",
                    path.as_str().escape_default()
                )
            }
        }
    }
}

pub fn init_plugin(name: &str, dependency: &Dependency, root: &RealPath) -> miette::Result<()> {
    let workspace = root.join("workspace");

    let cargo_toml = make_plugin_cargo_toml!(name = PluginDependency { dependency });

    let lib_rs = make_plugin_lib_rs!(name);

    let plugin_dir = workspace.join("plugins").join(name);
    std::fs::create_dir_all(&plugin_dir).map_err(|err| {
        miette::miette!(
            "Failed to create plugin directory '{}'. {err}",
            plugin_dir.display()
        )
    })?;

    let src_dir = plugin_dir.join("src");
    std::fs::create_dir_all(&src_dir).map_err(|err| {
        miette::miette!(
            "Failed to create plugin src directory '{}'. {err}",
            src_dir.display()
        )
    })?;

    let cargo_toml_path = plugin_dir.join("Cargo.toml");
    let lib_rs_path = src_dir.join("lib.rs");

    std::fs::write(&cargo_toml_path, cargo_toml).map_err(|err| {
        miette::miette!(
            "Failed to write plugin Cargo.toml file '{}'. {err}",
            cargo_toml_path.display()
        )
    })?;

    std::fs::write(&lib_rs_path, lib_rs).map_err(|err| {
        miette::miette!(
            "Failed to write plugin lib.rs file '{}'. {err}",
            lib_rs_path.display()
        )
    })?;

    Ok(())
}

pub fn run_editor(name: &str, root: &RealPath) -> Command {
    let workspace = root.join("workspace");
    let mut cmd = Command::new("cargo");
    cmd.arg("run")
        .arg("--features=arcana/ed")
        .arg(format!("--package={}", name))
        .arg("--bin=ed")
        .env("RUSTFLAGS", "-Zshare-generics=off -Cprefer-dynamic=yes")
        .current_dir(&workspace);
    cmd
}

pub fn build_plugin(name: &str, root: &RealPath) -> miette::Result<PluginBuild> {
    let workspace = root.join("workspace");

    let child = Command::new("cargo")
        .arg("build")
        .arg("--features=arcana/ed")
        .arg(format!("--package={}-arcana-plugin", name))
        .env("RUSTFLAGS", "-Zshare-generics=off -Cprefer-dynamic=yes")
        .current_dir(&workspace)
        .spawn()
        .map_err(|err| {
            miette::miette!(
                "Failed to start building plugin '{name}' in '{}'. {err}",
                workspace.display()
            )
        })?;

    let artifact = plugin_lib_path(name, root);

    Ok(PluginBuild { child, artifact })
}

fn plugin_lib_path(name: &str, root: &RealPath) -> PathBuf {
    let mut lib_path = root.as_path().to_owned();
    lib_path.push("workspace");
    lib_path.push("target");
    lib_path.push("debug");
    lib_path.push(format!(
        "{DLL_PREFIX}{name}_arcana_plugin{DLL_SUFFIX}",
        name = name.replace('-', "_")
    ));
    lib_path
}

pub struct PluginBuild {
    child: Child,
    artifact: PathBuf,
}

impl PluginBuild {
    pub fn finished(&mut self) -> miette::Result<bool> {
        match self.child.try_wait() {
            Err(err) => {
                miette::bail!("Failed to wait for build process to finish. {err}",);
            }
            Ok(None) => Ok(false),
            Ok(Some(status)) if status.success() => Ok(true),
            Ok(Some(status)) => {
                miette::bail!(
                    "Build process failed with status '{status}'.",
                    status = status
                );
            }
        }
    }

    pub fn artifact(&self) -> &Path {
        &self.artifact
    }
}
