use std::{cell::RefCell, slice::Iter};

use hashbrown::HashSet;

use crate::{arena::Arena, make_id, stid::WithStid, Stid};

use super::target::{Target, TargetHub, TargetId};

make_id!(pub JobId);

/// Descroption of job creating a target.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct JobCreateDesc {
    /// Target name.
    pub name: String,

    /// Target type.
    pub ty: Stid,
}

impl JobCreateDesc {
    pub fn new<T: WithStid>(name: impl Into<String>) -> Self {
        JobCreateDesc {
            name: name.into(),
            ty: T::stid(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct JobUpdateDesc {
    /// Target type.
    pub ty: Stid,
}

impl JobUpdateDesc {
    pub fn new<T: WithStid>() -> Self {
        JobUpdateDesc { ty: T::stid() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct JobReadDesc {
    /// Target type.
    pub ty: Stid,
}

impl JobReadDesc {
    pub fn new<T: WithStid>() -> Self {
        JobReadDesc { ty: T::stid() }
    }
}

/// Job description.
/// A set of targets a job creates, updates and reads.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct JobDesc {
    /// List of targets job reads.
    /// They are inputs of the job.
    pub reads: Vec<JobReadDesc>,

    /// List of targets job updates.
    /// They are inputs and outputs of the job.
    pub updates: Vec<JobUpdateDesc>,

    /// List of targets job creates.
    /// They are outputs of the job.
    pub creates: Vec<JobCreateDesc>,
}

#[doc(hidden)]
#[macro_export]
macro_rules! add_job_desc {
    (($reads:ident, $updates:ident, $creates:ident)) => {};
    (($reads:ident, $updates:ident, $creates:ident) $ty:ty $($rest:tt)+) => {
        $reads.push($crate::work::JobReadDesc::new::< $ty >());
        add_job_desc!(($reads, $updates, $creates) $($rest)+);
    };
    (($reads:ident, $updates:ident, $creates:ident) mut $ty:ty $($rest:tt)+) => {
        $crate::work::JobUpdateDesc::new::< $ty >(),
        add_job_desc!(($reads, $updates, $creates) $($rest)+);
    };
    (($reads:ident, $updates:ident, $creates:ident) +$ty:ty => $name:expr $($rest:tt)+) => {
        $crate::work::JobCreateDesc::new::< $ty >($name),
        add_job_desc!(($reads, $updates, $creates) $($rest)+);
    };
}

#[macro_export]
macro_rules! job_desc {
    ($($descs:tt)*) => {{
        let mut reads = Vec::new();
        let mut updates = Vec::new();
        let mut creates = Vec::new();
        $crate::add_job_desc!((reads, updates, creates) $($descs)*);
        $crate::work::JobDesc {
            reads,
            updates,
            creates,
        }
    }};
}

impl JobDesc {
    /// Returns input stable type ID by index.
    pub fn input_type(&self, input: usize) -> Stid {
        if input < self.updates.len() {
            self.updates[input].ty
        } else {
            self.reads[input - self.updates.len()].ty
        }
    }

    /// Returns output stable type ID by index.
    pub fn output_type(&self, output: usize) -> Stid {
        if output < self.creates.len() {
            self.creates[output].ty
        } else {
            self.updates[output - self.creates.len()].ty
        }
    }

    /// Returns output name by index.
    pub fn output_name(&self, output: usize) -> Option<&str> {
        if output < self.creates.len() {
            Some(&self.creates[output].name)
        } else {
            None
        }
    }
}

pub trait Job {
    /// First phase of a job is planning.
    ///
    /// This phase is responsible for:
    /// - Determining which jobs to run
    /// - Compute resource description for each job
    /// - Allocate resources
    ///
    /// This phase is executed for each frame, so considered hot path.
    /// It is important to keep it simple and fast,
    /// keep allocations to minimum and reuse as much as possible.
    fn plan(&mut self, planner: Planner<'_>, world: &mut WorldLocal);

    /// Second phase of a job is execution.
    ///
    /// This phase is responsible for recording commands.
    /// It fetches pre-allocated target resources and
    /// does anything necessary to record commands into command buffers:
    /// - Creating pipelines
    /// - Binding resources
    /// - Recording draw/dispatch calls
    fn exec(&mut self, runner: Exec<'_>, world: &mut WorldLocal);
}

pub struct JobCreateTarget {
    /// Target name.
    pub name: String,

    /// Target type.
    pub ty: Stid,

    /// Assigned target id.
    pub id: Option<TargetId>,
}

pub struct JobUpdateTarget {
    /// Target type.
    pub ty: Stid,

    /// Assigned target id.
    pub id: Option<TargetId>,

    /// Job ID that outputs this target.
    pub dep_id: Option<JobId>,
}

pub struct JobReadTarget {
    /// Target type.
    pub ty: Stid,

    /// Assigned target id.
    pub id: Option<TargetId>,

    /// Job ID that outputs this target.
    pub dep_id: Option<JobId>,
}

pub struct Planner<'a> {
    /// List of targets updates of the job correspond to.
    updates: Iter<'a, JobUpdateTarget>,

    /// List of targets creates of the job correspond to.
    creates: Iter<'a, JobCreateTarget>,

    /// List of targets reads of the job correspond to.
    reads: Iter<'a, JobReadTarget>,

    /// Where all targets live.
    hub: &'a mut TargetHub,

    /// Set of selected jobs.
    selected_jobs: &'a mut HashSet<JobId>,

    device: mev::Device,
}

impl Planner<'_> {
    /// Provide resource description for next input.
    /// Allows merging resource description other readers.
    pub fn create<T>(&mut self) -> Option<&T::Info>
    where
        T: Target,
    {
        let create = self.creates.next().expect("No more creates");
        assert_eq!(create.ty, Stid::of::<T>());
        self.hub
            .plan_create::<T>(create.id?, &create.name, &self.device)
    }

    /// Fetcehs resource description for next update.
    pub fn update<T>(&mut self) -> Option<&T::Info>
    where
        T: Target,
    {
        let update = self.updates.next().expect("No more updates");
        assert_eq!(update.ty, Stid::of::<T>());
        let info = self.hub.plan_update::<T>(update.id?)?;

        if let Some(dep_id) = update.dep_id {
            self.selected_jobs.insert(dep_id);
        }

        Some(info)
    }

    /// Provide resource description for next input.
    /// Allows merging resource description other readers.
    pub fn read<T>(&mut self, info: T::Info)
    where
        T: Target,
    {
        let read = self.reads.next().expect("No more reads");
        assert_eq!(read.ty, Stid::of::<T>());
        let Some(id) = read.id else {
            return;
        };
        self.hub.plan_read::<T>(id, info);

        if let Some(dep_id) = read.dep_id {
            self.selected_jobs.insert(dep_id);
        }
    }
}

pub struct Exec<'a> {
    /// List of targets updates of the job correspond to.
    updates: Iter<'a, JobUpdateTarget>,

    /// List of targets creates of the job correspond to.
    creates: Iter<'a, JobCreateTarget>,

    /// List of targets reads of the job correspond to.
    reads: Iter<'a, JobReadTarget>,

    /// Where all targets live.
    hub: &'a mut TargetHub,

    device: mev::Device,
    queue: RefCell<&'a mut mev::Queue>,

    /// Arena for command buffers.
    /// This allows taking references to newly allocated command encoders
    /// And after job is done, collecting them in allocated order.
    cbufs: &'a Arena<mev::CommandEncoder>,
}

impl Exec<'_> {
    /// Fetches next resource to update.
    ///
    /// Returns none if not connected to next input.
    pub fn update<T>(&mut self) -> Option<&T>
    where
        T: Target,
    {
        let update = self.updates.next().expect("No more updates");
        self.hub.get::<T>(update.id?)
    }

    /// Fetches next resource to create.
    ///
    /// Returns none if not connected.
    pub fn create<T>(&mut self) -> Option<&T>
    where
        T: Target,
    {
        let create = self.creates.next().expect("No more creates");
        self.hub.get::<T>(create.id?)
    }

    /// Fetches next resource to read.
    ///
    /// Returns none if not connected.
    pub fn read<T>(&mut self) -> Option<&T>
    where
        T: Target,
    {
        let read = self.reads.next().expect("No more reads");
        self.hub.get::<T>(read.id?)
    }

    /// Allocates new command encoder.
    /// It will be automatically submitted to this job's queue.
    ///
    /// Returned reference is bound to this `Exec`'s borrow,
    /// so make sure to fetch target references before calling this.
    pub fn new_encoder(&self) -> &mut mev::CommandEncoder {
        let encoder = self.queue.borrow_mut().new_command_encoder().unwrap();
        self.cbufs.put(encoder)
    }

    /// Returns reference to device.
    pub fn device(&self) -> &mev::Device {
        &self.device
    }
}

pub struct JobNode {
    pub(super) job: Box<dyn Job>,
    pub(super) updates: Vec<JobUpdateTarget>,
    pub(super) creates: Vec<JobCreateTarget>,
    pub(super) reads: Vec<JobReadTarget>,
}

impl JobNode {
    /// Construct new job node from description and job instance.
    pub fn new(desc: JobDesc, job: Box<dyn Job>) -> Self {
        JobNode {
            job,
            updates: desc
                .updates
                .into_iter()
                .map(|u| JobUpdateTarget {
                    ty: u.ty,
                    id: None,
                    dep_id: None,
                })
                .collect(),
            creates: desc
                .creates
                .into_iter()
                .map(|c| JobCreateTarget {
                    ty: c.ty,
                    name: c.name,
                    id: None,
                })
                .collect(),
            reads: desc
                .reads
                .into_iter()
                .map(|c| JobReadTarget {
                    ty: c.ty,
                    id: None,
                    dep_id: None,
                })
                .collect(),
        }
    }

    pub(super) fn plan(
        &mut self,
        hub: &mut TargetHub,
        selected_jobs: &mut HashSet<JobId>,
        device: mev::Device,
        world: &mut WorldLocal,
    ) {
        let planner = Planner {
            updates: self.updates.iter(),
            creates: self.creates.iter(),
            reads: self.reads.iter(),
            hub,
            selected_jobs,
            device,
        };
        self.job.plan(planner, world);
    }

    pub(super) fn exec(
        &mut self,
        hub: &mut TargetHub,
        device: mev::Device,
        queue: &mut mev::Queue,
        cbufs: &Arena<mev::CommandEncoder>,
        world: &mut WorldLocal,
    ) {
        let exec = Exec {
            updates: self.updates.iter(),
            creates: self.creates.iter(),
            reads: self.reads.iter(),
            hub,
            device,
            queue: RefCell::new(queue),
            cbufs,
        };

        self.job.exec(exec, world);
    }
}