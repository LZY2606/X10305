//! Resource budgets for parsing untrusted E57 input.
//!
//! The limits are shared by the regular parsing path and any constrained
//! parsing entry point: there is no separate test-only parser. When a budget
//! is exceeded the resulting error always contains a snapshot of all three
//! counters (bytes, records and XML element depth) together with the phase
//! that triggered the violation, so failures remain diagnosable instead of
//! looking like generic read errors.

use crate::Error;
use crate::Result;
use std::fmt::{Display, Formatter};

/// Snapshot of how many resources a parser has consumed so far.
///
/// The counters are monotonic for the lifetime of a reader.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BudgetUsage {
    /// Logical bytes (without per-page CRC bytes) read from the E57 container.
    pub bytes_read: u64,
    /// Complete point records handed out to the caller.
    pub records_read: u64,
    /// Maximum nesting depth of parsed XML elements.
    pub xml_element_depth: u64,
}

impl Display for BudgetUsage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "budget usage: {} bytes, {} records, XML depth {}",
            self.bytes_read, self.records_read, self.xml_element_depth
        )
    }
}

/// Resource limits that protect the parser against hostile input.
///
/// All limits default to effectively unlimited values to preserve the
/// behavior of the existing unconstrained entry points. Use one of the
/// constructors to get tighter limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseLimits {
    /// Maximum number of logical container bytes the parser may consume.
    pub max_bytes: u64,
    /// Maximum number of point records the parser may hand out.
    pub max_records: u64,
    /// Maximum nesting depth of XML elements.
    pub max_xml_element_depth: u64,
}

impl Default for ParseLimits {
    fn default() -> Self {
        Self::UNLIMITED
    }
}

impl ParseLimits {
    /// Limits that never trigger, matching the unconstrained entry points.
    pub const UNLIMITED: ParseLimits = ParseLimits {
        max_bytes: u64::MAX,
        max_records: u64::MAX,
        max_xml_element_depth: u64::MAX,
    };

    /// Create custom limits. Use [`ParseLimits::UNLIMITED`] for any dimension
    /// that should stay unconstrained.
    pub const fn new(max_bytes: u64, max_records: u64, max_xml_element_depth: u64) -> Self {
        Self {
            max_bytes,
            max_records,
            max_xml_element_depth,
        }
    }
}

/// Internal budget tracker shared by the paged reader, the point queue and
/// the XML validation step of the production parsing path.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Budget {
    limits: ParseLimits,
    usage: BudgetUsage,
}

impl Budget {
    pub(crate) fn new(limits: ParseLimits) -> Self {
        Self {
            limits,
            usage: BudgetUsage::default(),
        }
    }

    pub(crate) fn usage(&self) -> BudgetUsage {
        self.usage
    }

    /// Account for `count` additional logical bytes read from the container.
    pub(crate) fn charge_bytes(&mut self, count: u64, phase: &str) -> Result<()> {
        let bytes_read = self.usage.bytes_read.saturating_add(count);
        if bytes_read > self.limits.max_bytes {
            Error::read(format!(
                "Resource budget exceeded while {phase}: byte limit {} crossed ({bytes_read} logical bytes read); {}",
                self.limits.max_bytes, self.usage
            ))
        } else {
            self.usage.bytes_read = bytes_read;
            Ok(())
        }
    }

    /// Account for one complete point record that is about to be returned.
    pub(crate) fn charge_record(&mut self, phase: &str) -> Result<()> {
        let records_read = self.usage.records_read.saturating_add(1);
        if records_read > self.limits.max_records {
            Error::read(format!(
                "Resource budget exceeded while {phase}: record limit {} crossed ({records_read} records read); {}",
                self.limits.max_records, self.usage
            ))
        } else {
            self.usage.records_read = records_read;
            Ok(())
        }
    }

    /// Record the observed XML nesting depth and reject it if it is too deep.
    pub(crate) fn check_xml_depth(&mut self, depth: u64, phase: &str) -> Result<()> {
        if depth > self.usage.xml_element_depth {
            self.usage.xml_element_depth = depth;
        }
        if depth > self.limits.max_xml_element_depth {
            Error::read(format!(
                "Resource budget exceeded while {phase}: XML element depth limit {} crossed (depth {depth}); {}",
                self.limits.max_xml_element_depth, self.usage
            ))
        } else {
            Ok(())
        }
    }
}
