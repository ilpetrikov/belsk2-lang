use crate::token::{Token, TokenType};

pub struct Lexer {
    input: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
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

    fn peek(&self) -> char {
        if self.pos >= self.input.len() {
            '\0'
        } else {
            self.input[self.pos]
        }
    }

    fn peek_at(&self, offset: usize) -> char {
        let p = self.pos + offset;
        if p >= self.input.len() {
            '\0'
        } else {
            self.input[p]
        }
    }

    fn advance(&mut self) -> char {
        let ch = self.input[self.pos];
        self.pos += 1;
        if ch == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        ch
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            if self.pos >= self.input.len() {
                break;
            }
            let ch = self.peek();
            if ch == ' ' || ch == '\t' || ch == '\r' || ch == '\n' {
                self.advance();
            } else if ch == '/' && self.peek_at(1) == '/' {
                while self.pos < self.input.len() && self.peek() != '\n' {
                    self.advance();
                }
            } else if ch == '/' && self.peek_at(1) == '*' {
                self.advance();
                self.advance();
                while self.pos + 1 < self.input.len() {
                    if self.peek() == '*' && self.peek_at(1) == '/' {
                        self.advance();
                        self.advance();
                        break;
                    }
                    self.advance();
                }
            } else {
                break;
            }
        }
    }

    fn read_string(&mut self, quote: char) -> String {
        let mut sb = String::new();
        self.advance();
        while self.pos < self.input.len() {
            let ch = self.peek();
            if ch == quote {
                break;
            }
            if ch == '\\' && self.pos + 1 < self.input.len() {
                self.advance();
                let esc = self.advance();
                match esc {
                    'n' => sb.push('\n'),
                    't' => sb.push('\t'),
                    '\\' => sb.push('\\'),
                    '"' => sb.push('"'),
                    '\'' => sb.push('\''),
                    '0' => sb.push('\0'),
                    _ => {
                        sb.push('\\');
                        sb.push(esc);
                    }
                }
            } else {
                sb.push(self.advance());
            }
        }
        if self.pos < self.input.len() {
            self.advance();
        }
        sb
    }

    fn read_number(&mut self) -> String {
        let start = self.pos;
        while self.pos < self.input.len() && self.peek() >= '0' && self.peek() <= '9' {
            self.advance();
        }
        if self.pos < self.input.len() && self.peek() == '.' {
            let next = self.peek_at(1);
            if next >= '0' && next <= '9' {
                self.advance();
                while self.pos < self.input.len() && self.peek() >= '0' && self.peek() <= '9' {
                    self.advance();
                }
            }
        }
        self.input[start..self.pos].iter().collect::<String>()
    }

    fn read_ident(&mut self) -> String {
        let start = self.pos;
        while self.pos < self.input.len() {
            let ch = self.peek();
            if ch.is_alphanumeric() || ch == '_' {
                self.advance();
            } else {
                break;
            }
        }
        self.input[start..self.pos].iter().collect()
    }

    pub fn tokenize(&mut self) -> Vec<Token> {
        let mut tokens = Vec::new();
        loop {
            self.skip_whitespace_and_comments();
            if self.pos >= self.input.len() {
                tokens.push(Token::new(TokenType::Eof, "", self.line, self.col));
                break;
            }
            let ch = self.peek();
            let line = self.line;
            let col = self.col;

            if ch == '"' || ch == '\'' {
                tokens.push(Token::new(
                    TokenType::String,
                    &self.read_string(ch),
                    line,
                    col,
                ));
            } else if ch >= '0' && ch <= '9' {
                tokens.push(Token::new(
                    TokenType::Number,
                    &self.read_number(),
                    line,
                    col,
                ));
            } else if ch.is_alphabetic() || ch == '_' {
                tokens.push(Token::new(TokenType::Ident, &self.read_ident(), line, col));
            } else if ch == '(' {
                self.advance();
                tokens.push(Token::new(TokenType::LParen, "(", line, col));
            } else if ch == ')' {
                self.advance();
                tokens.push(Token::new(TokenType::RParen, ")", line, col));
            } else if ch == '{' {
                self.advance();
                tokens.push(Token::new(TokenType::LBrace, "{", line, col));
            } else if ch == '}' {
                self.advance();
                tokens.push(Token::new(TokenType::RBrace, "}", line, col));
            } else if ch == '[' {
                self.advance();
                tokens.push(Token::new(TokenType::LBracket, "[", line, col));
            } else if ch == ']' {
                self.advance();
                tokens.push(Token::new(TokenType::RBracket, "]", line, col));
            } else if ch == ';' {
                self.advance();
                tokens.push(Token::new(TokenType::Semicolon, ";", line, col));
            } else if ch == ',' {
                self.advance();
                tokens.push(Token::new(TokenType::Comma, ",", line, col));
            } else if ch == ':' {
                self.advance();
                tokens.push(Token::new(TokenType::Colon, ":", line, col));
            } else if ch == '.' {
                self.advance();
                tokens.push(Token::new(TokenType::Dot, ".", line, col));
            } else if ch == '-' && self.peek_at(1) == '>' {
                self.advance();
                self.advance();
                tokens.push(Token::new(TokenType::Arrow, "->", line, col));
            } else if ch == '&' && self.peek_at(1) == '&' {
                self.advance();
                self.advance();
                tokens.push(Token::new(TokenType::And, "&&", line, col));
            } else if ch == '|' && self.peek_at(1) == '|' {
                self.advance();
                self.advance();
                tokens.push(Token::new(TokenType::Or, "||", line, col));
            } else if ch == '!' && self.peek_at(1) == '=' {
                self.advance();
                self.advance();
                tokens.push(Token::new(TokenType::Neq, "!=", line, col));
            } else if ch == '!' {
                self.advance();
                tokens.push(Token::new(TokenType::Not, "!", line, col));
            } else if ch == '+' && self.peek_at(1) == '=' {
                self.advance();
                self.advance();
                tokens.push(Token::new(TokenType::PlusEq, "+=", line, col));
            } else if ch == '-' && self.peek_at(1) == '=' {
                self.advance();
                self.advance();
                tokens.push(Token::new(TokenType::MinusEq, "-=", line, col));
            } else if ch == '=' && self.peek_at(1) == '=' {
                self.advance();
                self.advance();
                tokens.push(Token::new(TokenType::EqEq, "==", line, col));
            } else if ch == '<' && self.peek_at(1) == '=' {
                self.advance();
                self.advance();
                tokens.push(Token::new(TokenType::Lte, "<=", line, col));
            } else if ch == '>' && self.peek_at(1) == '=' {
                self.advance();
                self.advance();
                tokens.push(Token::new(TokenType::Gte, ">=", line, col));
            } else if ch == '=' {
                self.advance();
                tokens.push(Token::new(TokenType::Eq, "=", line, col));
            } else if ch == '+' {
                self.advance();
                tokens.push(Token::new(TokenType::Plus, "+", line, col));
            } else if ch == '-' {
                self.advance();
                tokens.push(Token::new(TokenType::Minus, "-", line, col));
            } else if ch == '*' {
                self.advance();
                tokens.push(Token::new(TokenType::Star, "*", line, col));
            } else if ch == '/' {
                self.advance();
                tokens.push(Token::new(TokenType::Slash, "/", line, col));
            } else if ch == '%' {
                self.advance();
                tokens.push(Token::new(TokenType::Percent, "%", line, col));
            } else if ch == '<' {
                self.advance();
                tokens.push(Token::new(TokenType::Lt, "<", line, col));
            } else if ch == '>' {
                self.advance();
                tokens.push(Token::new(TokenType::Gt, ">", line, col));
            } else {
                eprintln!("line {}:{}: unexpected character '{}'", line, col, ch);
                self.advance();
            }
        }
        tokens
    }
}
