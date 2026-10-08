use crate::span::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    Eof,
    Ident,
    String,
    Number,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Semicolon,
    Comma,
    Dot,
    Colon,
    Eq,
    EqEq,
    Neq,
    Lt,
    Gt,
    Lte,
    Gte,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    PlusEq,
    MinusEq,
    And,
    Or,
    Not,
    Arrow,
    Amp,
    Pipe,
    Caret,
    Tilde,
    /// `<<`. There is no `>>` token: the parser joins two adjacent `>` so
    /// that `List<List<int>>` lexes naturally.
    Shl,
    StarEq,
    SlashEq,
    PercentEq,
    AmpEq,
    PipeEq,
    CaretEq,
    ShlEq,
}

impl TokenKind {
    pub fn name(self) -> &'static str {
        match self {
            TokenKind::Eof => "end of input",
            TokenKind::Ident => "identifier",
            TokenKind::String => "string",
            TokenKind::Number => "number",
            TokenKind::LParen => "'('",
            TokenKind::RParen => "')'",
            TokenKind::LBrace => "'{'",
            TokenKind::RBrace => "'}'",
            TokenKind::LBracket => "'['",
            TokenKind::RBracket => "']'",
            TokenKind::Semicolon => "';'",
            TokenKind::Comma => "','",
            TokenKind::Dot => "'.'",
            TokenKind::Colon => "':'",
            TokenKind::Eq => "'='",
            TokenKind::EqEq => "'=='",
            TokenKind::Neq => "'!='",
            TokenKind::Lt => "'<'",
            TokenKind::Gt => "'>'",
            TokenKind::Lte => "'<='",
            TokenKind::Gte => "'>='",
            TokenKind::Plus => "'+'",
            TokenKind::Minus => "'-'",
            TokenKind::Star => "'*'",
            TokenKind::Slash => "'/'",
            TokenKind::Percent => "'%'",
            TokenKind::PlusEq => "'+='",
            TokenKind::MinusEq => "'-='",
            TokenKind::And => "'&&'",
            TokenKind::Or => "'||'",
            TokenKind::Not => "'!'",
            TokenKind::Arrow => "'->'",
            TokenKind::Amp => "'&'",
            TokenKind::Pipe => "'|'",
            TokenKind::Caret => "'^'",
            TokenKind::Tilde => "'~'",
            TokenKind::Shl => "'<<'",
            TokenKind::StarEq => "'*='",
            TokenKind::SlashEq => "'/='",
            TokenKind::PercentEq => "'%='",
            TokenKind::AmpEq => "'&='",
            TokenKind::PipeEq => "'|='",
            TokenKind::CaretEq => "'^='",
            TokenKind::ShlEq => "'<<='",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    /// Source text for identifiers and numbers, the unescaped contents for
    /// strings, empty for punctuation.
    pub text: String,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, text: impl Into<String>, span: Span) -> Self {
        Token {
            kind,
            text: text.into(),
            span,
        }
    }

    /// Human-readable description used in error messages.
    pub fn describe(&self) -> String {
        match self.kind {
            TokenKind::Ident => format!("'{}'", self.text),
            TokenKind::Number => format!("number {}", self.text),
            TokenKind::String => format!("string \"{}\"", self.text),
            k => k.name().to_string(),
        }
    }
}
