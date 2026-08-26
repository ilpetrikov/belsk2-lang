package interpreter

import (
	"fmt"
	"os"
	"strings"
	"unicode"
)

type Lexer struct {
	input string
	pos   int
	line  int
	col   int
}

func NewLexer(input string) *Lexer {
	return &Lexer{input: input, line: 1, col: 1}
}

func (l *Lexer) peek() byte {
	if l.pos >= len(l.input) {
		return 0
	}
	return l.input[l.pos]
}

func (l *Lexer) peekAt(offset int) byte {
	p := l.pos + offset
	if p >= len(l.input) {
		return 0
	}
	return l.input[p]
}

func (l *Lexer) advance() byte {
	ch := l.input[l.pos]
	l.pos++
	if ch == '\n' {
		l.line++
		l.col = 1
	} else {
		l.col++
	}
	return ch
}

func (l *Lexer) skipWhitespaceAndComments() {
	for l.pos < len(l.input) {
		ch := l.input[l.pos]
		if ch == ' ' || ch == '\t' || ch == '\r' || ch == '\n' {
			l.advance()
		} else if ch == '/' && l.peekAt(1) == '/' {
			for l.pos < len(l.input) && l.input[l.pos] != '\n' {
				l.advance()
			}
		} else if ch == '/' && l.peekAt(1) == '*' {
			l.advance()
			l.advance()
			for l.pos+1 < len(l.input) {
				if l.input[l.pos] == '*' && l.input[l.pos+1] == '/' {
					l.advance()
					l.advance()
					break
				}
				l.advance()
			}
		} else {
			break
		}
	}
}

func (l *Lexer) readString(quote byte) string {
	var sb strings.Builder
	l.advance()
	for l.pos < len(l.input) {
		ch := l.input[l.pos]
		if ch == quote {
			break
		}
		if ch == '\\' && l.pos+1 < len(l.input) {
			l.advance()
			esc := l.advance()
			switch esc {
			case 'n':
				sb.WriteByte('\n')
			case 't':
				sb.WriteByte('\t')
			case '\\':
				sb.WriteByte('\\')
			case '"':
				sb.WriteByte('"')
			case '\'':
				sb.WriteByte('\'')
			case '0':
				sb.WriteByte(0)
			default:
				sb.WriteByte('\\')
				sb.WriteByte(esc)
			}
		} else {
			sb.WriteByte(l.advance())
		}
	}
	if l.pos < len(l.input) {
		l.advance()
	}
	return sb.String()
}

func (l *Lexer) readNumber() string {
	start := l.pos
	for l.pos < len(l.input) && l.input[l.pos] >= '0' && l.input[l.pos] <= '9' {
		l.advance()
	}
	if l.pos < len(l.input) && l.input[l.pos] == '.' {
		next := l.peekAt(1)
		if next >= '0' && next <= '9' {
			l.advance()
			for l.pos < len(l.input) && l.input[l.pos] >= '0' && l.input[l.pos] <= '9' {
				l.advance()
			}
		}
	}
	return l.input[start:l.pos]
}

func (l *Lexer) readIdent() string {
	start := l.pos
	for l.pos < len(l.input) {
		ch := rune(l.input[l.pos])
		if unicode.IsLetter(ch) || unicode.IsDigit(ch) || ch == '_' {
			l.advance()
		} else {
			break
		}
	}
	return l.input[start:l.pos]
}

func (l *Lexer) Tokenize() []Token {
	var tokens []Token
	for {
		l.skipWhitespaceAndComments()
		if l.pos >= len(l.input) {
			tokens = append(tokens, Token{TOKEN_EOF, "", l.line, l.col})
			break
		}
		ch := l.peek()
		line, col := l.line, l.col

		switch {
		case ch == '"' || ch == '\'':
			tokens = append(tokens, Token{TOKEN_STRING, l.readString(ch), line, col})
		case ch >= '0' && ch <= '9':
			tokens = append(tokens, Token{TOKEN_NUMBER, l.readNumber(), line, col})
		case unicode.IsLetter(rune(ch)) || ch == '_':
			tokens = append(tokens, Token{TOKEN_IDENT, l.readIdent(), line, col})
		case ch == '(':
			l.advance()
			tokens = append(tokens, Token{TOKEN_LPAREN, "(", line, col})
		case ch == ')':
			l.advance()
			tokens = append(tokens, Token{TOKEN_RPAREN, ")", line, col})
		case ch == '{':
			l.advance()
			tokens = append(tokens, Token{TOKEN_LBRACE, "{", line, col})
		case ch == '}':
			l.advance()
			tokens = append(tokens, Token{TOKEN_RBRACE, "}", line, col})
		case ch == '[':
			l.advance()
			tokens = append(tokens, Token{TOKEN_LBRACKET, "[", line, col})
		case ch == ']':
			l.advance()
			tokens = append(tokens, Token{TOKEN_RBRACKET, "]", line, col})
		case ch == ';':
			l.advance()
			tokens = append(tokens, Token{TOKEN_SEMICOLON, ";", line, col})
		case ch == ',':
			l.advance()
			tokens = append(tokens, Token{TOKEN_COMMA, ",", line, col})
		case ch == ':':
			l.advance()
			tokens = append(tokens, Token{TOKEN_COLON, ":", line, col})
		case ch == '.':
			l.advance()
			tokens = append(tokens, Token{TOKEN_DOT, ".", line, col})
		case ch == '-' && l.peekAt(1) == '>':
			l.advance()
			l.advance()
			tokens = append(tokens, Token{TOKEN_ARROW, "->", line, col})
		case ch == '&' && l.peekAt(1) == '&':
			l.advance()
			l.advance()
			tokens = append(tokens, Token{TOKEN_AND, "&&", line, col})
		case ch == '|' && l.peekAt(1) == '|':
			l.advance()
			l.advance()
			tokens = append(tokens, Token{TOKEN_OR, "||", line, col})
		case ch == '!' && l.peekAt(1) == '=':
			l.advance()
			l.advance()
			tokens = append(tokens, Token{TOKEN_NEQ, "!=", line, col})
		case ch == '!':
			l.advance()
			tokens = append(tokens, Token{TOKEN_NOT, "!", line, col})
		case ch == '+' && l.peekAt(1) == '=':
			l.advance()
			l.advance()
			tokens = append(tokens, Token{TOKEN_PLUS_EQ, "+=", line, col})
		case ch == '-' && l.peekAt(1) == '=':
			l.advance()
			l.advance()
			tokens = append(tokens, Token{TOKEN_MINUS_EQ, "-=", line, col})
		case ch == '=' && l.peekAt(1) == '=':
			l.advance()
			l.advance()
			tokens = append(tokens, Token{TOKEN_EQEQ, "==", line, col})
		case ch == '<' && l.peekAt(1) == '=':
			l.advance()
			l.advance()
			tokens = append(tokens, Token{TOKEN_LTE, "<=", line, col})
		case ch == '>' && l.peekAt(1) == '=':
			l.advance()
			l.advance()
			tokens = append(tokens, Token{TOKEN_GTE, ">=", line, col})
		case ch == '=':
			l.advance()
			tokens = append(tokens, Token{TOKEN_EQ, "=", line, col})
		case ch == '+':
			l.advance()
			tokens = append(tokens, Token{TOKEN_PLUS, "+", line, col})
		case ch == '-':
			l.advance()
			tokens = append(tokens, Token{TOKEN_MINUS, "-", line, col})
		case ch == '*':
			l.advance()
			tokens = append(tokens, Token{TOKEN_STAR, "*", line, col})
		case ch == '/':
			l.advance()
			tokens = append(tokens, Token{TOKEN_SLASH, "/", line, col})
		case ch == '%':
			l.advance()
			tokens = append(tokens, Token{TOKEN_PERCENT, "%", line, col})
		case ch == '<':
			l.advance()
			tokens = append(tokens, Token{TOKEN_LT, "<", line, col})
		case ch == '>':
			l.advance()
			tokens = append(tokens, Token{TOKEN_GT, ">", line, col})
		default:
			fmt.Fprintf(os.Stderr, "line %d:%d: unexpected character '%c'\n", line, col, ch)
			l.advance()
		}
	}
	return tokens
}
