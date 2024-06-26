use std::{fmt, path::Path};

use camino::Utf8Path;

use crate::{dependency::Dependency, path::make_relative, plugin::Plugin, WORKSPACE_DIR_NAME};

struct ArcanaDependency<'a>(&'a Dependency);

impl fmt::Display for ArcanaDependency<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Dependency::Crates(version) => write!(f, "\"{}\"", version),
            Dependency::Git { git, branch } => {
                if let Some(branch) = branch {
                    write!(f, "{{ git = \"{git}\", branch = \"{branch}\" }}")
                } else {
                    write!(f, "{{ git = \"{git}\" }}")
                }
            }
            Dependency::Path { path } => {
                write!(f, "{{ path = \"{}\" }}", path.as_str().escape_default())
            }
        }
    }
}

struct ArcanaEdDependency<'a>(&'a Dependency);

impl fmt::Display for ArcanaEdDependency<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Dependency::Crates(version) => write!(f, "\"{}\"", version),
            Dependency::Git { git, branch } => {
                if let Some(branch) = branch {
                    write!(f, "{{ git = \"{git}\", branch = \"{branch}\" }}")
                } else {
                    write!(f, "{{ git = \"{git}\" }}")
                }
            }
            Dependency::Path { path } => {
                write!(
                    f,
                    "{{ path = \"{}/../ed\" }}",
                    path.as_str().escape_default()
                )
            }
        }
    }
}

struct PluginDependency<'a> {
    dep: &'a Dependency,
}

impl fmt::Display for PluginDependency<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.dep {
            Dependency::Crates(version) => write!(f, "\"{}\"", version),
            Dependency::Git { git, branch } => {
                if let Some(branch) = branch {
                    write!(f, "{{ git = \"{git}\", branch = \"{branch}\" }}",)
                } else {
                    write!(f, "{{ git = \"{git}\" }}")
                }
            }
            Dependency::Path { path } => {
                write!(f, "{{ path = \"{}\" }}", path.as_str().escape_default(),)
            }
        }
    }
}

/// Generates new plugin crate
pub fn new_plugin_crate(
    name: &str,
    path: &Utf8Path,
    engine: Dependency,
    root: Option<&Path>,
) -> miette::Result<Plugin> {
    if path.exists() {
        miette::bail!(
            "Cannot create plugins crate. Path '{}' already exists",
            path
        );
    }

    std::fs::create_dir_all(&path).map_err(|err| {
        miette::miette!(
            "Failed to create project plugin crate directory: '{}'. {err:?}",
            path
        )
    })?;

    let engine = match root {
        None => engine.make_relative(path),
        Some(root) => engine.make_relative_from(root, path),
    }?;

    let mut cargo_toml = format!(
        r#"[package]
version = "0.0.0"
name = "{name}"
edition = "2021"
publish = false

[dependencies]
arcana = {engine}
"#,
        engine = ArcanaDependency(&engine)
    );

    let cargo_toml_path = path.join("Cargo.toml");
    write_file(&cargo_toml_path, &cargo_toml).map_err(|err| {
        miette::miette!(
            "Failed to create project game crate Cargo.toml '{}'. {err:?}",
            cargo_toml_path
        )
    })?;

    let src_path = path.join("src");
    std::fs::create_dir_all(&src_path).map_err(|err| {
        miette::miette!(
            "Failed to create project game crate src directory: '{}'. {err:?}",
            src_path
        )
    })?;

    let lib_rs = format!(
        r#"
use arcana::{{
    edict::{{Scheduler, World}},
    plugin::ArcanaPlugin,
    export_arcana_plugin,
}};

export_arcana_plugin!(ThePlugin);

pub struct ThePlugin;

impl ArcanaPlugin for ThePlugin {{
    fn name(&self) -> &'static str {{
        "{name}"
    }}

    fn init(&self, _world: &mut World, _scheduler: &mut Scheduler) {{
        unimplemented!()
    }}
}}
"#
    );

    let lib_rs_path = src_path.join("lib.rs");
    write_file(&lib_rs_path, &lib_rs).map_err(|err| {
        miette::miette!(
            "Failed to create project plugins crate source: '{}'. {err:?}",
            lib_rs_path
        )
    })?;

    Plugin::open_local(path.to_owned())
}

/// Generates workspace.
pub fn init_workspace(
    root: &Path,
    name: &str,
    engine: &Dependency,
    plugins: &[Plugin],
) -> miette::Result<()> {
    let workspace = root.join(WORKSPACE_DIR_NAME);
    std::fs::create_dir_all(&*workspace).map_err(|err| {
        miette::miette!(
            "Failed to create project workspace directory: '{}'. {err:?}",
            workspace.display()
        )
    })?;

    let gitignore = "crates\nArcana.bin.bak\n";
    let gitignore_path = workspace.join(".gitignore");
    std::fs::write(&gitignore_path, gitignore).map_err(|err| {
        miette::miette!(
            "Failed to create project workspace .gitignore: '{}'. {err:?}",
            workspace.display()
        )
    })?;

    let engine = engine.clone().make_relative_from(".", WORKSPACE_DIR_NAME)?;

    let cargo_toml = format!(
        r#"# This file is automatically generated for Arcana Project.
# It should not require manual editing.
# If manual editing is required, consider posting your motivation in new GitHub issue
# [{gh_issue}]

[workspace]
resolver = "2"
members = ["plugins", "ed", "game"]

[workspace.dependencies]
arcana = {arcana}
arcana-ed = {arcana_ed}
"#,
        gh_issue = github_autogen_issue_template("workspace Cargo.toml"),
        arcana = ArcanaDependency(&engine),
        arcana_ed = ArcanaEdDependency(&engine),
    );

    let cargo_toml_path = workspace.join("Cargo.toml");
    write_file(&cargo_toml_path, &cargo_toml).map_err(|err| {
        miette::miette!(
            "Failed to create project workspace Cargo.toml: '{}'. {err:?}",
            cargo_toml_path.display()
        )
    })?;

    let rust_toolchain = r#"[toolchain]
channel = "nightly"
    "#;

    let rust_toolchain_path = workspace.join("rust-toolchain.toml");
    write_file(&rust_toolchain_path, &rust_toolchain).map_err(|err| {
        miette::miette!(
            "Failed to create project workspace rust-toolchain.toml: '{}'. {err:?}",
            rust_toolchain_path.display()
        )
    })?;

    init_ed_crate(root, &workspace)?;
    init_plugins_crate(root, &workspace, plugins)?;
    init_game_crate(root, &workspace, name, plugins)?;

    Ok(())
}

/// Generates ed crate
fn init_ed_crate(root: &Path, workspace: &Path) -> miette::Result<()> {
    let ed_path = workspace.join("ed");

    std::fs::create_dir_all(&ed_path).map_err(|err| {
        miette::miette!(
            "Failed to create project ed crate directory: '{}'. {err:?}",
            ed_path.display()
        )
    })?;

    let cargo_toml = format!(
        r#"# This file is automatically generated for Arcana Project.
# It should not require manual editing.
# If manual editing is required, consider posting your motivation in new GitHub issue
# [{gh_issue}]
[package]
name = "ed"
version = "0.0.0"
publish = false
edition = "2021"

[dependencies]
arcana = {{ workspace = true }}
arcana-ed = {{ workspace = true }}
"#,
        gh_issue = github_autogen_issue_template("ed/Cargo.toml")
    );

    let cargo_toml_path = ed_path.join("Cargo.toml");
    write_file(&cargo_toml_path, &cargo_toml).map_err(|err| {
        miette::miette!(
            "Failed to create project ed crate Cargo.toml '{}'. {err:?}",
            cargo_toml_path.display()
        )
    })?;

    let src_path = ed_path.join("src");
    std::fs::create_dir_all(&src_path).map_err(|err| {
        miette::miette!(
            "Failed to create project ed crate src directory: '{}'. {err:?}",
            src_path.display()
        )
    })?;

    let main_rs = format!(
        r#"//! This file is automatically generated for Arcana Project.
//! It should not require manual editing.
//! If manual editing is required, consider posting your motivation in new GitHub issue
//! [{gh_issue}]

fn main() {{
    arcana_ed::run(env!("CARGO_MANIFEST_DIR").as_ref());
}}
"#,
        gh_issue = github_autogen_issue_template("ed/src/main.rs")
    );

    let main_rs_path = src_path.join("main.rs");
    write_file(&main_rs_path, &main_rs).map_err(|err| {
        miette::miette!(
            "Failed to create project ed crate source: '{}'. {err:?}",
            main_rs_path.display()
        )
    })?;

    Ok(())
}

/// Generates plugins crate
fn init_plugins_crate(root: &Path, workspace: &Path, plugins: &[Plugin]) -> miette::Result<()> {
    let plugins_path = workspace.join("plugins");

    std::fs::create_dir_all(&plugins_path).map_err(|err| {
        miette::miette!(
            "Failed to create project plugins crate directory: '{}'. {err:?}",
            plugins_path.display()
        )
    })?;

    let mut cargo_toml = format!(
        r#"# This file is automatically generated for Arcana Project.
# It should not require manual editing.
# If manual editing is required, consider posting your motivation in new GitHub issue
# [{gh_issue}]
[package]
name = "plugins"
version = "0.0.0"
publish = false
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
arcana = {{ workspace = true, features = ["dynamic"] }}
arcana-ed = {{ workspace = true }}
"#,
        gh_issue = github_autogen_issue_template("plugins/Cargo.toml")
    );

    for plugin in plugins {
        let dep = plugin
            .dependency
            .clone()
            .make_relative_from(root, &plugins_path)?;

        cargo_toml.push_str(&format!(
            "{name} = {dependency}\n",
            name = &plugin.name,
            dependency = PluginDependency { dep: &dep }
        ));
    }

    let cargo_toml_path = plugins_path.join("Cargo.toml");
    write_file(&cargo_toml_path, &cargo_toml).map_err(|err| {
        miette::miette!(
            "Failed to create project plugins crate Cargo.toml '{}'. {err:?}",
            cargo_toml_path.display()
        )
    })?;

    let src_path = plugins_path.join("src");
    std::fs::create_dir_all(&src_path).map_err(|err| {
        miette::miette!(
            "Failed to create project plugins crate src directory: '{}'. {err:?}",
            src_path.display()
        )
    })?;

    let mut lib_rs = format!(
        r#"//! This file is automatically generated for Arcana Project.
//! It should not require manual editing.
//! If manual editing is required, consider posting your motivation in new GitHub issue
//! [{gh_issue}]

#[no_mangle]
pub fn arcana_version() -> &'static str {{
    arcana::version()
}}

#[no_mangle]
pub fn arcana_linked(check: &::core::sync::atomic::AtomicBool) -> bool {{
    arcana::plugin::running_arcana_instance_check(check)
}}

#[no_mangle]
pub fn arcana_plugins() -> &'static [(arcana::Ident, &'static dyn arcana::plugin::ArcanaPlugin)] {{
    const PLUGINS: [(arcana::Ident, &'static dyn arcana::plugin::ArcanaPlugin); {plugins_count}] = ["#,
        gh_issue = github_autogen_issue_template("plugins/src/lib.rs"),
        plugins_count = plugins.len(),
    );

    if !plugins.is_empty() {
        lib_rs.push('\n');

        for plugin in plugins {
            lib_rs.push_str(&format!(
                "        (arcana::Ident::from_ident_str(stringify!({name})), {name}::__arcana_plugin()),\n",
                name = &plugin.name
            ));
        }
    }

    lib_rs.push_str(
        r#"    ];
    &PLUGINS
}"#,
    );

    let lib_rs_path = src_path.join("lib.rs");
    write_file(&lib_rs_path, &lib_rs).map_err(|err| {
        miette::miette!(
            "Failed to create project plugins crate source: '{}'. {err:?}",
            lib_rs_path.display()
        )
    })?;

    Ok(())
}

/// Generates game crate
fn init_game_crate(
    root: &Path,
    workspace: &Path,
    name: &str,
    plugins: &[Plugin],
) -> miette::Result<()> {
    let game_path = workspace.join("game");

    std::fs::create_dir_all(&game_path).map_err(|err| {
        miette::miette!(
            "Failed to create project game crate directory: '{}'. {err:?}",
            game_path.display()
        )
    })?;

    let mut cargo_toml = format!(
        r#"# This file is automatically generated for Arcana Project.
# It should not require manual editing.
# If manual editing is required, consider posting your motivation in new GitHub issue
# [{gh_issue}]
[package]
name = "game"
version = "0.0.0"
publish = false
edition = "2021"

[[bin]]
name = "{name}"
path = "src/main.rs"

[dependencies]
arcana = {{ workspace = true }}
"#,
        gh_issue = github_autogen_issue_template("game/Cargo.toml")
    );

    for plugin in plugins {
        let dep = plugin
            .dependency
            .clone()
            .make_relative_from(root, &game_path)?;

        cargo_toml.push_str(&format!(
            "{name} = {dependency}\n",
            name = &plugin.name,
            dependency = PluginDependency { dep: &dep }
        ));
    }

    let cargo_toml_path = game_path.join("Cargo.toml");
    write_file(&cargo_toml_path, &cargo_toml).map_err(|err| {
        miette::miette!(
            "Failed to create project game crate Cargo.toml '{}'. {err:?}",
            cargo_toml_path.display()
        )
    })?;

    let src_path = game_path.join("src");
    std::fs::create_dir_all(&src_path).map_err(|err| {
        miette::miette!(
            "Failed to create project game crate src directory: '{}'. {err:?}",
            src_path.display()
        )
    })?;

    let mut main_rs = format!(
        r#"//! This file is automatically generated for Arcana Project.
//! It should not require manual editing.
//! If manual editing is required, consider posting your motivation in new GitHub issue
//! [{gh_issue}]

fn main() {{
    const PLUGINS: [&'static dyn arcana::plugin::ArcanaPlugin; {plugins_count}] = ["#,
        gh_issue = github_autogen_issue_template("game/src/main.rs"),
        plugins_count = plugins.len(),
    );

    if !plugins.is_empty() {
        main_rs.push('\n');

        for plugin in plugins {
            main_rs.push_str(&format!(
                "        {name}::__arcana_plugin(),\n",
                name = &plugin.name
            ));
        }
    }

    main_rs.push_str(
        r#"    ];
    arcana::game::run(&PLUGINS);
}"#,
    );

    let main_rs_path = src_path.join("main.rs");
    write_file(&main_rs_path, &main_rs).map_err(|err| {
        miette::miette!(
            "Failed to create project game crate source: '{}'. {err:?}",
            main_rs_path.display()
        )
    })?;

    Ok(())
}

/// Writes content to a file.
/// If new content is the same as old content the file is not modified.
fn write_file<P, C>(path: P, content: C) -> std::io::Result<()>
where
    P: AsRef<Path>,
    C: AsRef<[u8]>,
{
    match std::fs::read(path.as_ref()) {
        Ok(old_content) if old_content == content.as_ref() => {
            return Ok(());
        }
        _ => {}
    }
    std::fs::write(path, content)
}

fn github_autogen_issue_template(file: &str) -> String {
    format!("https://github.com/zakarumych/nothing/issues/new?body=%3C%21--%20Please%2C%20provide%20your%20reason%20to%20edit%20auto-generated%20{file}%20in%20Arcana%20project%20--%3E")
}
