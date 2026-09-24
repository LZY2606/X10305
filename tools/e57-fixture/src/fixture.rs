use crate::prng::SplitMix64;
use crate::summary::{canonical_record, values_checksum, PointCloudSummary, SemanticSummary};
use crate::{GateFailure, MemFile, Stage};
use e57::{E57Writer, RawValues, Record, RecordDataType, RecordName, RecordValue};
use std::f64::consts::PI;
use std::io::{Read, Seek, Write};

/// The fixed set of small fixture cases.
///
/// Each case is generated from a fixed seed through the public
/// writer API and needs no external point cloud data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FixtureCase {
    /// Point cloud with zero records.
    ZeroRecords,
    /// Point cloud with many different prototype fields.
    MultiPrototype,
    /// Point cloud with extreme full-range integer values.
    ExtremeIntegers,
    /// Point cloud with scaled integer values.
    ScaledInteger,
}

impl FixtureCase {
    /// All fixture cases in stable declaration order.
    pub const ALL: [FixtureCase; 4] = [
        FixtureCase::ZeroRecords,
        FixtureCase::MultiPrototype,
        FixtureCase::ExtremeIntegers,
        FixtureCase::ScaledInteger,
    ];

    /// Stable name of the case, used in reports and on the command line.
    pub fn name(self) -> &'static str {
        match self {
            FixtureCase::ZeroRecords => "zero-records",
            FixtureCase::MultiPrototype => "multi-prototype",
            FixtureCase::ExtremeIntegers => "extreme-integers",
            FixtureCase::ScaledInteger => "scaled-integer",
        }
    }

    /// Looks up a case by its stable name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.name() == name)
    }

    fn seed_salt(self) -> u64 {
        match self {
            FixtureCase::ZeroRecords => 0x5eed_0001,
            FixtureCase::MultiPrototype => 0x5eed_0002,
            FixtureCase::ExtremeIntegers => 0x5eed_0003,
            FixtureCase::ScaledInteger => 0x5eed_0004,
        }
    }

    fn point_count(self) -> usize {
        match self {
            FixtureCase::ZeroRecords => 0,
            FixtureCase::MultiPrototype => 37,
            FixtureCase::ExtremeIntegers => 9,
            FixtureCase::ScaledInteger => 41,
        }
    }

    fn prototype(self) -> Vec<Record> {
        match self {
            FixtureCase::ZeroRecords => vec![
                Record::CARTESIAN_X_F64,
                Record::CARTESIAN_Y_F64,
                Record::CARTESIAN_Z_F64,
            ],
            FixtureCase::MultiPrototype => vec![
                Record::CARTESIAN_X_F64,
                Record::CARTESIAN_Y_F64,
                Record::CARTESIAN_Z_F64,
                Record::CARTESIAN_INVALID_STATE,
                Record::SPHERICAL_RANGE_F32,
                Record::SPHERICAL_AZIMUTH_F32,
                Record::SPHERICAL_ELEVATION_F32,
                Record::COLOR_RED_U8,
                Record::COLOR_GREEN_U8,
                Record::COLOR_BLUE_U8,
                Record {
                    name: RecordName::IsColorInvalid,
                    data_type: RecordDataType::Integer { min: 0, max: 1 },
                },
                Record::INTENSITY_U16,
                Record {
                    name: RecordName::IsIntensityInvalid,
                    data_type: RecordDataType::Integer { min: 0, max: 1 },
                },
                Record {
                    name: RecordName::RowIndex,
                    data_type: RecordDataType::Integer { min: 0, max: 1023 },
                },
                Record {
                    name: RecordName::ColumnIndex,
                    data_type: RecordDataType::Integer { min: 0, max: 1023 },
                },
                Record {
                    name: RecordName::ReturnCount,
                    data_type: RecordDataType::Integer { min: 0, max: 7 },
                },
                Record {
                    name: RecordName::ReturnIndex,
                    data_type: RecordDataType::Integer { min: 0, max: 7 },
                },
                Record {
                    name: RecordName::TimeStamp,
                    data_type: RecordDataType::F64,
                },
                Record {
                    name: RecordName::IsTimeStampInvalid,
                    data_type: RecordDataType::Integer { min: 0, max: 1 },
                },
            ],
            FixtureCase::ExtremeIntegers => {
                let full_range = RecordDataType::Integer {
                    min: i64::MIN,
                    max: i64::MAX,
                };
                vec![
                    Record {
                        name: RecordName::CartesianX,
                        data_type: full_range.clone(),
                    },
                    Record {
                        name: RecordName::CartesianY,
                        data_type: full_range.clone(),
                    },
                    Record {
                        name: RecordName::CartesianZ,
                        data_type: full_range,
                    },
                ]
            }
            FixtureCase::ScaledInteger => {
                let scaled = RecordDataType::ScaledInteger {
                    min: -100_000,
                    max: 100_000,
                    scale: 0.001,
                    offset: -10.0,
                };
                vec![
                    Record {
                        name: RecordName::CartesianX,
                        data_type: scaled.clone(),
                    },
                    Record {
                        name: RecordName::CartesianY,
                        data_type: scaled.clone(),
                    },
                    Record {
                        name: RecordName::CartesianZ,
                        data_type: scaled,
                    },
                    Record {
                        name: RecordName::Intensity,
                        data_type: RecordDataType::ScaledInteger {
                            min: 0,
                            max: 2047,
                            scale: 1.0,
                            offset: 0.0,
                        },
                    },
                ]
            }
        }
    }

    fn points(self, prng: &mut SplitMix64) -> Vec<RawValues> {
        let count = self.point_count();
        let mut points = Vec::with_capacity(count);
        for index in 0..count {
            points.push(self.point(prng, index));
        }
        points
    }

    fn point(self, prng: &mut SplitMix64, index: usize) -> RawValues {
        match self {
            FixtureCase::ZeroRecords => Vec::new(),
            FixtureCase::MultiPrototype => vec![
                RecordValue::Double(prng.next_f64() * 2000.0 - 1000.0),
                RecordValue::Double(prng.next_f64() * 2000.0 - 1000.0),
                RecordValue::Double(prng.next_f64() * 2000.0 - 1000.0),
                RecordValue::Integer(0),
                RecordValue::Single((prng.next_f64() * 500.0) as f32),
                RecordValue::Single((prng.next_f64() * 2.0 * PI - PI) as f32),
                RecordValue::Single((prng.next_f64() * PI - PI / 2.0) as f32),
                RecordValue::Integer(prng.next_below(256) as i64),
                RecordValue::Integer(prng.next_below(256) as i64),
                RecordValue::Integer(prng.next_below(256) as i64),
                RecordValue::Integer(0),
                RecordValue::Integer(prng.next_below(65536) as i64),
                RecordValue::Integer(0),
                RecordValue::Integer((index % 16) as i64),
                RecordValue::Integer(((index * 7) % 16) as i64),
                RecordValue::Integer(3),
                RecordValue::Integer(prng.next_below(3) as i64),
                RecordValue::Double(prng.next_f64() * 1000.0),
                RecordValue::Integer(0),
            ],
            FixtureCase::ExtremeIntegers => {
                const EXTREMES: [i64; 9] = [
                    i64::MIN,
                    i64::MAX,
                    0,
                    1,
                    -1,
                    i64::MIN + 1,
                    i64::MAX - 1,
                    42,
                    -42,
                ];
                vec![
                    RecordValue::Integer(EXTREMES[index % EXTREMES.len()]),
                    RecordValue::Integer(EXTREMES[(index + 3) % EXTREMES.len()]),
                    RecordValue::Integer(prng.next_i64()),
                ]
            }
            FixtureCase::ScaledInteger => {
                if index == 0 {
                    return vec![
                        RecordValue::ScaledInteger(-100_000),
                        RecordValue::ScaledInteger(-100_000),
                        RecordValue::ScaledInteger(-100_000),
                        RecordValue::ScaledInteger(0),
                    ];
                }
                if index == 1 {
                    return vec![
                        RecordValue::ScaledInteger(100_000),
                        RecordValue::ScaledInteger(100_000),
                        RecordValue::ScaledInteger(100_000),
                        RecordValue::ScaledInteger(2047),
                    ];
                }
                vec![
                    RecordValue::ScaledInteger(-100_000 + prng.next_below(200_001) as i64),
                    RecordValue::ScaledInteger(-100_000 + prng.next_below(200_001) as i64),
                    RecordValue::ScaledInteger(-100_000 + prng.next_below(200_001) as i64),
                    RecordValue::ScaledInteger(prng.next_below(2048) as i64),
                ]
            }
        }
    }
}

/// A generated fixture: the E57 file bytes plus the expected
/// semantic summary derived from the generator inputs.
#[derive(Clone, Debug)]
pub struct Fixture {
    /// The case this fixture was generated for.
    pub case: FixtureCase,
    /// The fixed seed the fixture was generated from.
    pub seed: u64,
    /// The complete E57 file bytes (kept in memory, no temp files).
    pub bytes: Vec<u8>,
    /// The expected semantic summary for round-trip verification.
    pub expected: SemanticSummary,
}

/// Deterministic file GUID for a case.
/// The GUID intentionally does not depend on the seed, so that
/// semantic comparisons of different seeds reach the record level.
fn file_guid(case: FixtureCase) -> String {
    format!("fixture-file-{}", case.name())
}

/// Deterministic point cloud GUID for a case.
fn pointcloud_guid(case: FixtureCase) -> String {
    format!("fixture-pc-{}", case.name())
}

/// Generates a fixture completely in memory.
///
/// The whole generation goes through the public `E57Writer` API,
/// no internal objects are constructed or forged.
pub fn generate(case: FixtureCase, seed: u64) -> Result<Fixture, GateFailure> {
    let mem = MemFile::new();
    let expected = generate_into(case, seed, mem.clone()).map_err(|err| {
        GateFailure::new(
            case.name(),
            seed,
            Stage::Generate,
            String::from("public writer API"),
            format!("failed to generate fixture: {err}"),
        )
    })?;
    Ok(Fixture {
        case,
        seed,
        bytes: mem.to_bytes(),
        expected,
    })
}

/// Generates a fixture into the supplied writer and returns the
/// expected semantic summary derived from the generator inputs.
///
/// This variant accepts any writer, which allows tests to inject
/// recoverable IO failures in the middle of the generation.
/// Errors are returned in the existing `e57` error layering.
pub fn generate_into<W: Read + Write + Seek>(
    case: FixtureCase,
    seed: u64,
    writer: W,
) -> e57::Result<SemanticSummary> {
    let mut prng = SplitMix64::new(seed ^ case.seed_salt());
    let prototype = case.prototype();
    let points = case.points(&mut prng);

    let file_guid = file_guid(case);
    let pc_guid = pointcloud_guid(case);

    let mut e57 = E57Writer::new(writer, &file_guid)?;
    let mut pc_writer = e57.add_pointcloud(&pc_guid, prototype.clone())?;
    for point in &points {
        pc_writer.add_point(point.clone())?;
    }
    pc_writer.finalize()?;
    e57.finalize()?;
    drop(e57);

    Ok(SemanticSummary {
        guid: file_guid,
        pointclouds: vec![PointCloudSummary {
            guid: Some(pc_guid),
            records: points.len() as u64,
            prototype: prototype.iter().map(canonical_record).collect(),
            values_checksum: values_checksum(&points),
            values: points,
        }],
    })
}
