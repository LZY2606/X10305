use crate::budget::Budgets;
use crate::corrupt::{run_rejection, Corruption, RejectionReport};
use crate::fixture::{generate, FixtureCase};
use crate::verify::{verify, RoundTripReport};
use crate::GateFailure;
use std::fmt::{Display, Formatter};

/// Report of a complete fixture gate run.
///
/// Contains one round-trip report per fixture case and one
/// rejection report per corruption, all in stable execution order.
#[derive(Clone, Debug, PartialEq)]
pub struct GateReport {
    /// The fixed seed all fixtures were generated from.
    pub seed: u64,
    /// Round-trip reports in case execution order.
    pub roundtrips: Vec<RoundTripReport>,
    /// Corruption rejection reports in corruption declaration order.
    pub rejections: Vec<RejectionReport>,
}

impl Display for GateReport {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        writeln!(f, "fixture gate report (seed {:#018x}):", self.seed)?;
        for report in &self.roundtrips {
            writeln!(
                f,
                "  round-trip '{}': ok ({} bytes, {} records, xml-depth {}, checksums ok)",
                report.case.name(),
                report.file_bytes,
                report.records_checked,
                report.xml_depth
            )?;
        }
        for report in &self.rejections {
            let parse = report.parse_layer.map(|l| l.name()).unwrap_or("accepted");
            writeln!(
                f,
                "  corruption '{}': rejected (parse layer: {})",
                report.corruption.name(),
                parse
            )?;
        }
        Ok(())
    }
}

/// Runs the complete fixture gate: generates every fixture case from
/// the fixed seed, verifies the round-trips and checks that all local
/// corruptions are rejected through the existing error layering.
///
/// The gate is stateless and deterministic: running it repeatedly
/// with the same seed produces identical reports and leaves no
/// temporary files behind.
pub fn run_gate(
    seed: u64,
    cases: &[FixtureCase],
    budgets: &Budgets,
) -> Result<GateReport, GateFailure> {
    let mut roundtrips = Vec::with_capacity(cases.len());
    for case in cases {
        let fixture = generate(*case, seed)?;
        roundtrips.push(verify(&fixture, budgets)?);
    }
    let mut rejections = Vec::with_capacity(Corruption::ALL.len());
    for corruption in Corruption::ALL {
        rejections.push(run_rejection(seed, corruption, budgets)?);
    }
    Ok(GateReport {
        seed,
        roundtrips,
        rejections,
    })
}
