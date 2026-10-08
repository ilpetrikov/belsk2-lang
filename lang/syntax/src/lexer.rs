use crate::error::{Error, Result};
use crate::span::Span;
use crate::token::{Token, TokenKind};

pub struct Lexer {
    input: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
}

/// Splits source code into tokens. The last token is always [`TokenKind::Eof`].
pub fn tokenize(source: &str) -> Result<Vec<Token>> {
    Lexer::new(source).tokenize()
}

impl Lexer {
    pub fn new(source: &str) -> Self {
        Lexer {
            input: source.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    fn peek(&self) -> Option<char> {
        self.input.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.input.get(self.pos + offset).copied()
    }

    fn span(&self) -> Span {
        Span::new(self.line, self.col)
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += 1;
        if ch == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(ch)
    }

    fn skip_trivia(&mut self) -> Result<()> {
        loop {
            match (self.peek(), self.peek_at(1)) {
                (Some(' ' | '\t' | '\r' | '\n' | '\u{feff}'), _) => {
                    self.bump();
                }
                (Some('/'), Some('/')) => {
                    while self.peek().is_some_and(|c| c != '\n') {
                        self.bump();
                    }
                }
                (Some('/'), Some('*')) => {
                    let start = self.span();
                    self.bump();
                    self.bump();
                    loop {
                        match (self.peek(), self.peek_at(1)) {
                            (Some('*'), Some('/')) => {
                                self.bump();
                                self.bump();
                                break;
                            }
                            (Some(_), _) => {
                                self.bump();
                            }
                            (None, _) => {
                                let mut e = Error::syntax("unterminated block comment", start);
                                e.incomplete = true;
                                return Err(e);
                            }
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn read_string(&mut self, quote: char) -> Result<String> {
        let start = self.span();
        self.bump();
        let mut out = String::new();
        loop {
            let Some(ch) = self.bump() else {
                let mut e = Error::syntax("unterminated string literal", start);
                e.incomplete = true;
                return Err(e);
            };
            if ch == quote {
                return Ok(out);
            }
            if ch != '\\' {
                out.push(ch);
                continue;
            }
            let esc_span = self.span();
            match self.bump() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('\'') => out.push('\''),
                Some('0') => out.push('\0'),
                Some(other) => {
                    return Err(Error::syntax(
                        format!("unknown escape sequence '\\{other}'"),
                        esc_span,
                    ))
                }
                None => {
                    let mut e = Error::syntax("unterminated string literal", start);
                    e.incomplete = true;
                    return Err(e);
                }
            }
        }
    }

    fn read_while(&mut self, pred: impl Fn(char) -> bool) -> String {
        let mut out = String::new();
        while let Some(c) = self.peek().filter(|&c| pred(c)) {
            out.push(c);
            self.bump();
        }
        out
    }

    /// Reads a number literal as written: `42`, `1_000`, `0xFF`, `0b1010`,
    /// `3.14`, `1e-9`, with an optional suffix (`10L`, `2.5f`, ...). The
    /// parser interprets the text.
    fn read_number(&mut self) -> String {
        let digit_or_sep = |c: char| c.is_ascii_digit() || c == '_';
        if self.peek() == Some('0') && matches!(self.peek_at(1), Some('x' | 'X' | 'b' | 'B')) {
            let mut text = String::new();
            text.extend(self.bump());
            text.extend(self.bump());
            text.push_str(&self.read_while(|c| c.is_ascii_alphanumeric() || c == '_'));
            return text;
        }
        let mut text = self.read_while(digit_or_sep);
        if self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            text.extend(self.bump());
            text.push_str(&self.read_while(digit_or_sep));
        }
        let exp_digit_at = match self.peek_at(1) {
            Some('+' | '-') => 2,
            _ => 1,
        };
        if matches!(self.peek(), Some('e' | 'E'))
            && self
                .peek_at(exp_digit_at)
                .is_some_and(|c| c.is_ascii_digit())
        {
            for _ in 0..exp_digit_at {
                text.extend(self.bump());
            }
            text.push_str(&self.read_while(digit_or_sep));
        }
        // Suffix (validated by the parser).
        text.push_str(&self.read_while(|c| c.is_alphanumeric() || c == '_'));
        text
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>> {
        let mut tokens = Vec::new();
        loop {
            self.skip_trivia()?;
            let span = self.span();
            let Some(ch) = self.peek() else {
                tokens.push(Token::new(TokenKind::Eof, "", span));
                return Ok(tokens);
            };

            if ch == '"' || ch == '\'' {
                let s = self.read_string(ch)?;
                tokens.push(Token::new(TokenKind::String, s, span));
                continue;
            }
            if ch.is_ascii_digit() {
                let n = self.read_number();
                tokens.push(Token::new(TokenKind::Number, n, span));
                continue;
            }
            if ch.is_alphabetic() || ch == '_' {
                let id = self.read_while(|c| c.is_alphanumeric() || c == '_');
                tokens.push(Token::new(TokenKind::Ident, id, span));
                continue;
            }

            let next = self.peek_at(1);
            let (kind, len) = match (ch, next) {
                ('-', Some('>')) => (TokenKind::Arrow, 2),
                ('&', Some('&')) => (TokenKind::And, 2),
                ('|', Some('|')) => (TokenKind::Or, 2),
                ('!', Some('=')) => (TokenKind::Neq, 2),
                ('+', Some('=')) => (TokenKind::PlusEq, 2),
                ('-', Some('=')) => (TokenKind::MinusEq, 2),
                ('=', Some('=')) => (TokenKind::EqEq, 2),
                ('<', Some('=')) => (TokenKind::Lte, 2),
                ('>', Some('=')) => (TokenKind::Gte, 2),
                ('<', Some('<')) if self.peek_at(2) == Some('=') => (TokenKind::ShlEq, 3),
                ('<', Some('<')) => (TokenKind::Shl, 2),
                ('*', Some('=')) => (TokenKind::StarEq, 2),
                ('/', Some('=')) => (TokenKind::SlashEq, 2),
                ('%', Some('=')) => (TokenKind::PercentEq, 2),
                ('&', Some('=')) => (TokenKind::AmpEq, 2),
                ('|', Some('=')) => (TokenKind::PipeEq, 2),
                ('^', Some('=')) => (TokenKind::CaretEq, 2),
                ('(', _) => (TokenKind::LParen, 1),
                (')', _) => (TokenKind::RParen, 1),
                ('{', _) => (TokenKind::LBrace, 1),
                ('}', _) => (TokenKind::RBrace, 1),
                ('[', _) => (TokenKind::LBracket, 1),
                (']', _) => (TokenKind::RBracket, 1),
                (';', _) => (TokenKind::Semicolon, 1),
                (',', _) => (TokenKind::Comma, 1),
                (':', _) => (TokenKind::Colon, 1),
                ('.', _) => (TokenKind::Dot, 1),
                ('!', _) => (TokenKind::Not, 1),
                ('=', _) => (TokenKind::Eq, 1),
                ('+', _) => (TokenKind::Plus, 1),
                ('-', _) => (TokenKind::Minus, 1),
                ('*', _) => (TokenKind::Star, 1),
                ('/', _) => (TokenKind::Slash, 1),
                ('%', _) => (TokenKind::Percent, 1),
                ('<', _) => (TokenKind::Lt, 1),
                ('>', _) => (TokenKind::Gt, 1),
                ('&', _) => (TokenKind::Amp, 1),
                ('|', _) => (TokenKind::Pipe, 1),
                ('^', _) => (TokenKind::Caret, 1),
                ('~', _) => (TokenKind::Tilde, 1),
                _ => {
                    return Err(Error::syntax(
                        format!("unexpected character '{}'", ch.escape_debug()),
                        span,
                    ))
                }
            };
            for _ in 0..len {
                self.bump();
            }
            tokens.push(Token::new(kind, "", span));
        }
    }
}
