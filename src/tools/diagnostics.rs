//! Diagnostics from external checkers.
//!
//! miv does not know what a YAML error is. It knows how to run a command and
//! read `file:line:col: message` out of its output, which is enough to wire up
//! yamllint, shellcheck, hadolint, actionlint, terraform and anything else that
//! follows the same convention.

use crate::config::CheckerConfig;
use ratatui::style::Color;
use regex::Regex;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Ordered worst-first so `min` picks the most severe.
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.to_ascii_lowercase();
        if text.contains("err") || text == "e" || text.contains("fatal") {
            Some(Severity::Error)
        } else if text.contains("warn") || text == "w" {
            Some(Severity::Warning)
        } else if text.contains("info") || text.contains("note") || text.contains("hint") {
            Some(Severity::Info)
        } else {
            None
        }
    }

    pub fn sign(self) -> &'static str {
        match self {
            Severity::Error => "●",
            Severity::Warning => "▲",
            Severity::Info => "?",
        }
    }

    pub fn color(self) -> Color {
        match self {
            Severity::Error => Color::Red,
            Severity::Warning => Color::Yellow,
            Severity::Info => Color::Blue,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    /// Zero-based, like every other line number inside the editor.
    pub line: usize,
    pub col: Option<usize>,
    pub severity: Severity,
    pub message: String,
    /// Which checker said so, shown in the list and the virtual text.
    pub source: String,
}

/// A compiled checker: a command plus the pattern that reads its output.
#[derive(Clone, Debug)]
pub struct Checker {
    pub name: String,
    pub command: Vec<String>,
    pattern: Regex,
    default_severity: Severity,
    filetypes: Vec<String>,
    extensions: Vec<String>,
}

impl Checker {
    pub fn compile(config: &CheckerConfig) -> Result<Self, String> {
        if config.command.is_empty() {
            return Err("a checker needs a command".to_string());
        }
        let pattern = Regex::new(&config.pattern)
            .map_err(|e| format!("checker {}: bad pattern: {e}", config.display_name()))?;
        if !pattern.capture_names().any(|name| name == Some("line")) {
            return Err(format!(
                "checker {}: pattern must capture (?<line>...)",
                config.display_name()
            ));
        }
        let default_severity = match &config.severity {
            Some(text) => Severity::parse(text).ok_or_else(|| {
                format!(
                    "checker {}: unknown severity {text:?}",
                    config.display_name()
                )
            })?,
            None => Severity::Warning,
        };
        Ok(Self {
            name: config.display_name(),
            command: config.command.clone(),
            pattern,
            default_severity,
            filetypes: config
                .filetypes
                .iter()
                .map(|f| f.to_ascii_lowercase())
                .collect(),
            extensions: config
                .extensions
                .iter()
                .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
                .collect(),
        })
    }

    pub fn applies_to(&self, syntax_name: &str, path: Option<&Path>) -> bool {
        crate::tools::external::applies(&self.filetypes, &self.extensions, syntax_name, path)
    }

    /// Pull diagnostics out of a tool's output, ignoring lines that do not
    /// match: tools mix diagnostics with summaries and progress noise.
    pub fn parse(&self, output: &str) -> Vec<Diagnostic> {
        let mut found = Vec::new();
        for line in output.lines() {
            let Some(captures) = self.pattern.captures(line) else {
                continue;
            };
            let Some(number) = captures
                .name("line")
                .and_then(|m| m.as_str().trim().parse::<usize>().ok())
            else {
                continue;
            };
            let col = captures
                .name("col")
                .and_then(|m| m.as_str().trim().parse::<usize>().ok())
                .map(|c| c.saturating_sub(1));
            let severity = captures
                .name("severity")
                .and_then(|m| Severity::parse(m.as_str()))
                .unwrap_or(self.default_severity);
            let message = captures
                .name("message")
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_else(|| line.trim().to_string());
            found.push(Diagnostic {
                line: number.saturating_sub(1),
                col,
                severity,
                message,
                source: self.name.clone(),
            });
        }
        found
    }
}

/// Every checker's findings for one buffer, kept per tool so re-running one
/// checker does not discard another's.
#[derive(Default)]
pub struct Diagnostics {
    by_tool: BTreeMap<String, Vec<Diagnostic>>,
}

impl Diagnostics {
    pub fn replace(&mut self, tool: &str, items: Vec<Diagnostic>) {
        if items.is_empty() {
            self.by_tool.remove(tool);
        } else {
            self.by_tool.insert(tool.to_string(), items);
        }
    }

    pub fn clear(&mut self) {
        self.by_tool.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.by_tool.is_empty()
    }

    /// All diagnostics, ordered by position so navigation and the list agree.
    pub fn sorted(&self) -> Vec<&Diagnostic> {
        let mut all: Vec<&Diagnostic> = self.by_tool.values().flatten().collect();
        all.sort_by_key(|d| (d.line, d.col.unwrap_or(0), d.severity));
        all
    }

    pub fn on_line(&self, line: usize) -> Vec<&Diagnostic> {
        let mut items: Vec<&Diagnostic> = self
            .by_tool
            .values()
            .flatten()
            .filter(|d| d.line == line)
            .collect();
        items.sort_by_key(|d| (d.severity, d.col.unwrap_or(0)));
        items
    }

    /// The sign to draw in the gutter: the most severe thing on the line.
    pub fn worst_on_line(&self, line: usize) -> Option<Severity> {
        self.by_tool
            .values()
            .flatten()
            .filter(|d| d.line == line)
            .map(|d| d.severity)
            .min()
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        let mut counts = (0, 0, 0);
        for diagnostic in self.by_tool.values().flatten() {
            match diagnostic.severity {
                Severity::Error => counts.0 += 1,
                Severity::Warning => counts.1 += 1,
                Severity::Info => counts.2 += 1,
            }
        }
        counts
    }

    /// The next diagnostic after `line`, wrapping around the file.
    pub fn next_from(&self, line: usize, forward: bool) -> Option<&Diagnostic> {
        let sorted = self.sorted();
        if sorted.is_empty() {
            return None;
        }
        if forward {
            sorted
                .iter()
                .find(|d| d.line > line)
                .or_else(|| sorted.first())
                .copied()
        } else {
            sorted
                .iter()
                .rev()
                .find(|d| d.line < line)
                .or_else(|| sorted.last())
                .copied()
        }
    }
}

/// Checkers that work out of the box on a machine that has the tools.
///
/// These are deliberately conservative: each one is a tool that prints
/// `file:line:col: message`, which keeps the pattern honest and the list easy
/// to extend.
pub fn builtin_checkers() -> Vec<CheckerConfig> {
    let checker = |name: &str,
                   command: &[&str],
                   pattern: &str,
                   extensions: &[&str],
                   filetypes: &[&str],
                   severity: &str| {
        CheckerConfig {
            name: Some(name.to_string()),
            command: command.iter().map(|s| s.to_string()).collect(),
            pattern: pattern.to_string(),
            severity: Some(severity.to_string()),
            filetypes: filetypes.iter().map(|s| s.to_string()).collect(),
            extensions: extensions.iter().map(|s| s.to_string()).collect(),
        }
    };

    vec![
        checker(
            "yamllint",
            &["yamllint", "-f", "parsable", "$FILE"],
            r"^[^:]*:(?<line>\d+):(?<col>\d+):\s*\[(?<severity>\w+)\]\s*(?<message>.*)$",
            &["yml", "yaml"],
            &["yaml"],
            "warning",
        ),
        checker(
            "actionlint",
            &["actionlint", "-oneline", "-no-color", "$FILE"],
            r"^[^:]*:(?<line>\d+):(?<col>\d+):\s*(?<message>.*)$",
            &[],
            &[],
            "error",
        ),
        checker(
            "shellcheck",
            &["shellcheck", "-f", "gcc", "-"],
            r"^[^:]*:(?<line>\d+):(?<col>\d+):\s*(?<severity>\w+):\s*(?<message>.*)$",
            &["sh", "bash", "zsh"],
            &["shell", "bash"],
            "warning",
        ),
        checker(
            "hadolint",
            &["hadolint", "--no-color", "-f", "tty", "-"],
            r"^[^:]*:(?<line>\d+)\s+\S+\s+(?<severity>\w+):\s*(?<message>.*)$",
            &["dockerfile"],
            &["dockerfile"],
            "warning",
        ),
        checker(
            "jq",
            &["jq", "-e", ".", "$FILE"],
            r"(?<message>.*?)\s+at line (?<line>\d+), column (?<col>\d+)",
            &["json"],
            &["json"],
            "error",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin(name: &str) -> Checker {
        let config = builtin_checkers()
            .into_iter()
            .find(|c| c.display_name() == name)
            .unwrap_or_else(|| panic!("no built-in checker called {name}"));
        Checker::compile(&config).expect("built-in checkers must compile")
    }

    #[test]
    fn severities_are_read_from_the_words_tools_actually_use() {
        assert_eq!(Severity::parse("error"), Some(Severity::Error));
        assert_eq!(Severity::parse("ERROR"), Some(Severity::Error));
        assert_eq!(Severity::parse("fatal"), Some(Severity::Error));
        assert_eq!(Severity::parse("warning"), Some(Severity::Warning));
        assert_eq!(Severity::parse("warn"), Some(Severity::Warning));
        assert_eq!(Severity::parse("note"), Some(Severity::Info));
        assert_eq!(Severity::parse("style"), None);
    }

    #[test]
    fn severity_orders_worst_first_so_the_gutter_shows_the_worst() {
        assert!(Severity::Error < Severity::Warning);
        assert_eq!(
            [Severity::Warning, Severity::Error, Severity::Info]
                .into_iter()
                .min(),
            Some(Severity::Error)
        );
    }

    #[test]
    fn a_pattern_without_a_line_group_is_refused_at_load() {
        let config = CheckerConfig {
            command: vec!["true".into()],
            pattern: r"^(?<message>.*)$".into(),
            ..Default::default()
        };
        let error = Checker::compile(&config).unwrap_err();
        assert!(error.contains("line"), "{error}");

        let config = CheckerConfig {
            command: vec!["true".into()],
            pattern: r"(?<line>\d+".into(),
            ..Default::default()
        };
        assert!(Checker::compile(&config).is_err());

        let config = CheckerConfig {
            pattern: r"(?<line>\d+)".into(),
            ..Default::default()
        };
        assert!(Checker::compile(&config).unwrap_err().contains("command"));
    }

    #[test]
    fn every_builtin_checker_compiles() {
        for config in builtin_checkers() {
            Checker::compile(&config).unwrap_or_else(|e| panic!("{}: {e}", config.display_name()));
        }
    }

    // These lock in the output formats of the tools we ship patterns for.
    // A tool changing its format would otherwise fail silently.

    #[test]
    fn yamllint_output_parses() {
        let found = builtin("yamllint").parse(
            "conf.yaml:3:1: [warning] too many blank lines (1 > 0 allowed) (empty-lines)\n\
             conf.yaml:7:81: [error] line too long (95 > 80 characters) (line-length)",
        );
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].line, 2);
        assert_eq!(found[0].col, Some(0));
        assert_eq!(found[0].severity, Severity::Warning);
        assert!(found[0].message.starts_with("too many blank lines"));
        assert_eq!(found[1].severity, Severity::Error);
        assert_eq!(found[1].line, 6);
    }

    #[test]
    fn shellcheck_output_parses() {
        let found = builtin("shellcheck")
            .parse("-:5:6: warning: x is referenced but not assigned. [SC2154]");
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].line, found[0].col), (4, Some(5)));
        assert_eq!(found[0].severity, Severity::Warning);
        assert_eq!(found[0].source, "shellcheck");
    }

    #[test]
    fn hadolint_output_parses() {
        let found = builtin("hadolint")
            .parse("-:3 DL3006 warning: Always tag the version of an image explicitly");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].line, 2);
        assert_eq!(found[0].severity, Severity::Warning);
        assert!(found[0].message.starts_with("Always tag"));
    }

    #[test]
    fn actionlint_output_parses() {
        let found = builtin("actionlint")
            .parse(".github/workflows/ci.yml:12:9: property \"foo\" is not defined [expression]");
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].line, found[0].col), (11, Some(8)));
        // No severity group, so the configured default applies.
        assert_eq!(found[0].severity, Severity::Error);
    }

    #[test]
    fn jq_output_parses() {
        let found = builtin("jq")
            .parse("jq: parse error: Expected separator between values at line 4, column 0");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].line, 3);
        assert_eq!(found[0].severity, Severity::Error);
        assert!(found[0].message.contains("Expected separator"));
    }

    #[test]
    fn lines_that_do_not_match_are_ignored() {
        // Tools mix diagnostics with summaries, banners and progress output.
        let found = builtin("yamllint").parse(
            "Loading configuration...\n\
             conf.yaml:1:1: [error] syntax error (syntax)\n\
             \n\
             2 problems found",
        );
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn a_checker_applies_by_filetype_or_by_extension() {
        let checker = builtin("yamllint");
        assert!(checker.applies_to("YAML", None));
        assert!(checker.applies_to("yaml", None));
        assert!(checker.applies_to("Plain Text", Some(std::path::Path::new("/tmp/deploy.yml"))));
        assert!(!checker.applies_to("Rust", Some(std::path::Path::new("/tmp/main.rs"))));
    }

    #[test]
    fn a_checker_with_no_filters_applies_everywhere() {
        let config = CheckerConfig {
            command: vec!["true".into()],
            pattern: r"(?<line>\d+)".into(),
            ..Default::default()
        };
        let checker = Checker::compile(&config).unwrap();
        assert!(checker.applies_to("Rust", None));
        assert!(checker.applies_to("YAML", None));
    }

    fn diagnostic(line: usize, severity: Severity, source: &str) -> Diagnostic {
        Diagnostic {
            line,
            col: None,
            severity,
            message: format!("{severity:?} on {line}"),
            source: source.to_string(),
        }
    }

    #[test]
    fn one_checkers_results_do_not_displace_anothers() {
        let mut store = Diagnostics::default();
        store.replace("a", vec![diagnostic(1, Severity::Error, "a")]);
        store.replace("b", vec![diagnostic(2, Severity::Warning, "b")]);
        assert_eq!(store.counts(), (1, 1, 0));

        // Re-running `a` with nothing to say clears only its own findings.
        store.replace("a", Vec::new());
        assert_eq!(store.counts(), (0, 1, 0));
        assert_eq!(store.sorted().len(), 1);
        assert_eq!(store.sorted()[0].source, "b");
    }

    #[test]
    fn the_gutter_shows_the_worst_thing_on_the_line() {
        let mut store = Diagnostics::default();
        store.replace(
            "a",
            vec![
                diagnostic(4, Severity::Info, "a"),
                diagnostic(4, Severity::Error, "a"),
                diagnostic(4, Severity::Warning, "a"),
            ],
        );
        assert_eq!(store.worst_on_line(4), Some(Severity::Error));
        assert_eq!(store.worst_on_line(5), None);
        // ...and the virtual text shows it first.
        assert_eq!(store.on_line(4)[0].severity, Severity::Error);
    }

    #[test]
    fn navigation_moves_through_diagnostics_and_wraps() {
        let mut store = Diagnostics::default();
        store.replace(
            "a",
            vec![
                diagnostic(2, Severity::Error, "a"),
                diagnostic(9, Severity::Warning, "a"),
            ],
        );
        assert_eq!(store.next_from(0, true).unwrap().line, 2);
        assert_eq!(store.next_from(2, true).unwrap().line, 9);
        assert_eq!(store.next_from(9, true).unwrap().line, 2, "should wrap");
        assert_eq!(store.next_from(9, false).unwrap().line, 2);
        assert_eq!(store.next_from(0, false).unwrap().line, 9, "should wrap");
        assert!(Diagnostics::default().next_from(0, true).is_none());
    }
}
