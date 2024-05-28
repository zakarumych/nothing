//! This module provides a type that can be used to replace `TypeId` and be guarnateed to be stable.
//!
//! `Stid` is 128bit long semi-random value that is associated with a type
//! via derive macro or manual implementation of `WithStid` trait.
//!

use arcana_proc::with_stid;
use gametime::TimeSpan;

crate::make_id! {
    /// Stable Type Identifier.
    ///
    /// Identifier is assigned via `WithStid` trait.
    /// The trait can be implemented manually, derived or implemented by `with_stid!` macro.
    ///
    /// Derive macro and proc-macro allow specifying the identifier value.
    /// If not specified, the value is generated by hashing macro input with stable hash.
    ///
    /// Generated identifier will always have MSB set to 1.
    /// This ensures that manually set identifiers with MSB set to 0 will never collide with generated ones.
    pub Stid;
}

/// Trait for types that have stable identifier.
/// Derive it, implementa manually or use `with_stid!` macro.
pub trait WithStid: 'static {
    fn stid() -> Stid
    where
        Self: Sized;

    fn stid_dyn(&self) -> Stid;
}

impl Stid {
    pub fn of<T>() -> Self
    where
        T: WithStid,
    {
        T::stid()
    }

    pub fn of_val<T>(value: &T) -> Self
    where
        T: WithStid + ?Sized,
    {
        value.stid_dyn()
    }
}

with_stid!(u8 = 0x0000_0000_0000_0001);
with_stid!(u16 = 0x0000_0000_0000_0002);
with_stid!(u32 = 0x0000_0000_0000_0003);
with_stid!(u64 = 0x0000_0000_0000_0004);
with_stid!(i8 = 0x0000_0000_0000_0005);
with_stid!(i16 = 0x0000_0000_0000_0006);
with_stid!(i32 = 0x0000_0000_0000_0007);
with_stid!(i64 = 0x0000_0000_0000_0008);
with_stid!(f32 = 0x0000_0000_0000_0009);
with_stid!(f64 = 0x0000_0000_0000_000A);

with_stid!(TimeSpan = 0x0000_0000_0000_00041);
with_stid!(::edict::entity::EntityId = 0x0000_0000_0000_0042);
