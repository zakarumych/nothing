use std::{path::PathBuf, process::Child, time::Duration};

use arcana::{
    blink_alloc::BlinkAlloc,
    edict::world::World,
    gametime::FrequencyNumExt,
    input::ViewportInput,
    mev,
    project::{Dependency, Profile, Project},
    render::{render, RenderGraph, RenderResources},
    viewport::Viewport,
    Clock, ClockStep, Ident,
};
use arcana_egui::{Egui, EguiRender};
use arcana_launcher::Start;
use egui::vec2;
use egui_file::FileDialog;
use hashbrown::HashMap;
use winit::{event::WindowEvent, event_loop::ControlFlow, window::Window};

fn main() {
    use tracing_subscriber::layer::SubscriberExt as _;

    if let Err(err) = tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            // .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .finish()
            .with(tracing_error::ErrorLayer::default()),
    ) {
        panic!("Failed to install tracing subscriber: {}", err);
    }

    let events = EventLoop::with_user_event().build().unwrap();
    let mut app = App::new(&events);
    let mut clock = Clock::new();
    let mut limiter = clock.ticker(60.hz());

    #[allow(deprecated)]
    events
        .run(move |event, el| {
            let step = clock.step();
            limiter.ticks(step.step);

            app.on_input(event);
            app.tick(step);

            if app.should_quit() {
                el.exit();
                return;
            }

            let next = limiter.next_tick().unwrap();
            let until = clock.stamp_instant(next);
            let now = clock.stamp_instant(clock.now());

            assert!(until - now < Duration::from_millis(100));

            el.set_control_flow(ControlFlow::WaitUntil(until));
        })
        .unwrap();
}

pub enum UserEvent {}

pub type Event = winit::event::Event<UserEvent>;
pub type EventLoop = winit::event_loop::EventLoop<UserEvent>;
pub type ActiveEventLoop = winit::event_loop::ActiveEventLoop;

struct ErrorDialog {
    title: String,
    message: String,
}

impl ErrorDialog {
    fn show(&self, cx: &egui::Context) -> bool {
        let title = egui::WidgetText::from(&*self.title);
        let message = egui::WidgetText::from(&*self.message);

        let mut close = false;
        egui::Window::new(title)
            .resizable(false)
            .collapsible(false)
            .show(cx, |ui| {
                ui.label(message);
                if ui.button("Ok").clicked() {
                    close = true;
                }
            });
        close
    }
}

enum AppDialog {
    NewProject(NewProject),
    OpenProject(FileDialog),
    AddEngine(FileDialog),
    Error(ErrorDialog),
}

enum AppChild {
    EditorBuilding(Child, PathBuf),
    EditorRunning(Child),
}

/// Editor app instance.
/// Contains state of the editor.
pub struct App {
    // App state is stored in World.
    world: World,

    graph: RenderGraph,
    resources: RenderResources,
    device: mev::Device,
    queue: mev::Queue,
    blink: BlinkAlloc,
    start: Start,
    profile: Profile,
    recent: HashMap<PathBuf, Result<Project, miette::Report>>,

    /// Open dialog.
    dialog: Option<AppDialog>,

    /// Running child app.
    /// When this is `Some`, laucher is not interactive,
    /// window is hidden.
    /// But event-loop is still running.
    ///
    /// When child app finishes, launcher is shown again.
    child: Option<AppChild>,

    should_quit: bool,
}

impl App {
    pub fn new(events: &EventLoop) -> Self {
        let (device, queue) = init_mev();

        let mut builder = World::builder();
        builder.register_external::<Window>();
        builder.register_component::<Egui>();
        builder.register_external::<mev::Surface>();

        let mut world = builder.build();
        let mut graph = RenderGraph::new();

        let builder = Window::default_attributes().with_title("Arcana Launcher");

        #[allow(deprecated)]
        let window = events
            .create_window(builder)
            .map_err(|err| miette::miette!("Failed to create Ed window: {err:?}"))
            .unwrap();

        let size = window.inner_size();
        let scale_factor = window.scale_factor();

        let egui = Egui::new(
            vec2(size.width as f32, size.height as f32),
            scale_factor as f32,
        );

        world.insert_resource(Viewport::new_window(window));
        world.insert_resource(egui);

        let target = EguiRender::build(None, &mut graph);
        graph.present(target);

        App {
            world: world.into(),
            graph,
            resources: RenderResources::default(),
            device,
            queue,
            blink: BlinkAlloc::new(),
            start: Start::new(),
            profile: Profile::Debug,
            recent: HashMap::new(),

            dialog: None,
            child: None,
            should_quit: false,
        }
    }

    pub fn on_input(&mut self, event: Event) {
        match event {
            Event::WindowEvent {
                event: WindowEvent::RedrawRequested,
                ..
            } => {
                self.render();
            }
            Event::WindowEvent { window_id, event } => {
                let world = self.world.local();

                if world.expect_resource_mut::<Viewport>().get_window().id() == window_id {
                    let mut egui = world.expect_resource_mut::<Egui>();
                    if let Ok(event) = ViewportInput::try_from(&event) {
                        egui.handle_event(&event);
                    }
                }

                match event {
                    WindowEvent::CloseRequested => {
                        self.should_quit = true;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn tick(&mut self, step: ClockStep) {
        let mut viewport = self.world.expect_resource_mut::<Viewport>();
        let window = viewport.get_window_mut();

        match self.child {
            None => {}
            Some(AppChild::EditorBuilding(ref mut child, _)) => match child.try_wait() {
                Err(err) => {
                    self.dialog = Some(AppDialog::Error(ErrorDialog {
                        title: "Failed to check if build finished".to_owned(),
                        message: err.to_string(),
                    }));
                    self.child = None;
                }
                Ok(Some(status)) => {
                    if status.success() {
                        match self.child.take() {
                            Some(AppChild::EditorBuilding(_, path)) => {
                                let project = self.recent.get(&path).unwrap().as_ref().unwrap();

                                match project.run_editor_non_blocking(self.profile) {
                                    Err(err) => {
                                        self.dialog = Some(AppDialog::Error(ErrorDialog {
                                            title: "Failed to run Arcana Ed".to_owned(),
                                            message: err.to_string(),
                                        }));
                                        self.child = None;
                                    }
                                    Ok(child) => {
                                        self.child = Some(AppChild::EditorRunning(child));
                                        window.set_visible(false);
                                        return;
                                    }
                                }
                            }
                            _ => unreachable!(),
                        }
                    } else {
                        self.dialog = Some(AppDialog::Error(ErrorDialog {
                            title: "Failed to build Arcana Ed".to_owned(),
                            message: format!("{}", status),
                        }));
                        self.child = None;
                    }
                }
                Ok(None) => {}
            },
            Some(AppChild::EditorRunning(ref mut child)) => {
                match child.try_wait() {
                    Err(err) => {
                        self.dialog = Some(AppDialog::Error(ErrorDialog {
                            title: "Failed to check if Arcana Ed closed".to_owned(),
                            message: err.to_string(),
                        }));
                        self.child = None;
                        window.set_visible(true);
                    }
                    Ok(Some(status)) => {
                        if !status.success() {
                            self.dialog = Some(AppDialog::Error(ErrorDialog {
                                title: "Arcana Ed exited with error".to_owned(),
                                message: format!("{}", status),
                            }));
                        }
                        self.child = None;
                        window.set_visible(true);
                    }
                    Ok(None) => {
                        // Editor is still running.
                        return;
                    }
                }
            }
        }

        enum Action {
            Quit,
            RunEditor(PathBuf),
        }

        let mut action = None;

        let mut egui = self.world.expect_resource_mut::<Egui>();

        egui.run(step.now, |cx| {
            egui::TopBottomPanel::top("Menu").show(cx, |ui| {
                ui.set_enabled(self.dialog.is_none() && self.child.is_none());

                ui.menu_button("File", |ui| {
                    let r = ui.button("New Project");
                    if r.clicked() {
                        let engine = self.start.list_engine_versions().first().cloned();
                        self.dialog = Some(AppDialog::NewProject(NewProject::new(engine)));
                        ui.close_menu();
                    } else {
                        r.on_hover_ui(|ui| {
                            ui.label("Create new project");
                        });
                    }

                    let r = ui.button("Open Project");
                    if r.clicked() {
                        let mut dialog: FileDialog =
                            FileDialog::select_folder(None).title("Open project");
                        dialog.open();
                        self.dialog = Some(AppDialog::OpenProject(dialog));

                        ui.close_menu();
                    } else {
                        r.on_hover_ui(|ui| {
                            ui.label("Create new project");
                        });
                    }

                    let r = ui.button("Add Engine path");
                    if r.clicked() {
                        let mut dialog: FileDialog =
                            FileDialog::select_folder(None).title("Add engine");
                        dialog.open();
                        self.dialog = Some(AppDialog::AddEngine(dialog));

                        ui.close_menu();
                    } else {
                        r.on_hover_ui(|ui| {
                            ui.label("Add new engine to Arcana Launcher");
                        });
                    }

                    let r = ui.button("Exit");
                    if r.clicked() {
                        action = Some(Action::Quit);
                        ui.close_menu();
                    } else {
                        r.on_hover_ui(|ui| {
                            ui.label("Exit Arcana Launcher");
                        });
                    }
                });
            });

            egui::TopBottomPanel::top("Controls").show(cx, |ui| {
                ui.set_enabled(self.dialog.is_none() && self.child.is_none());

                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(self.profile == Profile::Debug, "Debug")
                        .clicked()
                    {
                        self.profile = Profile::Debug;
                    }
                    if ui
                        .selectable_label(self.profile == Profile::Release, "Release")
                        .clicked()
                    {
                        self.profile = Profile::Release;
                    }
                });
            });

            let mut remove_recent = None;

            egui::CentralPanel::default().show(cx, |ui| {
                ui.set_enabled(self.dialog.is_none());

                let recent = self.start.recent();

                if recent.len() == 0 {
                    ui.vertical_centered(|ui| {
                        ui.allocate_space(egui::vec2(0.0, ui.available_height() * 0.5));
                        ui.label("No recent projects");

                        let r = ui.button("New Project");
                        if r.clicked() {
                            let engine = self.start.list_engine_versions().first().cloned();
                            self.dialog = Some(AppDialog::NewProject(NewProject::new(engine)));
                            ui.close_menu();
                        } else {
                            r.on_hover_ui(|ui| {
                                ui.label("Create new project");
                            });
                        }
                    });
                } else {
                    ui.vertical(|ui| {
                        for path in recent {
                            let result = match self.recent.entry(path.to_owned()) {
                                hashbrown::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                                hashbrown::hash_map::Entry::Vacant(entry) => {
                                    entry.insert(Project::open(&path))
                                }
                            };

                            match result {
                                Err(err) => {
                                    egui::Frame::group(ui.style())
                                        .stroke(egui::Stroke::new(1.0, egui::Color32::DARK_RED))
                                        .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                ui.add_enabled(false, egui::Button::new(
                                                    egui::RichText::from(
                                                        egui_phosphor::regular::FOLDER_NOTCH_OPEN,
                                                    )
                                                    .size(30.0),
                                                ));

                                                ui.vertical(|ui| {
                                                    ui.label(format!(
                                                        "cannot open project. {err:?}"
                                                    ));
                                                    ui.label(path.display().to_string());
                                                });
                                                let r = ui.button(egui_phosphor::regular::X);

                                                if r.clicked() {
                                                    remove_recent = Some(path.to_owned());
                                                } else {
                                                    r.on_hover_ui(|ui| {
                                                        ui.label("Remove from thisl list");
                                                    });
                                                }
                                            });
                                        });
                                }
                                Ok(project) => {
                                    egui::Frame::group(ui.style()).show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            let r = ui.add(egui::Button::new(
                                                egui::RichText::from(
                                                    egui_phosphor::regular::FOLDER_NOTCH_OPEN,
                                                )
                                                .size(30.0),
                                            ));

                                            ui.vertical(|ui| {
                                                ui.label(project.name().as_str());
                                                ui.label(project.root_path().display().to_string());
                                            });

                                            if r.clicked() {
                                                action = Some(Action::RunEditor(path.to_owned()));
                                            } else {
                                                r.on_hover_ui(|ui| {
                                                    ui.label("Open this project");
                                                });
                                            }
                                            let r = ui.button(egui_phosphor::regular::X);

                                            if r.clicked() {
                                                remove_recent = Some(path.to_owned());
                                            } else {
                                                r.on_hover_ui(|ui| {
                                                    ui.label("Remove from thisl list");
                                                });
                                            }
                                        });
                                    });
                                }
                            }
                        }
                    });
                }
            });

            if let Some(path) = remove_recent {
                self.start.remove_recent(&path);
            }

            match self.child {
                None => match self.dialog {
                    None => {}
                    Some(AppDialog::Error(ref error)) => {
                        if error.show(cx) {
                            self.dialog = None;
                            window.request_redraw();
                        }
                    }
                    Some(AppDialog::OpenProject(ref mut file_dialog)) => {
                        match file_dialog.show(cx).state() {
                            egui_file::State::Open => {}
                            egui_file::State::Closed | egui_file::State::Cancelled => {
                                self.dialog = None;
                            }
                            egui_file::State::Selected => match file_dialog.path() {
                                None => {
                                    self.dialog = None;
                                }
                                Some(path) => {
                                    let result = Project::open(path);
                                    let result = self
                                        .recent
                                        .entry(path.to_owned())
                                        .insert(result)
                                        .into_mut();

                                    match result {
                                        Err(err) => {
                                            self.dialog = Some(AppDialog::Error(ErrorDialog {
                                                title: "Failed to open project".to_owned(),
                                                message: err.to_string(),
                                            }));
                                        }
                                        Ok(project) => {
                                            let path = project.root_path().to_owned();
                                            self.start.add_recent(path.clone());

                                            action = Some(Action::RunEditor(path.to_owned()));
                                            self.dialog = None;
                                        }
                                    }
                                }
                            },
                        }
                    }
                    Some(AppDialog::AddEngine(ref mut file_dialog)) => {
                        match file_dialog.show(cx).state() {
                            egui_file::State::Open => {}
                            egui_file::State::Closed | egui_file::State::Cancelled => {
                                self.dialog = None;
                            }
                            egui_file::State::Selected => match file_dialog.path() {
                                None => {
                                    self.dialog = None;
                                }
                                Some(path) => match Dependency::from_path(path) {
                                    Some(engine) => {
                                        self.start.add_engine(engine);
                                        self.dialog = None;
                                    }
                                    None => {
                                        self.dialog = Some(AppDialog::Error(ErrorDialog {
                                            title: "Failed to add engine".to_owned(),
                                            message: "Invalid engine path".to_owned(),
                                        }));
                                    }
                                },
                            },
                        }
                    }
                    Some(AppDialog::NewProject(ref mut new_project)) => {
                        match new_project.show(&self.start, cx) {
                            None => {}
                            Some(None) => {
                                self.dialog = None;
                                window.request_redraw();
                            }
                            Some(Some(project)) => {
                                let path = project.root_path().to_owned();
                                self.recent.insert(path.clone(), Ok(project));
                                self.start.add_recent(path.clone());

                                self.dialog = None;
                                window.request_redraw();
                                action = Some(Action::RunEditor(path));
                            }
                        }
                    }
                },
                Some(AppChild::EditorBuilding(_, _)) => {
                    egui::Window::new("Preparing project")
                        .resizable(false)
                        .collapsible(false)
                        .show(cx, |ui| {
                            ui.label("Preparing project...");
                            ui.spinner();
                        });
                }
                Some(AppChild::EditorRunning(_)) => {
                    unreachable!()
                }
            }

            if cx.requested_repaint_last_frame() {
                window.request_redraw();
            }
        });

        match action {
            None => {}
            Some(Action::Quit) => {
                drop(viewport);
                drop(egui);
                self.should_quit = true;
            }
            Some(Action::RunEditor(path)) => {
                let project = self.recent.get(&path).unwrap().as_ref().unwrap();

                match project.build_editor_non_blocking(self.profile) {
                    Err(err) => {
                        self.dialog = Some(AppDialog::Error(ErrorDialog {
                            title: "Failed to run project".to_owned(),
                            message: err.to_string(),
                        }));
                    }
                    Ok(child) => {
                        self.child = Some(AppChild::EditorBuilding(child, path));
                    }
                };
            }
        }
    }

    fn render(&mut self) {
        render(
            &mut self.graph,
            &self.device,
            &mut self.queue,
            &self.blink,
            None,
            &mut self.world,
            &mut self.resources,
        );
    }

    fn should_quit(&self) -> bool {
        self.should_quit
    }
}

enum NewProjectDialog {
    Error(ErrorDialog),
    PickPath(FileDialog),
}

/// This widget is used to configure and create new project.
struct NewProject {
    /// Name of new project.
    ///
    /// If bad `Ident` is provided, project may not be created.
    name: String,

    /// Path to new project.
    /// This path is absolute and normalized.
    ///
    /// If empty, project may not be created.
    path: String,

    /// If true, advanced options are shown.
    advanced: bool,

    /// Chosen engine version.
    ///
    /// If none, project may not be created.
    engine: Option<Dependency>,

    /// Current dialog.
    dialog: Option<NewProjectDialog>,
}

impl NewProject {
    /// Creates new `NewProject` widget.
    fn new(engine: Option<Dependency>) -> Self {
        NewProject {
            name: String::new(),
            path: String::new(),
            advanced: false,
            engine,
            dialog: None,
        }
    }

    fn can_create_project(&self) -> bool {
        Ident::from_str(&self.name).is_ok() && !self.path.is_empty() && self.engine.is_some()
    }

    fn show(&mut self, start: &Start, cx: &egui::Context) -> Option<Option<Project>> {
        let mut create_project = false;
        let mut close_dialog = false;

        egui::Window::new("New project")
            .auto_sized()
            .default_pos(egui::pos2(50.0, 50.0))
            .collapsible(false)
            .show(cx, |ui| {
                ui.set_enabled(self.dialog.is_none());

                egui::Grid::new("new-project-settings")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        let cfg_name_layout = egui::Layout::right_to_left(egui::Align::Center);
                        ui.with_layout(cfg_name_layout, |ui| ui.label("Name"));
                        ui.text_edit_singleline(&mut self.name);
                        ui.end_row();

                        ui.with_layout(cfg_name_layout, |ui| ui.label("Path"));
                        ui.horizontal(|ui| {
                            ui.text_edit_singleline(&mut self.path);
                            let r = ui.small_button(egui_phosphor::regular::DOTS_THREE);
                            if r.clicked() {
                                let mut dialog =
                                    FileDialog::select_folder(None).title("Select project path");
                                dialog.open();
                                self.dialog = Some(NewProjectDialog::PickPath(dialog));
                            }
                        });
                        ui.end_row();

                        ui.checkbox(&mut self.advanced, "Advanced");
                        ui.end_row();

                        if self.advanced {
                            ui.with_layout(cfg_name_layout, |ui| ui.label("Engine"));
                            let mut cbox = egui::ComboBox::from_id_source("versions").width(300.0);

                            if let Some(v) = &self.engine {
                                cbox = cbox.selected_text(display_dependency(v));
                            }

                            cbox.show_ui(ui, |ui| {
                                for v in start.list_engine_versions() {
                                    ui.selectable_value(
                                        &mut self.engine,
                                        Some(v.clone()),
                                        display_dependency(v),
                                    );
                                }
                            });

                            let r = ui.small_button(egui_phosphor::regular::DOTS_THREE);
                            if r.clicked() {
                                let mut dialog =
                                    FileDialog::select_folder(None).title("Select engine path");
                                dialog.open();
                                self.dialog = Some(NewProjectDialog::PickPath(dialog));
                            }

                            ui.end_row();
                        }
                    });

                ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    let r = ui.add_enabled(self.can_create_project(), egui::Button::new("Create"));
                    create_project = r.clicked();

                    let r = ui.add(egui::Button::new("Cancel"));
                    close_dialog = r.clicked();
                });
            });

        match self.dialog {
            None => {}
            Some(NewProjectDialog::Error(ref error)) => {
                if error.show(cx) {
                    self.dialog = None;
                }
            }
            Some(NewProjectDialog::PickPath(ref mut file_dialog)) => {
                match file_dialog.show(cx).state() {
                    egui_file::State::Open => {}
                    egui_file::State::Closed | egui_file::State::Cancelled => {
                        self.dialog = None;
                    }
                    egui_file::State::Selected => {
                        if let Some(path) = file_dialog.path() {
                            self.path = path.display().to_string();
                        }
                        self.dialog = None;
                    }
                }
            }
        }

        if close_dialog {
            return Some(None);
        }

        if create_project {
            let result = Project::new(
                Ident::from_str(&self.name).unwrap(),
                self.path.as_ref(),
                self.engine.clone().unwrap(),
                true,
            );

            match result {
                Ok(project) => {
                    return Some(Some(project));
                }
                Err(err) => {
                    self.dialog = Some(NewProjectDialog::Error(ErrorDialog {
                        title: "Failed to create project".to_owned(),
                        message: err.to_string(),
                    }));
                }
            }
        }

        None
    }
}

fn display_dependency(dep: &Dependency) -> String {
    match dep {
        Dependency::Crates(v) => {
            format!("{}{}", egui_phosphor::regular::PACKAGE, v)
        }
        Dependency::Git { git, branch: None } => {
            if let Some(suffix) = git.strip_prefix("https://github.com/") {
                format!("{}{}", egui_phosphor::regular::GITHUB_LOGO, suffix)
            } else if let Some(suffix) = git.strip_prefix("https://gitlab.com/") {
                format!("{}{}", egui_phosphor::regular::GITLAB_LOGO, suffix)
            } else {
                git.clone()
            }
        }
        Dependency::Git {
            git,
            branch: Some(branch),
        } => {
            if let Some(suffix) = git.strip_prefix("https://github.com/") {
                format!(
                    "{}{}{}{}",
                    egui_phosphor::regular::GITHUB_LOGO,
                    suffix,
                    egui_phosphor::regular::GIT_BRANCH,
                    branch
                )
            } else if let Some(suffix) = git.strip_prefix("https://gitlab.com/") {
                format!(
                    "{}{}{}{}",
                    egui_phosphor::regular::GITLAB_LOGO,
                    suffix,
                    egui_phosphor::regular::GIT_BRANCH,
                    branch
                )
            } else {
                format!("{}{}{}", git, egui_phosphor::regular::GIT_BRANCH, branch)
            }
        }
        Dependency::Path { path } => {
            format!("{}{}", egui_phosphor::regular::FILE_CODE, path)
        }
    }
}

fn init_mev() -> (mev::Device, mev::Queue) {
    let instance = mev::Instance::load().expect("Failed to init graphics");

    let (device, mut queues) = instance
        .create(mev::DeviceDesc {
            idx: 0,
            queues: &[0],
            features: mev::Features::SURFACE,
        })
        .unwrap();
    let queue = queues.pop().unwrap();
    (device, queue)
}
