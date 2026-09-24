//! Resource budgets for parsing potentially untrusted E57 input.
//!
//! The budget tracking used here is shared by the production parsing path
//! ([`E57Reader`](crate::E57Reader)) and the tests that exercise it. There is
//! no separate, test-only parser: both enter through the same reader APIs and
//! share the same counters and limit checks.

use crate::Error;
use crate::Result;

/// Kind of resource that exceeded its parser budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BudgetKind {
    /// Number of bytes consumed from the underlying input.
    Bytes,
    /// Number of point records decoded from compressed vectors.
    Records,
    /// Nesting depth encountered while traversing XML structures.
    Recursion,
}

impl BudgetKind {
    fn label(self) -> &'static str {
        match self {
            BudgetKind::Bytes => "byte",
            BudgetKind::Records => "record",
            BudgetKind::Recursion => "recursion",
        }
    }
}

/// Immutable description of the parser resource budgets.
///
/// The default values are effectively unlimited to keep the behavior of
/// existing callers unchanged. Use the builder methods to install tighter
/// limits when reading untrusted input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseBudget {
    /// Maximum number of bytes that may be consumed from the input.
    pub max_bytes: u64,
    /// Maximum number of point records that may be decoded.
    pub max_records: u64,
    /// Maximum nesting depth that may be encountered in the XML section.
    pub max_recursion: u32,
}

impl Default for ParseBudget {
    fn default() -> Self {
        Self {
            max_bytes: u64::MAX,
            max_records: u64::MAX,
            max_recursion: DEFAULT_MAX_RECURSION,
        }
    }
}

impl ParseBudget {
    /// Creates a new budget configuration with the supplied limits.
    pub fn new(max_bytes: u64, max_records: u64, max_recursion: u32) -> Self {
        Self {
            max_bytes,
            max_records,
            max_recursion,
        }
    }

    /// Limits the number of bytes that may be consumed from the input.
    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    /// Limits the number of point records that may be decoded.
    pub fn with_max_records(mut self, max_records: u64) -> Self {
        self.max_records = max_records;
        self
    }

    /// Limits the nesting depth that may be encountered in the XML section.
    pub fn with_max_recursion(mut self, max_recursion: u32) -> Self {
        self.max_recursion = max_recursion;
        self
    }
}

/// Snapshot of how much of a parser budget has been consumed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BudgetUsage {
    /// Bytes consumed from the underlying input.
    pub bytes: u64,
    /// Point records decoded from compressed vectors.
    pub records: u64,
    /// Greatest XML nesting depth encountered.
    pub recursion: u32,
}

/// Mutable, parser-internal budget tracker shared by all parsing stages.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BudgetTracker {
    limits: ParseBudget,
    usage: BudgetUsage,
}

impl BudgetTracker {
    /// Starts a fresh tracker for the supplied budget configuration.
    pub(crate) fn new(limits: ParseBudget) -> Self {
        Self {
            limits,
            usage: BudgetUsage::default(),
        }
    }

    /// Returns the immutable limit configuration.
    pub(crate) fn limits(&self) -> ParseBudget {
        self.limits
    }

    /// Returns a snapshot of the current counters.
    pub(crate) fn usage(&self) -> BudgetUsage {
        self.usage
    }

    /// Charges the supplied number of bytes read from the input.
    pub(crate) fn charge_bytes(&mut self, amount: u64, location: &str) -> Result<()> {
        self.usage.bytes = self.usage.bytes.saturating_add(amount);
        if self.usage.bytes > self.limits.max_bytes {
            Err(self.exceeded(BudgetKind::Bytes, location))
        } else {
            Ok(())
        }
    }

    /// Charges a single decoded point record.
    pub(crate) fn charge_record(&mut self, location: &str) -> Result<()> {
        self.usage.records = self.usage.records.saturating_add(1);
        if self.usage.records > self.limits.max_records {
            Err(self.exceeded(BudgetKind::Records, location))
        } else {
            Ok(())
        }
    }

    /// Reports the observed XML nesting depth if it is deeper than allowed.
    pub(crate) fn check_recursion(&mut self, depth: u32, location: &str) -> Result<()> {
        self.usage.recursion = self.usage.recursion.max(depth);
        if depth > self.limits.max_recursion {
            Err(self.exceeded(BudgetKind::Recursion, location))
        } else {
            Ok(())
        }
    }

    fn exceeded(&self, kind: BudgetKind, location: &str) -> Error {
        let ParseBudget {
            max_bytes,
            max_records,
            max_recursion,
        } = self.limits;
        let BudgetUsage {
            bytes,
            records,
            recursion,
        } = self.usage;
        Error::invalid(format!(
            "Parser {kind} budget exceeded at {location}: \
usage bytes={bytes}/max {max_bytes}, records={records}/max {max_records}, \
recursion depth={recursion}/max {max_recursion}",
            kind = kind.label()
        ))
    }
}

/// Default nesting depth used when no explicit XML recursion limit is set.
pub const DEFAULT_MAX_RECURSION: u32 = 256;

/// Deterministically walks the parsed XML document and reports the greatest
/// element nesting depth below the supplied root node.
///
/// The depth of the supplied node itself is reported as one. Every violation
/// is checked against the same budget tracker that guards the binary parsing.
pub(crate) fn charge_xml_depth(
    budget: &mut BudgetTracker,
    node: &roxmltree::Node,
) -> Result<()> {
    let mut deepest_path = String::new();
    let mut deepest_depth = 0_u32;
    walk_depth(node, 1, &mut String::new(), &mut |depth, path| {
        if depth > deepest_depth {
            deepest_depth = depth;
            deepest_path = path.to_owned();
        }
    });
    let location = if deepest_path.is_empty() {
        String::from("XML element")
    } else {
        format!("XML element '{deepest_path}'")
    };
    budget.check_recursion(deepest_depth, &location)
}

fn walk_depth<F: FnMut(u32, &str)>(
    node: &roxmltree::Node,
    depth: u32,
    path: &mut String,
    visit: &mut F,
) {
    let prev_len = path.len();
    if node.is_element() {
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(node.tag_name().name());
        visit(depth, path);
    }
    let child_depth = depth + u32::from(node.is_element());
    for child in node.children().filter(|n| n.is_element()) {
        walk_depth(&child, child_depth, path, visit);
    }
    path.truncate(prev_len);
}
