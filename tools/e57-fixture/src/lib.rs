//! Small and complete E57 fixture generation and verification tool.
//!
//! This crate builds tiny E57 files fully in memory from a fixed seed
//! (no external point cloud data is required) using only the public
//! writing API of the `e57` crate. The generated fixtures are then
//! verified through the public reading API of the `e57` crate:
//!
//! * XML section round-trip (semantic tree comparison, not raw bytes)
//! * Binary section round-trip (header and section invariants)
//! * Compressed vector round-trip (record summary comparison)
//! * Checksum validation of the CRC page layer
//! * Local corruption handling (checksum, truncation, XML pointer)
//!
//! All verification goes through the same production code paths that
//! regular users of the `e57` crate use. Nothing is parsed or written
//! twice, this crate only orchestrates the public entry points.
//!
//! Every failure is reported with the first violation location, the
//! fixed seed for minimal reproduction and the parser budgets
//! (bytes, records and recursion depth) that were in effect.

mod budget;
mod corrupt;
mod fixture;
mod gate;
mod memfile;
mod prng;
mod summary;
mod verify;

pub use budget::{BudgetReport, Budgets};
pub use corrupt::{apply_corruption, run_rejection, Corruption, RejectionReport};
pub use fixture::{generate, generate_into, Fixture, FixtureCase};
pub use gate::{run_gate, GateReport};
pub use memfile::{FlakyWriter, MemFile};
pub use prng::SplitMix64;
pub use summary::{compare_summaries, PointCloudSummary, SemanticSummary, Violation};
pub use verify::{verify, verify_bytes, RoundTripReport};

use std::fmt::{Display, Formatter};

/// The fixed default seed used for all fixture generation.
/// Fixtures are fully determined by this seed and the case name.
pub const DEFAULT_SEED: u64 = 0xE57F_1C7A_5EED_0001;

/// Stages of the fixture gate, in execution order.
/// The stage of a failure tells the caller which part of the
/// round-trip or corruption handling was violated first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// Fixture generation through the public writer API.
    Generate,
    /// CRC checksum validation of the paged file layer.
    Checksum,
    /// Extraction and parsing of the XML section.
    XmlSection,
    /// Header and binary section invariants.
    BinarySection,
    /// Compressed vector packet decoding.
    CompressedVector,
    /// Semantic tree comparison (XML metadata semantics).
    SemanticTree,
    /// Record summary comparison (point data semantics).
    RecordSummary,
    /// Resource budget enforcement (bytes, records, recursion).
    Budget,
    /// Local corruption detection.
    Corruption,
}

impl Stage {
    pub fn name(self) -> &'static str {
        match self {
            Stage::Generate => "generate",
            Stage::Checksum => "checksum",
            Stage::XmlSection => "xml-section",
            Stage::BinarySection => "binary-section",
            Stage::CompressedVector => "compressed-vector",
            Stage::SemanticTree => "semantic-tree",
            Stage::RecordSummary => "record-summary",
            Stage::Budget => "budget",
            Stage::Corruption => "corruption",
        }
    }
}

/// The error layer of the public `e57` error model.
/// Used to classify rejections of corrupted fixtures according to
/// the existing error layering of the library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorLayer {
    Invalid,
    Read,
    Write,
    NotImplemented,
    Internal,
}

impl ErrorLayer {
    pub fn name(self) -> &'static str {
        match self {
            ErrorLayer::Invalid => "Invalid",
            ErrorLayer::Read => "Read",
            ErrorLayer::Write => "Write",
            ErrorLayer::NotImplemented => "NotImplemented",
            ErrorLayer::Internal => "Internal",
        }
    }

    /// Classifies an error of the public `e57` error model into its layer.
    pub fn of(err: &e57::Error) -> Self {
        match err {
            e57::Error::Invalid { .. } => ErrorLayer::Invalid,
            e57::Error::Read { .. } => ErrorLayer::Read,
            e57::Error::Write { .. } => ErrorLayer::Write,
            e57::Error::NotImplemented { .. } => ErrorLayer::NotImplemented,
            e57::Error::Internal { .. } => ErrorLayer::Internal,
            // Future variants of the non-exhaustive error enum are
            // classified as internal until the gate learns about them.
            _ => ErrorLayer::Internal,
        }
    }
}

/// A single fixture gate failure with full context.
///
/// The display implementation always includes the case, the seed
/// (for minimal reproduction), the stage, the first violation
/// location and the parser budgets that were in effect.
#[derive(Clone, Debug)]
pub struct GateFailure {
    /// Name of the fixture case that failed.
    pub case: String,
    /// Fixed seed that reproduces the failure.
    pub seed: u64,
    /// Gate stage in which the failure was detected.
    pub stage: Stage,
    /// First violation location (for example "pointcloud 0, record 3, field 'cartesianX'").
    pub location: String,
    /// Human readable details, including expected and actual values.
    pub detail: String,
    /// Parser budgets in effect, if they could be observed.
    /// Boxed to keep the failure type small.
    pub budgets: Option<Box<BudgetReport>>,
}

impl GateFailure {
    pub(crate) fn new(
        case: &str,
        seed: u64,
        stage: Stage,
        location: String,
        detail: String,
    ) -> Self {
        Self {
            case: case.to_owned(),
            seed,
            stage,
            location,
            detail,
            budgets: None,
        }
    }

    pub(crate) fn with_budgets(mut self, budgets: BudgetReport) -> Self {
        self.budgets = Some(Box::new(budgets));
        self
    }
}

impl Display for GateFailure {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        write!(
            f,
            "fixture gate failure: case '{}' (seed {:#018x}) violated at stage '{}' first at {}: {}",
            self.case,
            self.seed,
            self.stage.name(),
            self.location,
            self.detail
        )?;
        if let Some(budgets) = &self.budgets {
            write!(f, "\n{budgets}")?;
        }
        Ok(())
    }
}

impl std::error::Error for GateFailure {}
