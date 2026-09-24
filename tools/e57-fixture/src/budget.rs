use std::fmt::{Display, Formatter};

/// Resource budgets enforced by the fixture gate.
///
/// The byte budget is shared with the parser itself (`e57::MAX_XML_SIZE`),
/// the record and recursion budgets are gate policies for the small
/// fixtures used here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budgets {
    /// Maximum accepted size of the XML section in bytes.
    /// This is the same byte budget the parser itself enforces.
    pub max_xml_bytes: usize,
    /// Maximum number of records the gate will read from a point cloud.
    pub max_records: u64,
    /// Maximum accepted XML tree recursion depth.
    pub max_xml_depth: usize,
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            max_xml_bytes: e57::MAX_XML_SIZE,
            max_records: 1_000_000,
            max_xml_depth: 64,
        }
    }
}

/// Observed resource usage of one gate run together with the budgets.
///
/// Included in every failure report so that the byte, record and
/// recursion budgets of the parser are always visible. Observed
/// values that could not be measured (for example because parsing
/// failed earlier) are reported as unknown instead of being guessed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BudgetReport {
    /// Observed physical size of the fixture file in bytes.
    pub file_bytes: usize,
    /// Observed size of the XML section in bytes, if it could be read.
    pub xml_bytes: Option<usize>,
    /// Observed number of records read, if reading reached that stage.
    pub records_read: Option<u64>,
    /// Observed XML tree recursion depth, if the XML could be parsed.
    pub xml_depth: Option<usize>,
    /// Byte budget for the XML section (shared with the parser).
    pub max_xml_bytes: usize,
    /// Record budget of the gate.
    pub max_records: u64,
    /// Recursion budget of the gate.
    pub max_xml_depth: usize,
}

impl BudgetReport {
    pub(crate) fn new(file_bytes: usize, budgets: &Budgets) -> Self {
        Self {
            file_bytes,
            xml_bytes: None,
            records_read: None,
            xml_depth: None,
            max_xml_bytes: budgets.max_xml_bytes,
            max_records: budgets.max_records,
            max_xml_depth: budgets.max_xml_depth,
        }
    }
}

impl Display for BudgetReport {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        let xml_bytes = self
            .xml_bytes
            .map(|v| v.to_string())
            .unwrap_or_else(|| String::from("unknown"));
        let records = self
            .records_read
            .map(|v| v.to_string())
            .unwrap_or_else(|| String::from("unknown"));
        let depth = self
            .xml_depth
            .map(|v| v.to_string())
            .unwrap_or_else(|| String::from("unknown"));
        write!(
            f,
            "budgets: file-bytes={} xml-bytes={}/{} records={}/{} xml-depth={}/{}",
            self.file_bytes,
            xml_bytes,
            self.max_xml_bytes,
            records,
            self.max_records,
            depth,
            self.max_xml_depth
        )
    }
}
