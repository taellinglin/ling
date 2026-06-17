//! Chat prompt formatting utilities.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: Role::System, content: content.into() }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: Role::User, content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: Role::Assistant, content: content.into() }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChatHistory {
    pub messages: Vec<Message>,
}

impl ChatHistory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, msg: Message) {
        self.messages.push(msg);
    }

    /// Format as a simple `<role>: content\n` transcript.
    pub fn to_plain(&self) -> String {
        self.messages
            .iter()
            .map(|m| format!("{}: {}", m.role.as_str(), m.content))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Format as ChatML (used by many open-weight models).
    pub fn to_chatml(&self) -> String {
        self.messages
            .iter()
            .map(|m| format!("<|im_start|>{}\n{}<|im_end|>", m.role.as_str(), m.content))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
