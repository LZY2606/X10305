//! Test-only support library for the "e57: roundtrip, local corruption
//! and resource budget fixture gate" regression materials.
//!
//! Everything in this crate is built on the public API of the `e57`
//! library (`E57Writer`, `E57Reader`, `PointCloudWriter`, the raw point
//! iterator and the public error type). No internal parser or writer
//! object is constructed directly: the gate drives the exact same code
//! paths as production callers do, so the fixtures cannot drift away
//! from the real parser.
//!
//! The gate is intentionally stateless: every invocation creates fresh
//! budgets, fresh counters and in-memory buffers. A failed run therefore
//! can never leave caches, counters or temporary state behind that a
//! later successful run would observe.

#![forbid(unsafe_code)]

pub mod budget;
pub mod cases;
pub mod corrupt;
pub mod generate;
pub mod report;
pub mod semantic;
pub mod xml_scan;

use budget::Budget;
use budget::Layer;
use budget::Metered;
use budget::Violation;
use cases::FixtureCase;
use e57::E57Reader;
use e57::RawValues;
use report::GateFailure;
use report::GateReport;
use report::Outcome;
use report::StageReport;
use semantic::SemanticTree;
use std::io::Cursor;

/// Fixed default seed used by all regression fixtures.
///
/// The fixtures are fully deterministic: they never read an external
/// point cloud or any on-disk sample file.
pub const FIXED_SEED: u64 = 0xE57_FIXED;

/// Scheduling variants that only reorder result-independent work.
///
/// Both variants execute the same public calls on the same bytes and
/// must produce the same semantic tree and record summary. They exist
/// to prove that the result does not depend on scheduling order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Schedule {
    /// Metadata is summarized before the records are iterated.
    MetaFirst,
    /// Records are iterated before the metadata is summarized.
    RecordsFirst,
}

impl Schedule {
    pub const ALL: [Schedule; 2] = [Schedule::MetaFirst, Schedule::RecordsFirst];

    pub fn name(self) -> &'static str {
        match self {
            Schedule::MetaFirst => "meta_first",
            Schedule::RecordsFirst => "records_first",
        }
    }
}

/// Configuration for a single gate run. Every run is independent.
#[derive(Clone, Debug)]
pub struct GateConfig {
    pub case: FixtureCase,
    pub seed: u64,
    pub schedule: Schedule,
    pub budget: Budget,
}

impl GateConfig {
    pub fn new(case: FixtureCase) -> Self {
        Self {
            case,
            seed: FIXED_SEED,
            schedule: Schedule::MetaFirst,
            budget: Budget::unlimited(),
        }
    }

    pub fn with_budget(mut self, budget: Budget) -> Self {
        self.budget = budget;
        self
    }

    pub fn with_schedule(mut self, schedule: Schedule) -> Self {
        self.schedule = schedule;
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }
}

/// Records collected from one point cloud during the record stage.
struct PcRecords {
    guid: String,
    declared: u64,
    points: Vec<RawValues>,
}

/// Executes one gate run completely in memory.
///
/// The returned report is the only state of the run: it records the
/// observed outcome, the first violating stage with context, the
/// resource usage of each budget and, for accepted fixtures, the
/// semantic tree and record summary of the roundtripped content.
pub fn run(config: &GateConfig) -> Result<GateReport, GateFailure> {
    // Stage 1: generate the fixture from the fixed seed via the public
    // writer, then apply the local byte level mutation (if any).
    let generated = generate::build_fixture(config.case, config.seed)
        .map_err(|err| GateFailure::preparation(&config.case, err))?;
    let original_len = generated.len() as u64;
    let input = corrupt::apply(config.case, generated);

    let mut stages: Vec<StageReport> = Vec::new();
    let mut usages = Vec::new();

    // Stage 2 (optional): CRC validation through the standalone public
    // entry. Pointer/truncation fixtures probe opening directly because
    // at the CRC layer they are indistinguishable from local damage.
    if config.case.probe_crc_first() {
        let mut budget = config.budget;
        let result = E57Reader::validate_crc(Metered::new(
            Cursor::new(input.clone()),
            &mut budget,
            "crc_validation",
        ));
        budget.record_byte_stages(1);
        stages.push(StageReport::crc(&budget, input.len() as u64));
        usages.push(budget.usage_snapshot());
        if let Err(err) = result {
            if let Some(violation) = budget.take_violation() {
                return Err(budget_failure(
                    config,
                    "crc_validation",
                    violation,
                    budget,
                    input.len() as u64,
                    stages,
                ));
            }
            return rejected(
                config,
                "crc_validation",
                Layer::classify(&err),
                err,
                budget,
                input.len() as u64,
                original_len,
                stages,
                usages,
            );
        }
    }

    // Stage 3: open the file via the public generic reader entry while
    // metering the logical bytes consumed by the parser.
    let mut open_budget = config.budget;
    let mut reader = match E57Reader::new(Metered::new(
        Cursor::new(input.clone()),
        &mut open_budget,
        "open",
    )) {
        Ok(reader) => reader,
        Err(err) => {
            if let Some(violation) = open_budget.take_violation() {
                return Err(budget_failure(
                    config,
                    "open",
                    violation,
                    open_budget,
                    input.len() as u64,
                    stages,
                ));
            }
            return rejected(
                config,
                "open",
                Layer::classify(&err),
                err,
                open_budget,
                input.len() as u64,
                original_len,
                stages,
                usages,
            );
        }
    };

    // Stage 4: the XML recursion budget is checked on the exact XML
    // string exposed by the public reader.
    let xml = reader.xml().to_owned();
    let mut xml_budget = config.budget;
    let depth = match xml_scan::max_element_depth(&xml, &mut xml_budget) {
        Ok(depth) => depth,
        Err(xml_scan::ScanError::Budget(violation)) => {
            stages.push(StageReport::xml_depth(&open_budget, &xml_budget));
            return Err(budget_failure(
                config,
                "xml_recursion",
                violation,
                xml_budget,
                input.len() as u64,
                stages,
            ));
        }
        Err(xml_scan::ScanError::Malformed(detail)) => {
            return Err(GateFailure::preparation(&config.case, detail));
        }
    };

    // Stage 5: iterate every point cloud through the public raw
    // iterator. Both scheduling variants read exactly the same records;
    // only the point at which metadata is summarized moves.
    let pointclouds = reader.pointclouds();
    let mut collected: Vec<PcRecords> = Vec::new();
    for pc in &pointclouds {
        let mut record_budget = config.budget;
        let iter = match reader.pointcloud_raw(pc) {
            Ok(iter) => iter,
            Err(err) => {
                return rejected(
                    config,
                    "record_iteration",
                    Layer::classify(&err),
                    err,
                    record_budget,
                    input.len() as u64,
                    original_len,
                    stages,
                    usages,
                );
            }
        };

        let mut points: Vec<RawValues> = Vec::new();
        for item in iter {
            if record_budget.records_used >= record_budget.records_limit {
                let violation = Violation::records(
                    "record_iteration",
                    record_budget.records_used,
                    record_budget.records_limit,
                );
                return Err(budget_failure(
                    config,
                    "record_iteration",
                    violation,
                    record_budget,
                    input.len() as u64,
                    stages,
                ));
            }
            match item {
                Ok(point) => {
                    record_budget.records_used += 1;
                    points.push(point);
                }
                Err(err) => {
                    return rejected(
                        config,
                        "record_iteration",
                        Layer::classify(&err),
                        err,
                        record_budget,
                        input.len() as u64,
                        original_len,
                        stages,
                        usages,
                    );
                }
            }
        }
        record_budget.record_byte_stages(1);
        collected.push(PcRecords {
            guid: pc.guid.clone().unwrap_or_default(),
            declared: pc.records,
            points,
        });
        stages.push(StageReport::records(&record_budget, pc.records));
        usages.push(record_budget.usage_snapshot());
    }

    open_budget.record_byte_stages(1);
    stages.push(StageReport::open(&open_budget, &xml_budget));
    stages.push(StageReport::xml_depth(&open_budget, &xml_budget));
    usages.push(open_budget.usage_snapshot());
    usages.push(xml_budget.usage_snapshot());

    if !config.case.is_accepted() {
        return Err(GateFailure::unexpected_acceptance(&config.case, &stages));
    }

    // Stage 6: summarize metadata either before or after the record
    // summary depending on the selected schedule. The semantic tree is
    // compared, never the compressed bytes.
    let semantic = match config.schedule {
        Schedule::MetaFirst => {
            SemanticTree::build(&reader, &pointclouds, &collected, depth)
        }
        Schedule::RecordsFirst => {
            SemanticTree::build_from_records(&pointclouds, &collected, depth, &reader)
        }
    };

    let expected_count: u64 = pointclouds.iter().map(|p| p.records).sum();
    let observed_count: u64 = collected.iter().map(|c| c.points.len() as u64).sum();
    for pc in &collected {
        if pc.points.len() as u64 != pc.declared {
            return Err(GateFailure::inconsistent(
                &config.case,
                format!(
                    "Point cloud '{}' delivered {} records but declares {}",
                    pc.guid,
                    pc.points.len(),
                    pc.declared
                ),
            ));
        }
    }
    if observed_count != expected_count {
        return Err(GateFailure::inconsistent(
            &config.case,
            format!("Iterator ended after {observed_count} records but XML declares {expected_count}"),
        ));
    }

    let digest = semantic.digest();

    Ok(GateReport {
        case: config.case,
        seed: config.seed,
        schedule: config.schedule,
        outcome: Outcome::Accepted,
        input_bytes: input.len() as u64,
        original_bytes: original_len,
        stages,
        usage: usages,
        semantic: Some(semantic),
        digest: Some(digest),
        expected_layer: None,
        observed_layer: None,
        violation_stage: None,
        violation_detail: None,
    })
}

#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
fn rejected(
    config: &GateConfig,
    stage: &str,
    observed: Layer,
    err: e57::Error,
    budget: Budget,
    input_len: u64,
    original_len: u64,
    stages: Vec<StageReport>,
    mut usages: Vec<budget::UsageSnapshot>,
) -> Result<GateReport, GateFailure> {
    if config.case.is_accepted() {
        return Err(GateFailure::unexpected_rejection(
            &config.case,
            stage,
            observed,
            err,
            budget,
        ));
    }
    let expected = config.case.expected_layer();
    let detail = format!("{err}");
    usages.push(budget.usage_snapshot());
    Ok(GateReport {
        case: config.case,
        seed: config.seed,
        schedule: config.schedule,
        outcome: if expected == observed {
            Outcome::Rejected
        } else {
            Outcome::WrongLayer
        },
        input_bytes: input_len,
        original_bytes: original_len,
        stages,
        usage: usages,
        semantic: None,
        digest: None,
        expected_layer: Some(expected),
        observed_layer: Some(observed),
        violation_stage: Some(stage.to_owned()),
        violation_detail: Some(detail),
    })
}

#[allow(clippy::needless_pass_by_value)]
fn budget_failure(
    config: &GateConfig,
    stage: &str,
    violation: Violation,
    budget: Budget,
    input_len: u64,
    stages: Vec<StageReport>,
) -> Result<GateReport, GateFailure> {
    Ok(GateReport {
        case: config.case,
        seed: config.seed,
        schedule: config.schedule,
        outcome: Outcome::BudgetViolation,
        input_bytes: input_len,
        original_bytes: input_len,
        stages,
        usage: vec![budget.usage_snapshot()],
        semantic: None,
        digest: None,
        expected_layer: None,
        observed_layer: Some(Layer::ResourceBudget),
        violation_stage: Some(stage.to_owned()),
        violation_detail: Some(violation.describe()),
    })
}
