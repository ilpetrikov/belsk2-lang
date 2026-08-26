#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenType {
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
}

impl TokenType {
    pub fn name(&self) -> &'static str {
        match self {
            TokenType::Eof => "EOF",
            TokenType::Ident => "IDENT",
            TokenType::String => "STRING",
            TokenType::Number => "NUMBER",
            TokenType::LParen => "(",
            TokenType::RParen => ")",
            TokenType::LBrace => "{",
            TokenType::RBrace => "}",
            TokenType::LBracket => "[",
            TokenType::RBracket => "]",
            TokenType::Semicolon => ";",
            TokenType::Comma => ",",
            TokenType::Dot => ".",
            TokenType::Colon => ":",
            TokenType::Eq => "=",
            TokenType::EqEq => "==",
            TokenType::Neq => "!=",
            TokenType::Lt => "<",
            TokenType::Gt => ">",
            TokenType::Lte => "<=",
            TokenType::Gte => ">=",
            TokenType::Plus => "+",
            TokenType::Minus => "-",
            TokenType::Star => "*",
            TokenType::Slash => "/",
            TokenType::Percent => "%",
            TokenType::PlusEq => "+=",
            TokenType::MinusEq => "-=",
            TokenType::And => "&&",
            TokenType::Or => "||",
            TokenType::Not => "!",
            TokenType::Arrow => "->",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tt: TokenType,
    pub value: String,
    pub line: usize,
    pub col: usize,
}

impl Token {
    pub fn new(tt: TokenType, value: &str, line: usize, col: usize) -> Self {
        Token {
            tt,
            value: value.to_string(),
            line,
            col,
        }
    }

    pub fn eof() -> Self {
        Token {
            tt: TokenType::Eof,
            value: String::new(),
            line: 0,
            col: 0,
        }
    }
}
