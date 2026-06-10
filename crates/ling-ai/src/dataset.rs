//! The **Ling Dialog Standard** (`.lingdialog`) — a tiny, human-writable corpus
//! format for training the in-engine dialog model ([`crate::dialog_lm`]).
//!
//! It is line-oriented and diff-friendly so writers and designers can author it
//! by hand or generate it from a spreadsheet, then `ling`/`lingfu` can load it
//! directly. The format is intentionally minimal:
//!
//! ```text
//! # comments start with '#'
//! @meta title: Tavern Banter        # arbitrary key/value metadata
//! @meta lang: en
//!
//! = greeting                        # '=' starts a new conversation (label optional)
//! npc:  Welcome, traveler!          # "speaker: text" is one turn
//! hero: Thanks. Any rumors?
//! npc:  They say the old mine
//!       woke something up.          # an indented line with no colon continues the turn
//!
//! =                                 # next conversation
//! npc:  Buy something or leave.
//! ```
//!
//! Rules
//! - `#` to end-of-line is a comment.
//! - `@meta key: value` records dataset metadata.
//! - `=` (optionally `= label`) begins a new conversation.
//! - `speaker: text` is a turn; the speaker is the text before the first colon.
//! - A non-directive line with no colon continues the previous turn (multi-line).
//! - Blank lines are ignored. Turns before any `=` open an implicit conversation.

/// One line spoken by one participant.
#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub speaker: String,
    pub text: String,
}

/// An ordered exchange of turns.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Conversation {
    pub label: Option<String>,
    pub turns: Vec<Turn>,
}

/// A parsed `.lingdialog` corpus.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dataset {
    pub meta: Vec<(String, String)>,
    pub conversations: Vec<Conversation>,
}

impl Dataset {
    /// Parse the `.lingdialog` text format. Never fails — malformed lines are
    /// skipped so a stray character can't break a training run.
    pub fn parse(src: &str) -> Dataset {
        let mut ds = Dataset::default();
        let mut cur: Option<Conversation> = None;

        for raw in src.lines() {
            let line = raw.split('#').next().unwrap_or("");
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // @meta key: value
            if let Some(rest) = trimmed.strip_prefix("@meta") {
                if let Some((k, v)) = rest.split_once(':') {
                    ds.meta.push((k.trim().to_string(), v.trim().to_string()));
                }
                continue;
            }

            // = [label]  → new conversation
            if let Some(rest) = trimmed.strip_prefix('=') {
                if let Some(c) = cur.take() {
                    ds.conversations.push(c);
                }
                let label = rest.trim();
                cur = Some(Conversation {
                    label: (!label.is_empty()).then(|| label.to_string()),
                    turns: Vec::new(),
                });
                continue;
            }

            // speaker: text  → a turn
            if let Some((speaker, text)) = trimmed.split_once(':') {
                let conv = cur.get_or_insert_with(Conversation::default);
                conv.turns.push(Turn {
                    speaker: speaker.trim().to_string(),
                    text: text.trim().to_string(),
                });
                continue;
            }

            // Continuation of the previous turn (indented wrap line).
            if let Some(conv) = cur.as_mut() {
                if let Some(last) = conv.turns.last_mut() {
                    last.text.push(' ');
                    last.text.push_str(trimmed);
                }
            }
        }
        if let Some(c) = cur {
            ds.conversations.push(c);
        }
        ds
    }

    /// Look up a metadata value by key.
    pub fn meta(&self, key: &str) -> Option<&str> {
        self.meta.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// Total number of turns across all conversations.
    pub fn turn_count(&self) -> usize {
        self.conversations.iter().map(|c| c.turns.len()).sum()
    }

    /// Every turn's text, flat — the simplest training signal.
    pub fn utterances(&self) -> Vec<String> {
        self.conversations
            .iter()
            .flat_map(|c| c.turns.iter().map(|t| t.text.clone()))
            .collect()
    }

    /// One training string per conversation, turns joined by ` <eos> ` so the
    /// model learns to continue *across* turns, not just within a line.
    pub fn training_lines(&self) -> Vec<String> {
        self.conversations
            .iter()
            .map(|c| {
                c.turns
                    .iter()
                    .map(|t| t.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" <eos> ")
            })
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Serialize back to `.lingdialog` text (round-trips [`Dataset::parse`]).
    pub fn to_lingdialog(&self) -> String {
        let mut out = String::from("# Ling Dialog Standard v1\n");
        for (k, v) in &self.meta {
            out.push_str(&format!("@meta {k}: {v}\n"));
        }
        for conv in &self.conversations {
            out.push('\n');
            match &conv.label {
                Some(l) => out.push_str(&format!("= {l}\n")),
                None => out.push_str("=\n"),
            }
            for t in &conv.turns {
                out.push_str(&format!("{}: {}\n", t.speaker, t.text));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# tavern
@meta title: Tavern
@meta lang: en

= greeting
npc:  Welcome, traveler!
hero: Any rumors?
npc:  They say the mine
      woke something up.

=
npc: Buy something or leave.
";

    #[test]
    fn parses_meta_and_conversations() {
        let ds = Dataset::parse(SAMPLE);
        assert_eq!(ds.meta("title"), Some("Tavern"));
        assert_eq!(ds.meta("lang"), Some("en"));
        assert_eq!(ds.conversations.len(), 2);
        assert_eq!(ds.conversations[0].label.as_deref(), Some("greeting"));
        assert_eq!(ds.turn_count(), 4);
    }

    #[test]
    fn continuation_lines_merge() {
        let ds = Dataset::parse(SAMPLE);
        let third = &ds.conversations[0].turns[2];
        assert_eq!(third.speaker, "npc");
        assert_eq!(third.text, "They say the mine woke something up.");
    }

    #[test]
    fn training_lines_join_turns() {
        let ds = Dataset::parse(SAMPLE);
        let lines = ds.training_lines();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("<eos>"));
    }

    #[test]
    fn roundtrips() {
        let ds = Dataset::parse(SAMPLE);
        let reparsed = Dataset::parse(&ds.to_lingdialog());
        assert_eq!(ds.conversations, reparsed.conversations);
        assert_eq!(ds.meta, reparsed.meta);
    }
}
