use e57::{RawValues, Record, RecordDataType, RecordName, RecordValue};
use std::fmt::{Display, Formatter};

/// Canonical string describing a record name, independent of internal
/// representation details. Used for semantic tree comparison.
pub fn canonical_name(name: &RecordName) -> String {
    format!("{name:?}")
}

/// Canonical string describing a record data type with all parameters.
pub fn canonical_type(data_type: &RecordDataType) -> String {
    match data_type {
        RecordDataType::Single { min, max } => format!("Single(min={min:?},max={max:?})"),
        RecordDataType::Double { min, max } => format!("Double(min={min:?},max={max:?})"),
        RecordDataType::ScaledInteger {
            min,
            max,
            scale,
            offset,
        } => format!("ScaledInteger(min={min},max={max},scale={scale},offset={offset})"),
        RecordDataType::Integer { min, max } => format!("Integer(min={min},max={max})"),
    }
}

/// Canonical string describing one prototype record (name and type).
pub fn canonical_record(record: &Record) -> String {
    format!(
        "{}:{}",
        canonical_name(&record.name),
        canonical_type(&record.data_type)
    )
}

/// FNV-1a checksum over raw record values.
/// Compact record summary used in reports and order-stability checks.
pub fn values_checksum(values: &[RawValues]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |byte: u8| {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for point in values {
        for value in point {
            let (tag, bytes): (u8, [u8; 8]) = match value {
                RecordValue::Single(v) => (1, {
                    let mut b = [0_u8; 8];
                    b[..4].copy_from_slice(&v.to_le_bytes());
                    b
                }),
                RecordValue::Double(v) => (2, v.to_le_bytes()),
                RecordValue::ScaledInteger(v) => (3, v.to_le_bytes()),
                RecordValue::Integer(v) => (4, v.to_le_bytes()),
            };
            mix(tag);
            for byte in bytes {
                mix(byte);
            }
        }
    }
    hash
}

/// Semantic summary of one point cloud: prototype descriptors in
/// declaration order plus a full record summary of the point data.
#[derive(Clone, Debug, PartialEq)]
pub struct PointCloudSummary {
    /// Point cloud GUID, if present.
    pub guid: Option<String>,
    /// Number of records declared in the XML metadata.
    pub records: u64,
    /// Canonical prototype descriptors in declaration order.
    pub prototype: Vec<String>,
    /// All raw record values in file order.
    pub values: Vec<RawValues>,
    /// FNV-1a checksum of the raw record values.
    pub values_checksum: u64,
}

/// Semantic tree of a whole E57 file, extracted through the public
/// reader API. Comparison of fixtures happens on this semantic level,
/// never on compressed bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticSummary {
    /// File GUID from the XML root.
    pub guid: String,
    /// Point cloud summaries in file order.
    pub pointclouds: Vec<PointCloudSummary>,
}

/// A single semantic violation with the first violating location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// Location of the first violation (for example
    /// "pointcloud 0, record 3, field 'cartesianX'").
    pub location: String,
    /// Details with expected and actual values.
    pub detail: String,
}

impl Display for Violation {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        write!(f, "{}: {}", self.location, self.detail)
    }
}

/// Compares two semantic summaries and returns the first violation.
///
/// The comparison follows the document order of the E57 file:
/// root metadata, point cloud order, prototype declaration order,
/// then record order. The first difference is reported with its
/// exact location, comparison never degrades to a boolean.
pub fn compare_summaries(
    expected: &SemanticSummary,
    actual: &SemanticSummary,
) -> Result<(), Violation> {
    if expected.guid != actual.guid {
        return Err(Violation {
            location: String::from("root guid"),
            detail: format!("expected guid '{}', found '{}'", expected.guid, actual.guid),
        });
    }
    if expected.pointclouds.len() != actual.pointclouds.len() {
        return Err(Violation {
            location: String::from("data3D vector"),
            detail: format!(
                "expected {} pointclouds, found {}",
                expected.pointclouds.len(),
                actual.pointclouds.len()
            ),
        });
    }
    for (index, (exp, act)) in expected
        .pointclouds
        .iter()
        .zip(actual.pointclouds.iter())
        .enumerate()
    {
        compare_pointcloud(index, exp, act)?;
    }
    Ok(())
}

fn compare_pointcloud(
    index: usize,
    expected: &PointCloudSummary,
    actual: &PointCloudSummary,
) -> Result<(), Violation> {
    let pc = format!("pointcloud {index}");
    if expected.guid != actual.guid {
        return Err(Violation {
            location: format!("{pc}, guid"),
            detail: format!("expected {:?}, found {:?}", expected.guid, actual.guid),
        });
    }
    if expected.records != actual.records {
        return Err(Violation {
            location: format!("{pc}, record count"),
            detail: format!(
                "expected {} records, found {}",
                expected.records, actual.records
            ),
        });
    }
    if expected.prototype.len() != actual.prototype.len() {
        return Err(Violation {
            location: format!("{pc}, prototype"),
            detail: format!(
                "expected {} prototype fields, found {}",
                expected.prototype.len(),
                actual.prototype.len()
            ),
        });
    }
    for (field, (exp, act)) in expected
        .prototype
        .iter()
        .zip(actual.prototype.iter())
        .enumerate()
    {
        if exp != act {
            return Err(Violation {
                location: format!("{pc}, prototype field {field}"),
                detail: format!("expected '{exp}', found '{act}'"),
            });
        }
    }
    if expected.values.len() != actual.values.len() {
        return Err(Violation {
            location: format!("{pc}, records"),
            detail: format!(
                "expected {} records, read {}",
                expected.values.len(),
                actual.values.len()
            ),
        });
    }
    for (record, (exp, act)) in expected.values.iter().zip(actual.values.iter()).enumerate() {
        for (field, (exp_value, act_value)) in exp.iter().zip(act.iter()).enumerate() {
            if exp_value != act_value {
                let name = expected
                    .prototype
                    .get(field)
                    .map(String::as_str)
                    .unwrap_or("unknown");
                return Err(Violation {
                    location: format!("{pc}, record {record}, field '{name}'"),
                    detail: format!("expected {exp_value}, found {act_value}"),
                });
            }
        }
    }
    if expected.values_checksum != actual.values_checksum {
        return Err(Violation {
            location: format!("{pc}, record summary"),
            detail: format!(
                "record checksum mismatch: expected {:#018x}, found {:#018x}",
                expected.values_checksum, actual.values_checksum
            ),
        });
    }
    Ok(())
}
