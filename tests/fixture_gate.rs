//! Regression gate for "e57: round-trip, local corruption and resource
//! budget fixtures".
//!
//! All fixtures are generated from a fixed seed in memory and never
//! touch external point cloud data. Verification compares semantic
//! trees and record summaries, not compressed bytes. Every check goes
//! through the public API of the e57 crate.

use e57::{E57Reader, RawValues, RecordValue};
use e57_fixture::{
    generate, generate_into, run_gate, run_rejection, verify, verify_bytes, Budgets, Corruption,
    ErrorLayer, FixtureCase, FlakyWriter, GateReport, MemFile, RoundTripReport, Stage,
    DEFAULT_SEED,
};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use std::path::Path;
use std::sync::{Arc, Barrier};

const SEED: u64 = DEFAULT_SEED;

fn budgets() -> Budgets {
    Budgets::default()
}

// ---------------------------------------------------------------------
// Fixed inputs: round-trip coverage of the fixture matrix
// ---------------------------------------------------------------------

#[test]
fn fixture_gate_zero_records_roundtrip() {
    let fixture = generate(FixtureCase::ZeroRecords, SEED).unwrap();
    let report = verify(&fixture, &budgets()).unwrap();
    assert_eq!(report.records_checked, 0);
    assert_eq!(report.budgets.records_read, Some(0));

    // Caller-visible result through the public API: one empty point cloud
    let mut reader = E57Reader::new(Cursor::new(fixture.bytes.as_slice())).unwrap();
    let pcs = reader.pointclouds();
    assert_eq!(pcs.len(), 1);
    assert_eq!(pcs[0].records, 0);
    assert_eq!(pcs[0].prototype.len(), 3);
    let mut iter = reader.pointcloud_raw(&pcs[0]).unwrap();
    assert!(iter.next().is_none());
}

#[test]
fn fixture_gate_multi_prototype_roundtrip() {
    let fixture = generate(FixtureCase::MultiPrototype, SEED).unwrap();
    // The fixture uses 19 different prototype fields
    assert_eq!(fixture.expected.pointclouds[0].prototype.len(), 19);
    let report = verify(&fixture, &budgets()).unwrap();
    assert_eq!(report.records_checked, 37);
    assert_eq!(report.record_checksums.len(), 1);
    assert_eq!(
        report.record_checksums[0],
        fixture.expected.pointclouds[0].values_checksum
    );
}

#[test]
fn fixture_gate_extreme_integers_roundtrip() {
    let fixture = generate(FixtureCase::ExtremeIntegers, SEED).unwrap();
    let report = verify(&fixture, &budgets()).unwrap();
    assert_eq!(report.records_checked, 9);

    // Caller-visible result: full-range i64 values survive the round-trip
    let mut reader = E57Reader::new(Cursor::new(fixture.bytes.as_slice())).unwrap();
    let pcs = reader.pointclouds();
    let values: Vec<RawValues> = reader
        .pointcloud_raw(&pcs[0])
        .unwrap()
        .collect::<e57::Result<_>>()
        .unwrap();
    let xs: Vec<i64> = values
        .iter()
        .map(|point| match &point[0] {
            RecordValue::Integer(value) => *value,
            other => panic!("expected integer record, got {other}"),
        })
        .collect();
    assert!(xs.contains(&i64::MIN));
    assert!(xs.contains(&i64::MAX));
    assert_eq!(values, fixture.expected.pointclouds[0].values);
}

#[test]
fn fixture_gate_scaled_integer_roundtrip() {
    let fixture = generate(FixtureCase::ScaledInteger, SEED).unwrap();
    let report = verify(&fixture, &budgets()).unwrap();
    assert_eq!(report.records_checked, 41);

    // Caller-visible result: scaled integer raw values survive the round-trip
    let mut reader = E57Reader::new(Cursor::new(fixture.bytes.as_slice())).unwrap();
    let pcs = reader.pointclouds();
    let values: Vec<RawValues> = reader
        .pointcloud_raw(&pcs[0])
        .unwrap()
        .collect::<e57::Result<_>>()
        .unwrap();
    assert_eq!(values[0][0], RecordValue::ScaledInteger(-100_000));
    assert_eq!(values[1][0], RecordValue::ScaledInteger(100_000));
    assert_eq!(values[1][3], RecordValue::ScaledInteger(2047));
    assert_eq!(values, fixture.expected.pointclouds[0].values);
}

// ---------------------------------------------------------------------
// Fixed inputs: local corruption handling with existing error layering
// ---------------------------------------------------------------------

#[test]
fn fixture_gate_corrupt_checksum_rejected() {
    let report = run_rejection(SEED, Corruption::ChecksumFlip, &budgets()).unwrap();
    assert!(report.rejected());
    assert_eq!(report.crc_layer, Some(ErrorLayer::Read));
    assert_eq!(report.parse_layer, Some(ErrorLayer::Read));
    assert!(
        report.crc_message.as_deref().unwrap().contains("checksum"),
        "unexpected CRC rejection message: {report:?}"
    );
}

#[test]
fn fixture_gate_truncated_packet_rejected() {
    let report = run_rejection(SEED, Corruption::TruncatedPacket, &budgets()).unwrap();
    assert!(report.rejected());
    assert_eq!(report.crc_layer, Some(ErrorLayer::Read));
    assert_eq!(report.parse_layer, Some(ErrorLayer::Read));
    assert!(
        report
            .parse_message
            .as_deref()
            .unwrap()
            .contains("truncated"),
        "unexpected truncation rejection message: {report:?}"
    );
}

#[test]
fn fixture_gate_xml_offset_out_of_bounds_rejected() {
    let report = run_rejection(SEED, Corruption::XmlOffsetOutOfBounds, &budgets()).unwrap();
    assert!(report.rejected());
    assert_eq!(report.parse_layer, Some(ErrorLayer::Read));
    assert!(
        report
            .parse_message
            .as_deref()
            .unwrap()
            .contains("XML section"),
        "unexpected XML offset rejection message: {report:?}"
    );
}

#[test]
fn fixture_gate_xml_length_beyond_budget_unsupported() {
    let report = run_rejection(SEED, Corruption::XmlLengthBeyondBudget, &budgets()).unwrap();
    assert!(report.rejected());
    // Undecidable input must be reported as unsupported, never guessed as success
    assert_eq!(report.parse_layer, Some(ErrorLayer::NotImplemented));
    let message = report.parse_message.as_deref().unwrap();
    assert!(
        message.contains(&e57::MAX_XML_SIZE.to_string()),
        "parser byte budget must be visible in the rejection: {message}"
    );
}

#[test]
fn fixture_gate_rejections_are_reproducible() {
    for corruption in Corruption::ALL {
        let first = run_rejection(SEED, corruption, &budgets()).unwrap();
        let second = run_rejection(SEED, corruption, &budgets()).unwrap();
        assert_eq!(
            first,
            second,
            "rejection of '{}' must be reproducible",
            corruption.name()
        );
    }
}

#[test]
fn fixture_gate_leaves_no_temp_files() {
    let temp_before = snapshot_dir(&std::env::temp_dir());
    let cwd_before = snapshot_dir(Path::new("."));

    // Repeated runs must produce identical reports and no leftover files
    let first = run_gate(SEED, &FixtureCase::ALL, &budgets()).unwrap();
    let second = run_gate(SEED, &FixtureCase::ALL, &budgets()).unwrap();
    assert_eq!(first, second);

    let temp_after = snapshot_dir(&std::env::temp_dir());
    let cwd_after = snapshot_dir(Path::new("."));
    assert_eq!(
        temp_before, temp_after,
        "fixture gate left temporary files behind"
    );
    let new_in_cwd: Vec<_> = cwd_after.difference(&cwd_before).collect();
    assert!(
        new_in_cwd.is_empty(),
        "fixture gate left files in the working directory: {new_in_cwd:?}"
    );
}

fn snapshot_dir(dir: &Path) -> BTreeSet<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------
// Order and scheduling stability (result-irrelevant conditions only)
// ---------------------------------------------------------------------

#[test]
fn fixture_gate_case_order_is_stable() {
    let forward = run_gate(SEED, &FixtureCase::ALL, &budgets()).unwrap();

    let mut reversed = FixtureCase::ALL;
    reversed.reverse();
    let reversed_report = run_gate(SEED, &reversed, &budgets()).unwrap();

    let mut rotated = FixtureCase::ALL;
    rotated.rotate_left(1);
    let rotated_report = run_gate(SEED, &rotated, &budgets()).unwrap();

    // Per-case results do not depend on the execution order
    assert_eq!(by_case(&forward), by_case(&reversed_report));
    assert_eq!(by_case(&forward), by_case(&rotated_report));
    // Corruption rejections do not depend on the case order either
    assert_eq!(forward.rejections, reversed_report.rejections);
    assert_eq!(forward.rejections, rotated_report.rejections);
}

fn by_case(report: &GateReport) -> BTreeMap<&'static str, RoundTripReport> {
    report
        .roundtrips
        .iter()
        .map(|r| (r.case.name(), r.clone()))
        .collect()
}

#[test]
fn fixture_gate_concurrent_runs_with_barrier_are_stable() {
    const THREADS: usize = 4;
    let reference = run_gate(SEED, &FixtureCase::ALL, &budgets()).unwrap();
    let barrier = Arc::new(Barrier::new(THREADS));
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                // Synchronize the start to maximize interleaving
                barrier.wait();
                run_gate(SEED, &FixtureCase::ALL, &Budgets::default())
            })
        })
        .collect();
    for handle in handles {
        let report = handle.join().unwrap().unwrap();
        assert_eq!(
            report, reference,
            "concurrent gate run produced a different result"
        );
    }
}

// ---------------------------------------------------------------------
// Acceptance story: recoverable failure, retry and state cleanliness
// ---------------------------------------------------------------------

#[test]
fn fixture_gate_recoverable_write_failure_retry() {
    let case = FixtureCase::MultiPrototype;

    // Inject one recoverable IO failure in the middle of the generation
    let failed_mem = MemFile::new();
    let flaky = FlakyWriter::new(failed_mem.clone(), 0);
    let err = generate_into(case, SEED, flaky).unwrap_err();
    assert!(
        matches!(err, e57::Error::Write { .. }),
        "injected failure must surface in the existing Write error layer, got: {err}"
    );

    // The failed attempt left only partial, different bytes behind
    let reference = generate(case, SEED).unwrap();
    assert_ne!(failed_mem.to_bytes(), reference.bytes);

    // Retry with identical inputs after recovery: stable result
    let retry_mem = MemFile::new();
    let retry_summary = generate_into(case, SEED, retry_mem.clone()).unwrap();
    assert_eq!(retry_mem.to_bytes(), reference.bytes);
    assert_eq!(retry_summary, reference.expected);
    verify(&reference, &budgets()).unwrap();
}

#[test]
fn fixture_gate_success_carries_no_failure_state() {
    let case = FixtureCase::ExtremeIntegers;
    let pristine = generate(case, SEED).unwrap();
    let gate_before = run_gate(SEED, &FixtureCase::ALL, &budgets()).unwrap();

    // Failures of different kinds in between
    let flaky = FlakyWriter::new(MemFile::new(), 0);
    assert!(generate_into(case, SEED, flaky).is_err());
    for corruption in Corruption::ALL {
        assert!(run_rejection(SEED, corruption, &budgets()).is_ok());
    }

    // Successful results after the failures carry no caches, counters
    // or temporary state from the failed calls
    let after = generate(case, SEED).unwrap();
    assert_eq!(pristine.bytes, after.bytes);
    assert_eq!(pristine.expected, after.expected);
    let gate_after = run_gate(SEED, &FixtureCase::ALL, &budgets()).unwrap();
    assert_eq!(gate_before, gate_after);
}

#[test]
fn fixture_gate_failure_report_shows_first_violation_and_budgets() {
    // Same case, different seed: first violation must be at record level
    let fixture_a = generate(FixtureCase::MultiPrototype, SEED).unwrap();
    let fixture_b = generate(FixtureCase::MultiPrototype, SEED + 1).unwrap();
    let err = verify_bytes(
        FixtureCase::MultiPrototype,
        SEED + 1,
        &fixture_a.bytes,
        &fixture_b.expected,
        &budgets(),
    )
    .unwrap_err();
    assert_eq!(err.stage, Stage::RecordSummary);
    let message = err.to_string();
    assert!(
        message.contains("multi-prototype"),
        "missing case: {message}"
    );
    assert!(message.contains("seed"), "missing seed: {message}");
    assert!(
        message.contains("record") && message.contains("field '"),
        "missing first violation location: {message}"
    );
    assert!(message.contains("budgets:"), "missing budgets: {message}");
    assert!(
        message.contains("xml-depth="),
        "missing recursion budget: {message}"
    );
    assert!(
        message.contains("records="),
        "missing record budget: {message}"
    );

    // Different case: first violation is at the semantic tree level
    let fixture_c = generate(FixtureCase::ScaledInteger, SEED).unwrap();
    let err = verify_bytes(
        FixtureCase::ScaledInteger,
        SEED,
        &fixture_a.bytes,
        &fixture_c.expected,
        &budgets(),
    )
    .unwrap_err();
    assert_eq!(err.stage, Stage::SemanticTree);
    assert!(err.location.contains("guid"), "unexpected location: {err}");
}

#[test]
fn fixture_gate_seed_is_deterministic() {
    let first = generate(FixtureCase::MultiPrototype, SEED).unwrap();
    let second = generate(FixtureCase::MultiPrototype, SEED).unwrap();
    assert_eq!(first.bytes, second.bytes);
    assert_eq!(first.expected, second.expected);

    // A different seed produces different point data, and the semantic
    // comparison (not the compressed bytes) detects it
    let other = generate(FixtureCase::MultiPrototype, SEED + 1).unwrap();
    assert_ne!(first.bytes, other.bytes);
    assert!(verify(&first, &budgets()).is_ok());
    assert!(verify(&other, &budgets()).is_ok());
}

#[test]
fn fixture_gate_record_and_recursion_budgets_are_enforced() {
    let fixture = generate(FixtureCase::MultiPrototype, SEED).unwrap();

    // Record budget: 37 records exceed a budget of 10
    let tight_records = Budgets {
        max_records: 10,
        ..budgets()
    };
    let err = verify(&fixture, &tight_records).unwrap_err();
    assert_eq!(err.stage, Stage::Budget);
    let message = err.to_string();
    assert!(
        message.contains("37"),
        "missing observed records: {message}"
    );
    assert!(message.contains("10"), "missing record budget: {message}");

    // Recursion budget: the E57 XML tree is deeper than 2 levels
    let tight_depth = Budgets {
        max_xml_depth: 2,
        ..budgets()
    };
    let err = verify(&fixture, &tight_depth).unwrap_err();
    assert_eq!(err.stage, Stage::Budget);
    assert!(
        err.to_string().contains("recursion"),
        "missing recursion context: {err}"
    );
}
