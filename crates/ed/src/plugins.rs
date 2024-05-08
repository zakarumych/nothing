use arcana::{
    edict::world::WorldLocal,
    plugin::ArcanaPlugin,
    project::{BuildProcess, Dependency, Ident, IdentBuf, Profile, Project, ProjectManifest},
    World,
};
use arcana_project::{new_plugin_crate, process_path_name, Plugin};
use camino::{Utf8Path, Utf8PathBuf};
use egui::{Color32, RichText, Ui};
use egui_file::FileDialog;

use crate::{
    container::{Container, PluginsError},
    data::ProjectData,
    get_profile,
};

/// Tool to manage plugins libraries
/// and enable/disable plugins.
pub(super) struct Plugins {
    // Linked plugins container.
    linked: Option<Container>,

    // Pending plugins container.
    // Will become linked on first occasion.
    pending: Option<Container>,

    /// Displaying plugins build failure report.
    /// Unset when build is successful or report widget is closed.
    failure: Option<miette::Report>,

    /// Running build process.
    /// Unset when build is finished.
    build: Option<BuildProcess>,

    /// Open dialog widget.
    dialog: Option<PluginsDialog>,

    profile: Profile,
}

enum PluginsDialog {
    NewPlugin(FileDialog),
    FindPlugin(FileDialog),
}

impl Plugins {
    pub fn new() -> Self {
        Plugins {
            linked: None,
            pending: None,
            failure: None,
            build: None,
            dialog: None,
            profile: get_profile(),
        }
    }

    /// Checks of all plugins from manifest are present in linked library.
    fn all_plugins_linked(&self, project: &ProjectManifest) -> bool {
        if let Some(linked) = &self.linked {
            return project.plugins.iter().all(|p| {
                let is_linked = linked.has(&p.name);
                if !is_linked {
                    tracing::debug!("Plugin '{}' is not linked", p.name);
                }
                is_linked
            });
        }
        false
    }

    /// Adds new plugin.
    pub fn add_plugin(
        &mut self,
        name: IdentBuf,
        dep: Dependency,
        project: &mut Project,
    ) -> miette::Result<()> {
        if project.has_plugin(&name) {
            miette::bail!("Plugin '{}' already exists", name);
        }

        let plugin = Plugin::from_dependency(name, dep)?;
        project.add_plugin(plugin)?;

        // Stop current build if there was one.
        tracing::info!(
            "Stopping current build process to re-build plugins library with new plugin"
        );
        self.build = None;

        // Set of active plugins doesn't change yet.
        Ok(())
    }

    pub fn get_plugin(&self, name: &Ident) -> Option<&dyn ArcanaPlugin> {
        self.linked.as_ref()?.get(name)
    }

    pub fn tick(world: &mut World) {
        let world = world.local();
        let plugins = &mut *world.expect_resource_mut::<Plugins>();
        let mut project = world.expect_resource_mut::<Project>();

        if let Some(mut build) = plugins.build.take() {
            match build.finished() {
                Ok(false) => plugins.build = Some(build),
                Ok(true) => {
                    tracing::info!(
                        "Finished building plugins library {}",
                        build.artifact().display()
                    );
                    let path = build.artifact();
                    match Container::load(path) {
                        Ok(container) => {
                            tracing::info!("New plugins container version pending. {container:#?}");
                            plugins.pending = Some(container);
                            plugins.failure = None;
                        }
                        Err(mut err) => {
                            tracing::error!("Failed to load plugins library. {err:?}");

                            if let Some(err) = err.downcast_mut::<PluginsError>() {
                                for md in err.missing_dependencies.drain(..) {
                                    tracing::error!("Missing dependency: {md:?}");

                                    if let Err(err) =
                                        plugins.add_plugin(md.plugin, md.dependency, &mut project)
                                    {
                                        tracing::error!(
                                            "Failed to add missing dependency. {err:?}"
                                        );
                                    }
                                }
                            }

                            if let Some(mut related) = err.related() {
                                for err in &mut related {
                                    tracing::error!("Related error: {err:?}");
                                }
                            }

                            plugins.failure = Some(err);
                        }
                    }
                }
                Err(err) => {
                    tracing::error!("Failed building plugins library. {err:?}");
                    plugins.failure = Some(err);
                }
            }
        }
    }

    pub fn show(world: &WorldLocal, ui: &mut Ui) {
        let plugins = &mut *world.expect_resource_mut::<Plugins>();
        let mut project = world.expect_resource_mut::<Project>();
        let mut data = world.expect_resource_mut::<ProjectData>();

        let mut sync = false;
        let mut rebuild = false;

        // Building status

        ui.add_enabled_ui(plugins.dialog.is_none(), |ui| {
            ui.allocate_ui_with_layout(
                ui.style().spacing.interact_size,
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    if plugins.build.is_some() {
                        ui.spinner();
                        ui.label("Building");
                    } else if let Some(failure) = &plugins.failure {
                        let r = ui.label(
                            egui::RichText::from("Plugins build: failed")
                                .color(ui.visuals().error_fg_color),
                        );
                        r.on_hover_ui(|ui| {
                            ui.label(failure.to_string());
                        });
                    } else {
                        ui.label("Plugins build: Ok");
                    }
                },
            );

            // Top menu
            ui.horizontal(|ui| {
                let r = match plugins.build.is_none() {
                    false => {
                        ui.add_enabled(false, egui::Button::new(egui_phosphor::regular::HAMMER))
                    }
                    true => ui.button(egui_phosphor::regular::HAMMER),
                };
                if r.clicked() {
                    let build = try_log_err!(project.build_plugins_library(plugins.profile));
                    plugins.build = Some(build);
                }
                let r = ui.button(egui_phosphor::regular::PLUS);

                if r.clicked() {
                    let mut dialog = FileDialog::select_folder(None);
                    dialog.open();
                    plugins.dialog = Some(PluginsDialog::NewPlugin(dialog));
                } else {
                    r.on_hover_ui(|ui| {
                        ui.label("New plugin");
                    });
                }

                let r = ui.button(egui_phosphor::regular::FOLDER_OPEN);
                if r.clicked() {
                    let mut dialog = FileDialog::select_folder(None);
                    dialog.open();
                    plugins.dialog = Some(PluginsDialog::FindPlugin(dialog));
                } else {
                    r.on_hover_ui(|ui| {
                        ui.label("Add plugin");
                    });
                }
            });

            ui.separator();

            // Plugins list
            let mut remove_plugin = None;

            egui::Grid::new("plugins-list")
                .striped(true)
                .show(ui, |ui| {
                    for (idx, plugin) in project.plugins().iter().enumerate() {
                        let mut heading = RichText::from(plugin.name.as_str());

                        let mut tooltip = "";
                        if !plugins.is_linked(&plugin.name) {
                            // Not linked plugin may not be active.
                            if plugins.pending.is_some() || plugins.build.is_some() {
                                tooltip = "Pending";
                                heading = heading.color(ui.visuals().warn_fg_color);
                            } else {
                                tooltip = "Build failed";
                                heading = heading.color(ui.visuals().error_fg_color);
                            }
                        } else if !data.enabled_plugins.contains(&plugin.name) {
                            heading = heading.color(ui.visuals().warn_fg_color);
                        } else if !plugins.is_active(&plugin.name) {
                            tooltip = "Dependencies are not enabled";
                            heading = heading.color(ui.visuals().warn_fg_color);
                        } else {
                            heading = heading.color(Color32::LIGHT_GREEN);
                        }

                        let was_enabled = data.enabled_plugins.contains(&plugin.name);
                        let mut enabled = was_enabled;
                        let r = ui.checkbox(&mut enabled, heading);

                        if !tooltip.is_empty() {
                            r.on_hover_text(tooltip);
                        }

                        if !was_enabled && enabled {
                            data.enabled_plugins.insert(plugin.name.clone());
                            sync = true;
                        } else if was_enabled && !enabled {
                            data.enabled_plugins.remove(&plugin.name);
                            sync = true;
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let r = ui.button(egui_phosphor::regular::TRASH);
                            if r.clicked() {
                                data.enabled_plugins.remove(&plugin.name);
                                remove_plugin = Some(idx);
                                sync = true;
                                rebuild = true;
                            }
                        });

                        ui.end_row();
                    }
                });

            if let Some(idx) = remove_plugin {
                project.manifest_mut().remove_plugin_idx(idx);
            }
        });

        match &mut plugins.dialog {
            None => {}
            Some(PluginsDialog::FindPlugin(dialog)) => match dialog.show(ui.ctx()).state() {
                egui_file::State::Open => {}
                egui_file::State::Closed | egui_file::State::Cancelled => {
                    plugins.dialog = None;
                }
                egui_file::State::Selected => match dialog.path() {
                    None => {
                        plugins.dialog = None;
                    }
                    Some(path) => {
                        match Utf8Path::from_path(path) {
                            Some(path) => {
                                match add_plugin_with_path(path.to_path_buf(), &mut project) {
                                    Ok(true) => {
                                        sync = true;
                                        rebuild = true;
                                    }
                                    Ok(false) => {
                                        tracing::warn!("Plugin already exists");
                                    }
                                    Err(err) => {
                                        tracing::error!("Failed to add plugin. {err:?}");
                                    }
                                }
                            }
                            None => {
                                tracing::error!("Invalid plugin path '{}'", path.display());
                            }
                        }
                        plugins.dialog = None;
                    }
                },
            },
            Some(PluginsDialog::NewPlugin(dialog)) => match dialog.show(ui.ctx()).state() {
                egui_file::State::Open => {}
                egui_file::State::Closed | egui_file::State::Cancelled => {
                    plugins.dialog = None;
                }
                egui_file::State::Selected => match dialog.path() {
                    None => {
                        plugins.dialog = None;
                    }
                    Some(path) => {
                        match Utf8Path::from_path(path) {
                            Some(path) => match process_path_name(path.as_std_path(), None) {
                                Ok((path, name)) => match Utf8PathBuf::from_path_buf(path) {
                                    Ok(path) => {
                                        match new_plugin_crate(
                                            &name,
                                            &path,
                                            project.engine().clone(),
                                        ) {
                                            Ok(plugin) => match project.add_plugin(plugin) {
                                                Ok(true) => {
                                                    sync = true;
                                                    rebuild = true;
                                                }
                                                Ok(false) => {
                                                    tracing::warn!("Plugin already exists");
                                                }
                                                Err(err) => {
                                                    tracing::error!(
                                                        "Failed to add plugin. {err:?}"
                                                    );
                                                }
                                            },
                                            Err(err) => {
                                                tracing::error!(
                                                    "Failed to create new plugin. {err:?}"
                                                );
                                            }
                                        }
                                    }
                                    Err(path) => {
                                        tracing::error!(
                                            "Plugin path is not UTF-8: {}",
                                            path.display()
                                        );
                                    }
                                },
                                Err(err) => {
                                    tracing::error!("Failed to process plugin path. {err:?}");
                                }
                            },
                            None => {
                                tracing::error!("Invalid plugin path '{}'", path.display());
                            }
                        }
                        plugins.dialog = None;
                    }
                },
            },
        }

        if sync {
            if let Some(c) = &mut plugins.linked {
                c.activate_plugins(&data.enabled_plugins);
            }

            if rebuild {
                try_log_err!(project.sync());
            }

            try_log_err!(data.sync(&project));
        }

        if rebuild {
            plugins.build = None;
            try_log_err!(project.init_workspace());
            plugins.build = ok_log_err!(project.build_plugins_library(plugins.profile));
        }
    }

    /// Checks if plugins with given name is active.
    pub fn is_linked(&self, name: &Ident) -> bool {
        self.linked.as_ref().map_or(false, |c| c.has(name))
    }

    /// Checks if plugins with given name is active.
    pub fn is_active(&self, name: &Ident) -> bool {
        self.linked.as_ref().map_or(false, |c| c.is_active(name))
    }
}

/// Adds new plugins library
fn add_plugin_with_path(path: Utf8PathBuf, project: &mut Project) -> miette::Result<bool> {
    let plugin = Plugin::open_local(path)?;

    project.add_plugin(plugin)
}
