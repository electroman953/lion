use crate::Span;

/// The kind of problem, which decides its title and whether it stops the program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// The program is refused before it runs, in both modes.
    Error,
    /// A non-blocking remark found at compile time.
    Warning,
    /// A non-blocking runtime remark, reported only in interpreted mode (spec §22.3).
    Alert,
    /// A runtime failure that stops the program and cannot be intercepted (spec §18.1).
    Bug,
    /// A defect in the Lion implementation itself, never in the user's program.
    Internal,
}

impl Severity {
    pub fn title(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Alert => "alert",
            Severity::Bug => "bug",
            Severity::Internal => "internal compiler error",
        }
    }
}

/// A span of code pointed at by a diagnostic, with an optional explanation.
#[derive(Clone, Debug)]
pub struct Label {
    pub span: Span,
    pub message: String,
    /// The primary label gives the reported location and is underlined with `^`;
    /// secondary labels add context and are underlined with `-`.
    pub primary: bool,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub labels: Vec<Label>,
    /// Free-form facts shown after the excerpt, such as `expected: Int`.
    pub notes: Vec<String>,
    /// Suggestions to fix the problem.
    pub help: Vec<String>,
}

impl Diagnostic {
    pub fn new(severity: Severity, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            severity,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
            help: Vec::new(),
        }
    }

    pub fn error(message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Severity::Error, message)
    }

    pub fn alert(message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Severity::Alert, message)
    }

    pub fn bug(message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Severity::Bug, message)
    }

    pub fn internal(message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Severity::Internal, message)
            .with_note("this is a defect in the Lion implementation, not in your program; please report it")
    }

    /// A part of the language that this version of the implementation does not support
    /// yet: always an error, never a silent approximation.
    pub fn not_implemented(span: Span, what: &str, section: &str) -> Diagnostic {
        Diagnostic::error(format!("not implemented yet: {what}")).with_primary(span, "").with_note(format!(
            "this part of Lion (spec {section}) is not supported by this version of the implementation"
        ))
    }

    pub fn with_primary(mut self, span: Span, message: impl Into<String>) -> Diagnostic {
        self.labels.push(Label { span, message: message.into(), primary: true });
        self
    }

    pub fn with_secondary(mut self, span: Span, message: impl Into<String>) -> Diagnostic {
        self.labels.push(Label { span, message: message.into(), primary: false });
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Diagnostic {
        self.notes.push(note.into());
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Diagnostic {
        self.help.push(help.into());
        self
    }

    /// Whether this diagnostic prevents the program from running or continuing.
    pub fn is_fatal(&self) -> bool {
        matches!(self.severity, Severity::Error | Severity::Bug | Severity::Internal)
    }
}
