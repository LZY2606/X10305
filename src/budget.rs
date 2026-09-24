//! Resource budgets for parsing untrusted E57 input.
//!
//! The limits are enforced on the regular production parsing path:
//! [`crate::PagedReader`] counts logical payload bytes handed to the parser,
//! the point cloud iterators count decoded records and the XML parser
//! validates the nesting depth against [`ParseLimits::max_xml_depth`].
//!
//! A budget violation is reported as [`crate::Error::Read`] and the
//! Display text of the violation always contains all three current
//! usage values together with the configured limits. The same snapshot
//! can be requested at any time via [`crate::E57Reader::budget_usage()`].

use std::cell::Cell;
use std::fmt::{Display, Formatter};

/// Default maximum nesting depth for XML elements.
const DEFAULT_MAX_XML_DEPTH: u32 = 1_000;

/// Resource limits used while parsing an E57 file.
///
/// All limits default to values that are effectively unlimited for normal
/// files, so existing callers observe no behavior change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ParseLimits {
    /// Maximum number of logical (without page checksum) bytes that the
    /// parser is allowed to consume from the file.
    pub max_bytes: u64,
    /// Maximum number of point records that point cloud iterators may deliver.
    pub max_records: u64,
    /// Maximum nesting depth of elements in the XML section.
    pub max_xml_depth: u32,
}

impl Default for ParseLimits {
    fn default() -> Self {
        Self {
            max_bytes: u64::MAX,
            max_records: u64::MAX,
            max_xml_depth: DEFAULT_MAX_XML_DEPTH,
        }
    }
}

impl ParseLimits {
    /// Creates limits using the built-in, practically unlimited defaults.
    pub fn unlimited() -> Self {
        Self::default()
    }
}

/// Snapshot of the resource usage counters at one point in time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BudgetUsage {
    /// Logical payload bytes already consumed from the paged reader.
    pub bytes: u64,
    /// Point records already delivered by point cloud iterators.
    pub records: u64,
    /// Deepest XML element nesting depth observed so far.
    pub xml_depth: u32,
}

/// Shared, cheaply clonable budget used by all readers of one E57 file.
#[derive(Clone)]
pub(crate) struct Budget {
    limits: ParseLimits,
    bytes: std::rc::Rc<Cell<u64>>,
    records: std::rc::Rc<Cell<u64>>,
    xml_depth: std::rc::Rc<Cell<u32>>,
}

impl Budget {
    pub fn new(limits: ParseLimits) -> Self {
        Self {
            limits,
            bytes: std::rc::Rc::new(Cell::new(0)),
            records: std::rc::Rc::new(Cell::new(0)),
            xml_depth: std::rc::Rc::new(Cell::new(0)),
        }
    }

    pub fn limits(&self) -> ParseLimits {
        self.limits
    }

    pub fn usage(&self) -> BudgetUsage {
        BudgetUsage {
            bytes: self.bytes.get(),
            records: self.records.get(),
            xml_depth: self.xml_depth.get(),
        }
    }

    /// Adds successfully consumed logical payload bytes and checks the byte budget.
    pub fn charge_bytes(&self, added: u64, context: &str) -> std::io::Result<()> {
        let value = self.bytes.get().saturating_add(added);
        self.bytes.set(value);
        if value > self.limits.max_bytes {
            Err(self.io_violation(context))
        } else {
            Ok(())
        }
    }

    /// Adds a delivered point record and checks the record budget.
    pub fn charge_record(&self) -> crate::Result<()> {
        let value = self.records.get().saturating_add(1);
        self.records.set(value);
        if value > self.limits.max_records {
            crate::Error::read_violation(format!(
                "point record budget exceeded: {self}"
            ))
        } else {
            Ok(())
        }
    }

    /// Updates the maximum observed XML nesting depth and checks the depth budget.
    pub fn check_xml_depth(&self, depth: u32) -> crate::Result<()> {
        if depth > self.xml_depth.get() {
            self.xml_depth.set(depth);
        }
        if depth > self.limits.max_xml_depth {
            crate::Error::read_violation(format!(
                "XML recursion budget exceeded: observed depth {depth} exceeds limit {}: {self}",
                self.limits.max_xml_depth
            ))
        } else {
            Ok(())
        }
    }

    fn io_violation(&self, context: &str) -> std::io::Error {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("byte budget exceeded while {context}: {self}"),
        )
    }
}

impl Display for Budget {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let usage = self.usage();
        write!(
            f,
            "budgets [bytes used/limit: {}/{}, records used/limit: {}/{}, xml depth used/limit: {}/{}]",
            usage.bytes,
            format_limit(self.limits.max_bytes),
            usage.records,
            format_limit(self.limits.max_records),
            usage.xml_depth,
            self.limits.max_xml_depth,
        )
    }
}

fn format_limit(limit: u64) -> String {
    if limit == u64::MAX {
        String::from("unlimited")
    } else {
        limit.to_string()
    }
}
