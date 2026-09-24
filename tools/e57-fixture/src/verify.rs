use crate::budget::{BudgetReport, Budgets};
use crate::fixture::Fixture;
use crate::summary::{
    canonical_record, compare_summaries, values_checksum, PointCloudSummary, SemanticSummary,
};
use crate::{FixtureCase, GateFailure, Stage};
use e57::{E57Reader, PointCloud, RawValues, RecordName};
use std::io::Cursor;

/// Report of one successful round-trip verification.
///
/// Contains the caller-visible results (record count, checksums)
/// and the observed resource usage together with the budgets.
#[derive(Clone, Debug, PartialEq)]
pub struct RoundTripReport {
    /// The verified fixture case.
    pub case: FixtureCase,
    /// The fixed seed of the fixture.
    pub seed: u64,
    /// Physical size of the E57 file in bytes.
    pub file_bytes: usize,
    /// Size of the XML section in bytes.
    pub xml_bytes: usize,
    /// Observed XML tree recursion depth.
    pub xml_depth: usize,
    /// Total number of records read and compared.
    pub records_checked: u64,
    /// FNV-1a record summary checksums, one per point cloud, in file order.
    pub record_checksums: Vec<u64>,
    /// Observed resource usage and budgets.
    pub budgets: BudgetReport,
}

/// Verifies a generated fixture against its own expected summary.
pub fn verify(fixture: &Fixture, budgets: &Budgets) -> Result<RoundTripReport, GateFailure> {
    verify_bytes(
        fixture.case,
        fixture.seed,
        &fixture.bytes,
        &fixture.expected,
        budgets,
    )
}

/// Verifies E57 file bytes against an expected semantic summary.
///
/// The verification walks the public reader API of the `e57` crate
/// through all layers: CRC checksums, XML section, binary section
/// header, compressed vector packets and finally the semantic tree
/// and record summary comparison. The first violation is reported
/// with its exact location and the budgets in effect.
pub fn verify_bytes(
    case: FixtureCase,
    seed: u64,
    bytes: &[u8],
    expected: &SemanticSummary,
    budgets: &Budgets,
) -> Result<RoundTripReport, GateFailure> {
    let mut observed = BudgetReport::new(bytes.len(), budgets);
    let fail = |stage: Stage, location: String, detail: String, observed: &BudgetReport| {
        GateFailure::new(case.name(), seed, stage, location, detail).with_budgets(observed.clone())
    };

    // Stage 1: validate the CRC checksums of the paged file layer
    E57Reader::validate_crc(Cursor::new(bytes)).map_err(|err| {
        fail(
            Stage::Checksum,
            String::from("CRC page layer"),
            format!("checksum validation failed: {err}"),
            &observed,
        )
    })?;

    // Stage 2: extract and parse the XML section
    let xml_raw = E57Reader::raw_xml(Cursor::new(bytes)).map_err(|err| {
        fail(
            Stage::XmlSection,
            String::from("XML section"),
            format!("failed to extract XML section: {err}"),
            &observed,
        )
    })?;
    observed.xml_bytes = Some(xml_raw.len());
    if xml_raw.len() > budgets.max_xml_bytes {
        return Err(fail(
            Stage::Budget,
            String::from("XML section"),
            format!(
                "XML section of {} bytes exceeds byte budget of {} bytes",
                xml_raw.len(),
                budgets.max_xml_bytes
            ),
            &observed,
        ));
    }
    let xml = String::from_utf8(xml_raw).map_err(|err| {
        fail(
            Stage::XmlSection,
            String::from("XML section"),
            format!("XML section is not valid UTF8: {err}"),
            &observed,
        )
    })?;
    let depth = xml_tree_depth(&xml).map_err(|err| {
        fail(
            Stage::XmlSection,
            String::from("XML section"),
            format!("failed to parse XML section: {err}"),
            &observed,
        )
    })?;
    observed.xml_depth = Some(depth);
    if depth > budgets.max_xml_depth {
        return Err(fail(
            Stage::Budget,
            String::from("XML section"),
            format!(
                "XML recursion depth {depth} exceeds recursion budget of {}",
                budgets.max_xml_depth
            ),
            &observed,
        ));
    }

    // Stage 3: open the file and check binary section invariants
    let mut reader = E57Reader::new(Cursor::new(bytes)).map_err(|err| {
        fail(
            Stage::BinarySection,
            String::from("file header"),
            format!("failed to open E57 file: {err}"),
            &observed,
        )
    })?;
    let header = reader.header();
    if header.phys_length != bytes.len() as u64 {
        return Err(fail(
            Stage::BinarySection,
            String::from("file header"),
            format!(
                "header physical length {} does not match actual file size {}",
                header.phys_length,
                bytes.len()
            ),
            &observed,
        ));
    }
    if header.phys_xml_offset >= header.phys_length {
        return Err(fail(
            Stage::BinarySection,
            String::from("file header"),
            format!(
                "XML offset {} points outside file of {} bytes",
                header.phys_xml_offset, header.phys_length
            ),
            &observed,
        ));
    }
    let pointclouds = reader.pointclouds();
    for (index, pc) in pointclouds.iter().enumerate() {
        let section_id = bytes.get(pc.file_offset as usize).copied().unwrap_or(0);
        if section_id != 1 {
            return Err(fail(
                Stage::BinarySection,
                format!("pointcloud {index}, binary section"),
                format!(
                    "compressed vector section at offset {} starts with id {section_id}, expected 1",
                    pc.file_offset
                ),
                &observed,
            ));
        }
    }

    // Stage 4: decode the compressed vector packets
    let mut all_values: Vec<Vec<RawValues>> = Vec::with_capacity(pointclouds.len());
    for (index, pc) in pointclouds.iter().enumerate() {
        if pc.records > budgets.max_records {
            observed.records_read = Some(0);
            return Err(fail(
                Stage::Budget,
                format!("pointcloud {index}, compressed vector"),
                format!(
                    "declared record count {} exceeds record budget of {}",
                    pc.records, budgets.max_records
                ),
                &observed,
            ));
        }
        let iter = reader.pointcloud_raw(pc).map_err(|err| {
            fail(
                Stage::CompressedVector,
                format!("pointcloud {index}, compressed vector"),
                format!("failed to open compressed vector: {err}"),
                &observed,
            )
        })?;
        let mut values = Vec::with_capacity(pc.records as usize);
        for (record, point) in iter.enumerate() {
            let point = point.map_err(|err| {
                observed.records_read = Some(record as u64);
                fail(
                    Stage::CompressedVector,
                    format!("pointcloud {index}, record {record}"),
                    format!("failed to decode record: {err}"),
                    &observed,
                )
            })?;
            values.push(point);
        }
        if values.len() as u64 != pc.records {
            return Err(fail(
                Stage::CompressedVector,
                format!("pointcloud {index}, compressed vector"),
                format!(
                    "declared {} records but decoded {}",
                    pc.records,
                    values.len()
                ),
                &observed,
            ));
        }
        all_values.push(values);
    }
    let records_checked: u64 = all_values.iter().map(|v| v.len() as u64).sum();
    observed.records_read = Some(records_checked);

    // Stage 5: check internal invariants that prove E57 semantics
    for (index, pc) in pointclouds.iter().enumerate() {
        check_bounds_containment(index, pc, &all_values[index]).map_err(|detail| {
            fail(
                Stage::CompressedVector,
                format!("pointcloud {index}, cartesian bounds"),
                detail,
                &observed,
            )
        })?;
    }

    // Stage 6: compare the semantic tree and the record summary
    let actual = semantic_summary(&reader, &pointclouds, &all_values);
    if let Err(violation) = compare_summaries(expected, &actual) {
        let stage = if violation.location.contains("record") {
            Stage::RecordSummary
        } else {
            Stage::SemanticTree
        };
        return Err(fail(stage, violation.location, violation.detail, &observed));
    }

    Ok(RoundTripReport {
        case,
        seed,
        file_bytes: bytes.len(),
        xml_bytes: xml.len(),
        xml_depth: depth,
        records_checked,
        record_checksums: all_values.iter().map(|v| values_checksum(v)).collect(),
        budgets: observed,
    })
}

/// Builds the semantic summary from the parsed file using only
/// the public reader API surface.
fn semantic_summary(
    reader: &E57Reader<Cursor<&[u8]>>,
    pointclouds: &[PointCloud],
    all_values: &[Vec<RawValues>],
) -> SemanticSummary {
    SemanticSummary {
        guid: reader.guid().to_owned(),
        pointclouds: pointclouds
            .iter()
            .zip(all_values.iter())
            .map(|(pc, values)| PointCloudSummary {
                guid: pc.guid.clone(),
                records: pc.records,
                prototype: pc.prototype.iter().map(canonical_record).collect(),
                values_checksum: values_checksum(values),
                values: values.clone(),
            })
            .collect(),
    }
}

/// Computes the maximum element depth of the XML tree with the same
/// XML parser the library itself uses for the E57 XML section.
fn xml_tree_depth(xml: &str) -> Result<usize, roxmltree::Error> {
    let document = roxmltree::Document::parse(xml)?;
    Ok(document
        .descendants()
        .filter(|n| n.is_element())
        .map(|n| n.ancestors().count())
        .max()
        .unwrap_or(0))
}

/// Internal invariant: the Cartesian bounds stored in the XML metadata
/// must contain all valid Cartesian coordinates of the point data.
fn check_bounds_containment(
    index: usize,
    pc: &PointCloud,
    values: &[RawValues],
) -> Result<(), String> {
    let Some(bounds) = &pc.cartesian_bounds else {
        return Ok(());
    };
    let find = |name: RecordName| pc.prototype.iter().position(|r| r.name == name);
    let (Some(xi), Some(yi), Some(zi)) = (
        find(RecordName::CartesianX),
        find(RecordName::CartesianY),
        find(RecordName::CartesianZ),
    ) else {
        return Ok(());
    };
    let invalid = find(RecordName::CartesianInvalidState);
    for (record, point) in values.iter().enumerate() {
        if let Some(invalid_index) = invalid {
            let state = point[invalid_index]
                .to_i64(&pc.prototype[invalid_index].data_type)
                .map_err(|err| format!("record {record}: cannot read invalid state: {err}"))?;
            if state != 0 {
                continue;
            }
        }
        let axes = [
            ("x", xi, bounds.x_min, bounds.x_max),
            ("y", yi, bounds.y_min, bounds.y_max),
            ("z", zi, bounds.z_min, bounds.z_max),
        ];
        for (axis, axis_index, min, max) in axes {
            let value = point[axis_index]
                .to_f64(&pc.prototype[axis_index].data_type)
                .map_err(|err| format!("record {record}: cannot read {axis} value: {err}"))?;
            let outside = min.is_some_and(|min| value < min) || max.is_some_and(|max| value > max);
            if outside {
                return Err(format!(
                    "pointcloud {index}, record {record}: {axis} value {value} is outside declared bounds [{min:?}, {max:?}]"
                ));
            }
        }
    }
    Ok(())
}
