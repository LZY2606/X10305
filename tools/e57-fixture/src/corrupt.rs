use crate::budget::{BudgetReport, Budgets};
use crate::fixture::{generate, Fixture};
use crate::{ErrorLayer, FixtureCase, GateFailure, Stage};
use e57::E57Reader;
use std::io::Cursor;

/// Local corruptions applied to generated fixtures.
///
/// Each corruption modifies the fixture bytes after generation
/// (never the internal objects) to simulate damaged files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corruption {
    /// Flip a byte inside the compressed vector data,
    /// which invalidates the CRC checksum of the page.
    ChecksumFlip,
    /// Cut the file in the middle of the first data packet.
    TruncatedPacket,
    /// Point the XML offset in the file header beyond the end of the file.
    XmlOffsetOutOfBounds,
    /// Set the XML length in the file header beyond the parser byte budget.
    XmlLengthBeyondBudget,
}

impl Corruption {
    /// All corruptions in stable declaration order.
    pub const ALL: [Corruption; 4] = [
        Corruption::ChecksumFlip,
        Corruption::TruncatedPacket,
        Corruption::XmlOffsetOutOfBounds,
        Corruption::XmlLengthBeyondBudget,
    ];

    /// Stable name of the corruption, used in reports.
    pub fn name(self) -> &'static str {
        match self {
            Corruption::ChecksumFlip => "corrupt-checksum",
            Corruption::TruncatedPacket => "truncated-packet",
            Corruption::XmlOffsetOutOfBounds => "xml-offset-out-of-bounds",
            Corruption::XmlLengthBeyondBudget => "xml-length-beyond-budget",
        }
    }

    /// The fixture case used as base material for this corruption.
    pub fn base_case(self) -> FixtureCase {
        // The multi-prototype fixture contains all relevant structures:
        // header, compressed vector section with data packets and XML.
        FixtureCase::MultiPrototype
    }
}

/// Report of one corruption rejection run.
///
/// Records the caller-visible results of both public detection
/// paths (CRC validation and full parsing) with their error layers.
#[derive(Clone, Debug, PartialEq)]
pub struct RejectionReport {
    /// The fixture case that was corrupted.
    pub case: FixtureCase,
    /// The applied corruption.
    pub corruption: Corruption,
    /// The fixed seed of the underlying fixture.
    pub seed: u64,
    /// Error layer of the CRC validation path, if it rejected the file.
    pub crc_layer: Option<ErrorLayer>,
    /// Error message of the CRC validation path, if it rejected the file.
    pub crc_message: Option<String>,
    /// Error layer of the full parsing path, if it rejected the file.
    pub parse_layer: Option<ErrorLayer>,
    /// Error message of the full parsing path, if it rejected the file.
    pub parse_message: Option<String>,
    /// Observed resource usage and budgets.
    pub budgets: BudgetReport,
}

impl RejectionReport {
    /// True if at least one of the public detection paths rejected the file.
    pub fn rejected(&self) -> bool {
        self.crc_layer.is_some() || self.parse_layer.is_some()
    }
}

/// Applies a corruption to the bytes of a generated fixture.
pub fn apply_corruption(fixture: &Fixture, corruption: Corruption) -> Vec<u8> {
    let mut bytes = fixture.bytes.clone();
    match corruption {
        Corruption::ChecksumFlip => {
            let offset = data_offset(fixture).unwrap_or(bytes.len() as u64 / 2);
            let index = (offset as usize).min(bytes.len() - 1);
            bytes[index] ^= 0xFF;
        }
        Corruption::TruncatedPacket => {
            // Cut the file one byte behind the first data packet header:
            // section header (32 bytes) + packet header (6 bytes) + 1 byte.
            let offset = data_offset(fixture).unwrap_or(48) + 32 + 6 + 1;
            bytes.truncate((offset as usize).min(bytes.len()));
        }
        Corruption::XmlOffsetOutOfBounds => {
            let out_of_bounds = bytes.len() as u64 + 4096;
            bytes[24..32].copy_from_slice(&out_of_bounds.to_le_bytes());
        }
        Corruption::XmlLengthBeyondBudget => {
            let beyond_budget = e57::MAX_XML_SIZE as u64 + 1;
            bytes[32..40].copy_from_slice(&beyond_budget.to_le_bytes());
        }
    }
    bytes
}

/// Physical file offset of the first point cloud binary section,
/// looked up through the public reader API.
fn data_offset(fixture: &Fixture) -> Option<u64> {
    let reader = E57Reader::new(Cursor::new(fixture.bytes.as_slice())).ok()?;
    reader.pointclouds().first().map(|pc| pc.file_offset)
}

/// Runs one corruption case: generates the base fixture from the
/// fixed seed, applies the corruption and checks that the public
/// detection paths reject the damaged file.
///
/// A corruption that passes both detection paths is a gate failure,
/// because damaged input must never be guessed as success.
pub fn run_rejection(
    seed: u64,
    corruption: Corruption,
    budgets: &Budgets,
) -> Result<RejectionReport, GateFailure> {
    let case = corruption.base_case();
    let fixture = generate(case, seed)?;
    let corrupted = apply_corruption(&fixture, corruption);

    // Detection path 1: standalone CRC validation (public API)
    let crc_result = E57Reader::validate_crc(Cursor::new(corrupted.as_slice())).map(|_| ());

    // Detection path 2: full parsing including all point data (public API)
    let parse_result = parse_fully(&corrupted);

    let report = RejectionReport {
        case,
        corruption,
        seed,
        crc_layer: crc_result.as_ref().err().map(ErrorLayer::of),
        crc_message: crc_result.err().map(|err| err.to_string()),
        parse_layer: parse_result.as_ref().err().map(ErrorLayer::of),
        parse_message: parse_result.err().map(|err| err.to_string()),
        budgets: BudgetReport::new(corrupted.len(), budgets),
    };

    if !report.rejected() {
        return Err(GateFailure::new(
            case.name(),
            seed,
            Stage::Corruption,
            format!("corruption '{}'", corruption.name()),
            String::from("corrupted input was accepted as valid, expected explicit rejection"),
        )
        .with_budgets(report.budgets.clone()));
    }
    Ok(report)
}

/// Parses the file and reads all point data through the public API.
fn parse_fully(bytes: &[u8]) -> e57::Result<()> {
    let mut reader = E57Reader::new(Cursor::new(bytes))?;
    let pointclouds = reader.pointclouds();
    for pc in pointclouds {
        let iter = reader.pointcloud_raw(&pc)?;
        for point in iter {
            point?;
        }
    }
    Ok(())
}
