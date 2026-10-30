//! The single error type crossing the WASM boundary, plus non-fatal warnings.

use serde::Serialize;

/// A fatal problem. Analysis stops and the UI shows this instead of any tabs.
#[derive(Debug, Clone, Serialize)]
pub struct EngineError {
    /// 1-based source line, when the problem can be attributed to one.
    pub line: Option<usize>,
    pub message: String,
    /// Concrete next action for the user. Worth writing for every error.
    pub hint: Option<String>,
}

impl EngineError {
    pub fn new(message: impl Into<String>) -> Self {
        EngineError { line: None, message: message.into(), hint: None }
    }

    pub fn at(line: usize, message: impl Into<String>) -> Self {
        EngineError { line: Some(line), message: message.into(), hint: None }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// Severity of a non-fatal remark shown in the left panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Something is probably not what the user meant.
    Warning,
    /// Something is fine but commonly misunderstood (e.g. left recursion).
    Note,
}

/// A remark that does not block analysis.
#[derive(Debug, Clone, Serialize)]
pub struct Warning {
    pub severity: Severity,
    pub line: Option<usize>,
    pub message: String,
}

impl Warning {
    pub fn warn(line: Option<usize>, message: impl Into<String>) -> Self {
        Warning { severity: Severity::Warning, line, message: message.into() }
    }

    pub fn note(message: impl Into<String>) -> Self {
        Warning { severity: Severity::Note, line: None, message: message.into() }
    }
}

pub type Result<T> = core::result::Result<T, EngineError>;
