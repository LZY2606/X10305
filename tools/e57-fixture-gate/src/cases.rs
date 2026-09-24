//! Enumeration of the fixed input set used by the fixture gate.

use crate::budget::Layer;

/// The fixed, named counter-example materials of the gate.
///
/// The names intentionally describe the E57 contract they exercise so a
/// failing test name directly identifies the broken contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FixtureCase {
    /// Point cloud with zero records but a full prototype.
    ZeroRecords,
    /// Point cloud with many different prototype field kinds.
    MultiplePrototypeFields,
    /// Full range 64 bit integer coordinate fields.
    ExtremeIntegers,
    /// Scaled integer coordinate with scale and offset.
    ScaledInteger,
    /// Data page with a damaged checksum.
    CorruptChecksum,
    /// File cut in the middle of the packet stream.
    TruncatedPacket,
    /// Header XML pointer that references an offset past EOF.
    XmlPointerOutOfBounds,
}

impl FixtureCase {
    pub const ALL: [FixtureCase; 7] = [
        FixtureCase::ZeroRecords,
        FixtureCase::MultiplePrototypeFields,
        FixtureCase::ExtremeIntegers,
        FixtureCase::ScaledInteger,
        FixtureCase::CorruptChecksum,
        FixtureCase::TruncatedPacket,
        FixtureCase::XmlPointerOutOfBounds,
    ];

    /// Parses the stable snake case identifier. Unknown ids are
    /// reported explicitly instead of being guessed as some other case.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.id() == id)
    }

    pub fn id(self) -> &'static str {
        match self {
            FixtureCase::ZeroRecords => "zero_records",
            FixtureCase::MultiplePrototypeFields => "multiple_prototype_fields",
            FixtureCase::ExtremeIntegers => "extreme_integers",
            FixtureCase::ScaledInteger => "scaled_integer",
            FixtureCase::CorruptChecksum => "corrupt_checksum",
            FixtureCase::TruncatedPacket => "truncated_packet",
            FixtureCase::XmlPointerOutOfBounds => "xml_pointer_out_of_bounds",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            FixtureCase::ZeroRecords => "point cloud with zero records",
            FixtureCase::MultiplePrototypeFields => {
                "point cloud with many prototype field kinds"
            }
            FixtureCase::ExtremeIntegers => "full range 64 bit integer records",
            FixtureCase::ScaledInteger => "scaled integer records with scale and offset",
            FixtureCase::CorruptChecksum => "binary page with damaged CRC checksum",
            FixtureCase::TruncatedPacket => "file truncated inside the packet stream",
            FixtureCase::XmlPointerOutOfBounds => {
                "header XML pointer referencing an offset behind EOF"
            }
        }
    }

    /// Accepted fixtures must survive the full roundtrip. Rejected
    /// fixtures must be refused through the existing error hierarchy.
    pub fn is_accepted(self) -> bool {
        matches!(
            self,
            FixtureCase::ZeroRecords
                | FixtureCase::MultiplePrototypeFields
                | FixtureCase::ExtremeIntegers
                | FixtureCase::ScaledInteger
        )
    }

    /// Error layer a conforming parser must use when rejecting the case.
    pub fn expected_layer(self) -> Layer {
        match self {
            FixtureCase::CorruptChecksum | FixtureCase::TruncatedPacket => Layer::Read,
            FixtureCase::XmlPointerOutOfBounds => Layer::Read,
            _ => Layer::Accepted,
        }
    }

    /// Whether the standalone CRC entry is a meaningful first probe.
    pub fn probe_crc_first(self) -> bool {
        // A pointer behind EOF and a file cut between pages cannot be
        // observed at the CRC layer in a way that is specific to the
        // mutation, so those cases probe the regular open path first.
        !matches!(
            self,
            FixtureCase::XmlPointerOutOfBounds | FixtureCase::TruncatedPacket
        )
    }
}
