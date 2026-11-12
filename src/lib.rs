//! Storage accounting is independent of collection and rendering.
#![forbid(unsafe_code)]

pub mod calculations;
pub mod cli;
pub mod clock;
pub mod collect;
pub mod findings;
pub mod group;
pub mod join;
pub mod metrics;
pub mod model;
pub mod output;
pub mod pipeline;
pub mod policy;
pub mod qty;
pub mod report;
pub mod sort;
pub mod transport;
pub(crate) mod work;

#[cfg(test)]
#[path = "../tests/unit/performance.rs"]
mod performance;

#[cfg(test)]
#[path = "../tests/unit/properties.rs"]
mod property_examples;
