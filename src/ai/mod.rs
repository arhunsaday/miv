//! Groundwork for AI-assisted editing.
//!
//! Two decisions shape this, and both are about keeping the editor honest:
//!
//! **The provider is an external command.** miv does not speak HTTP, hold an
//! API key, or know which model you use: it runs a command you configure, puts
//! a prompt on its stdin and reads the reply from its stdout. That works with
//! `claude -p`, `llm`, `ollama run`, or a three-line shell script, keeps
//! credentials in the place you already keep them, and makes the whole path
//! testable with a fake script.
//!
//! **An answer is a proposal, not an edit.** Nothing reaches the buffer until
//! you say so. A proposal carries the exact range it would replace and the
//! text it would put there, so it can be shown as a diff, applied as one undo
//! step, and — because the transaction records who authored it — told apart
//! from your own work afterwards.

pub mod proposal;

use crate::config::AiConfig;
use std::time::Duration;

/// A question on its way to the provider.
#[derive(Clone, Debug)]
pub struct Request {
    pub instruction: String,
    /// Which buffer and range the answer will replace.
    pub buffer_id: usize,
    pub start: usize,
    pub end: usize,
    pub original: String,
}

/// Build the prompt.
///
/// It is deliberately blunt about wanting only the replacement: a reply
/// wrapped in prose or code fences would have to be unwrapped by guesswork,
/// and guessing wrong means mangling someone's file.
pub fn prompt(request: &Request, filetype: &str, context: &str) -> String {
    let mut prompt = String::new();
    prompt.push_str(&format!(
        "You are editing a {filetype} file in a text editor.\n\
         Rewrite the REGION below according to the instruction.\n\n\
         Reply with the replacement text for the REGION and nothing else: no \
         explanation, no commentary, and no code fences. Preserve the \
         surrounding indentation style.\n\n"
    ));
    prompt.push_str(&format!("INSTRUCTION: {}\n\n", request.instruction));
    if !context.trim().is_empty() {
        prompt.push_str(&format!("CONTEXT (do not rewrite this):\n{context}\n\n"));
    }
    prompt.push_str(&format!("REGION:\n{}", request.original));
    prompt
}

/// Strip the wrapping a model adds anyway.
///
/// Asking for bare text is not the same as getting it, so a fence around the
/// whole reply is removed. Anything more ambitious would risk editing the
/// content itself.
pub fn clean(reply: &str) -> String {
    let trimmed = reply.trim_matches('\n');
    let lines: Vec<&str> = trimmed.lines().collect();
    let fenced = lines.len() >= 2
        && lines[0].trim_start().starts_with("```")
        && lines[lines.len() - 1].trim() == "```";
    if fenced {
        let inner = &lines[1..lines.len() - 1];
        let mut out = inner.join("\n");
        if reply.ends_with('\n') {
            out.push('\n');
        }
        return out;
    }
    reply.to_string()
}

/// Make a reply end the way the region it replaces does.
///
/// A provider naturally finishes with a newline, but a region taken from the
/// middle of a line does not have one. Pasting the reply verbatim would insert
/// a line break nobody asked for, so the shape is matched instead.
pub fn match_line_shape(original: &str, reply: &str) -> String {
    let original_ends = original.ends_with('\n');
    let reply_ends = reply.ends_with('\n');
    if original_ends == reply_ends {
        return reply.to_string();
    }
    if reply_ends {
        reply.trim_end_matches('\n').to_string()
    } else {
        format!("{reply}\n")
    }
}

pub fn timeout(config: &AiConfig) -> Duration {
    Duration::from_millis(config.timeout_ms)
}

/// One turn of the conversation shown in the sidebar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Turn {
    You(String),
    Assistant(String),
    Note(String),
}

impl Turn {
    pub fn speaker(&self) -> &'static str {
        match self {
            Turn::You(_) => "you",
            Turn::Assistant(_) => "ai",
            Turn::Note(_) => "··",
        }
    }

    pub fn text(&self) -> &str {
        match self {
            Turn::You(text) | Turn::Assistant(text) | Turn::Note(text) => text,
        }
    }
}

/// The transcript behind the sidebar's chat view.
#[derive(Default, Clone)]
pub struct Conversation {
    turns: Vec<Turn>,
}

/// Turns kept. A transcript is context, not an archive.
const MAX_TURNS: usize = 200;

impl Conversation {
    pub fn say(&mut self, turn: Turn) {
        if self.turns.len() >= MAX_TURNS {
            self.turns.remove(0);
        }
        self.turns.push(turn);
    }

    pub fn turns(&self) -> &[Turn] {
        &self.turns
    }

    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Request {
        Request {
            instruction: "add error handling".to_string(),
            buffer_id: 1,
            start: 0,
            end: 10,
            original: "let x = f();".to_string(),
        }
    }

    #[test]
    fn the_prompt_carries_the_instruction_the_region_and_the_filetype() {
        let built = prompt(&request(), "Rust", "fn caller() {}");
        assert!(built.contains("Rust"));
        assert!(built.contains("add error handling"));
        assert!(built.contains("let x = f();"));
        assert!(built.contains("fn caller() {}"));
        // And it asks for bare text, because unwrapping prose is guesswork.
        assert!(built.contains("nothing else"));
    }

    #[test]
    fn context_is_left_out_when_there_is_none() {
        let built = prompt(&request(), "Rust", "   \n  ");
        assert!(!built.contains("CONTEXT"));
    }

    #[test]
    fn a_fenced_reply_is_unwrapped() {
        assert_eq!(clean("```rust\nlet x = 1;\n```"), "let x = 1;");
        assert_eq!(clean("```\na\nb\n```"), "a\nb");
        // A trailing newline in the reply is preserved.
        assert_eq!(clean("```\na\n```\n"), "a\n");
    }

    #[test]
    fn an_unfenced_reply_is_left_exactly_as_it_came() {
        // Anything cleverer risks editing the content.
        assert_eq!(clean("let x = 1;"), "let x = 1;");
        assert_eq!(clean("  indented\n"), "  indented\n");
        // A fence that is only part of the reply is content, not wrapping.
        assert_eq!(clean("see ```this```"), "see ```this```");
    }

    #[test]
    fn a_reply_is_reshaped_to_the_region_it_replaces() {
        // The region is part of a line, so the reply must not add a break.
        assert_eq!(match_line_shape("let x = 1;", "let x = 2;\n"), "let x = 2;");
        // The region is whole lines, so the reply must end with one.
        assert_eq!(match_line_shape("a\nb\n", "c\nd"), "c\nd\n");
        // Already matching, so left alone.
        assert_eq!(match_line_shape("a\n", "b\n"), "b\n");
        assert_eq!(match_line_shape("a", "b"), "b");
    }

    #[test]
    fn a_transcript_is_bounded() {
        let mut conversation = Conversation::default();
        assert!(conversation.is_empty());
        for index in 0..MAX_TURNS + 20 {
            conversation.say(Turn::You(index.to_string()));
        }
        assert_eq!(conversation.turns().len(), MAX_TURNS);
        // The oldest went, the newest stayed.
        assert_eq!(
            conversation.turns().last().unwrap().text(),
            (MAX_TURNS + 19).to_string()
        );
    }
}
