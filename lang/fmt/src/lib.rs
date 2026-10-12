//! Formatter for Belsk2 code (`belsk2 fmt`).
//!
//! Works on tokens rather than the syntax tree, so comments and the blank
//! lines between things survive. Settings come from `belsk2fmt.toml`
//! (see [`Config`]). The result is checked: it must contain exactly the
//! same tokens as the input, otherwise formatting fails instead of
//! changing the program.
//!
//! ```
//! let out = belsk2_fmt::format("fn main(){var x=1+2;prinb(x);}", &Default::default()).unwrap();
//! assert_eq!(out, "fn main() {\n    var x = 1 + 2;\n    prinb(x);\n}\n");
//! ```

mod config;

pub use config::{BraceStyle, Config, IndentStyle, LineEnding, CONFIG_FILES};

use belsk2_syntax::token::TokenKind as K;
use belsk2_syntax::{tokenize_raw, Error, RawKind, RawToken};

const KEYWORDS: &[&str] = &[
    "using",
    "var",
    "fn",
    "if",
    "else",
    "while",
    "for",
    "in",
    "return",
    "break",
    "continue",
    "idb",
    "struct",
    "enum",
    "class",
    "public",
    "private",
    "protected",
    "static",
    "interface",
    "virtual",
    "override",
    "abstract",
    "base",
    "is",
    "as",
    "partial",
];

/// Keywords after which an expression starts.
const EXPR_KEYWORDS: &[&str] = &["return", "in", "if", "while", "idb"];

#[derive(Debug, Clone)]
struct Tok {
    kind: RawKind,
    text: String,
    line: usize,
    col: usize,
    end_line: usize,
    end_col: usize,
    /// `<` or `>` of type arguments (`List<int>`).
    generic: bool,
    /// The second `>` of `>>` or `>>=`: written without a space.
    joined: bool,
    /// A prefix operator (`-x`, `!done`).
    unary: bool,
    /// A `{` (or its `}`) that starts a dictionary literal, not a block.
    literal: bool,
    /// The `{` (or `}`) of the arms of a `match`.
    arms: bool,
    /// The `:` before base types (`class Dog : Animal`) or `base(...)`,
    /// written with spaces around it.
    spaced: bool,
}

impl Tok {
    fn is(&self, k: K) -> bool {
        self.kind == RawKind::Token(k)
    }

    fn is_comment(&self) -> bool {
        matches!(self.kind, RawKind::LineComment | RawKind::BlockComment)
    }

    fn keyword(&self) -> bool {
        self.is(K::Ident) && KEYWORDS.contains(&self.text.as_str())
    }

    fn name(&self) -> bool {
        self.is(K::Ident) && !self.keyword()
    }

    /// Whether an expression may start right after this token.
    fn expects_expr(&self) -> bool {
        match self.kind {
            RawKind::Token(k) => {
                is_binary(k)
                    || matches!(
                        k,
                        K::LParen
                            | K::LBracket
                            | K::LBrace
                            | K::Comma
                            | K::Colon
                            | K::Semicolon
                            | K::Not
                            | K::Tilde
                    )
                    || (k == K::Ident && EXPR_KEYWORDS.contains(&self.text.as_str()))
            }
            _ => true,
        }
    }
}

fn is_binary(k: K) -> bool {
    matches!(
        k,
        K::Eq
            | K::EqEq
            | K::Neq
            | K::Lt
            | K::Gt
            | K::Lte
            | K::Gte
            | K::Plus
            | K::Minus
            | K::Star
            | K::Slash
            | K::Percent
            | K::PlusEq
            | K::MinusEq
            | K::And
            | K::Or
            | K::Arrow
            | K::Amp
            | K::Pipe
            | K::Caret
            | K::Shl
            | K::StarEq
            | K::SlashEq
            | K::PercentEq
            | K::AmpEq
            | K::PipeEq
            | K::CaretEq
            | K::ShlEq
    )
}

fn tokens(source: &str) -> Result<Vec<Tok>, Error> {
    let raw: Vec<RawToken> = tokenize_raw(source)?;
    let mut toks: Vec<Tok> = raw
        .into_iter()
        .map(|t| {
            let len = t.text.chars().count();
            Tok {
                end_col: if t.end_line == t.line { t.col + len } else { 0 },
                kind: t.kind,
                text: t.text,
                line: t.line,
                col: t.col,
                end_line: t.end_line,
                generic: false,
                joined: false,
                unary: false,
                literal: false,
                spaced: false,
                arms: false,
            }
        })
        .collect();

    // Indices of real tokens (comments skipped) for looking around.
    let code: Vec<usize> = (0..toks.len())
        .filter(|&i| toks.get(i).is_some_and(|t| !t.is_comment()))
        .collect();
    let at = |toks: &Vec<Tok>, n: usize| code.get(n).and_then(|&i| toks.get(i)).cloned();

    for n in 0..code.len() {
        let Some(t) = at(&toks, n) else { continue };
        let prev = n.checked_sub(1).and_then(|p| at(&toks, p));
        let Some(&i) = code.get(n) else { continue };

        // `name<` with no space before `<` and only type-like tokens up to
        // the matching `>`: type arguments.
        if t.is(K::Lt)
            && !t.generic
            && prev
                .as_ref()
                .is_some_and(|p| p.name() && p.end_line == t.line && p.end_col == t.col)
        {
            let mut depth = 0usize;
            let mut end = None;
            for m in n..code.len() {
                let Some(u) = at(&toks, m) else { break };
                if u.is(K::Lt) {
                    depth += 1;
                } else if u.is(K::Gt) {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        end = Some(m);
                        break;
                    }
                } else if !(u.is(K::Ident)
                    || u.is(K::Comma)
                    || u.is(K::LBracket)
                    || u.is(K::RBracket))
                {
                    break;
                }
            }
            if let Some(end) = end {
                for m in n..=end {
                    if let Some(&j) = code.get(m) {
                        if let Some(u) = toks.get_mut(j) {
                            if u.is(K::Lt) || u.is(K::Gt) {
                                u.generic = true;
                            }
                        }
                    }
                }
            }
        }

        let Some(t) = at(&toks, n) else { continue };
        let joined = !t.generic
            && (t.is(K::Gt) || t.is(K::Gte))
            && prev.as_ref().is_some_and(|p| {
                p.is(K::Gt) && !p.generic && p.end_line == t.line && p.end_col == t.col
            });
        let unary = ((t.is(K::Minus) || t.is(K::Plus) || t.is(K::Not) || t.is(K::Tilde))
            && prev
                .as_ref()
                .is_none_or(|p| p.expects_expr() && !p.is(K::RBrace)))
            // `~Name()`, a destructor.
            || (t.is(K::Tilde) && prev.as_ref().is_some_and(|p| p.is(K::RBrace)));
        let literal = t.is(K::LBrace)
            && prev.as_ref().is_some_and(|p| {
                p.expects_expr() && !p.is(K::Semicolon) && !(p.is(K::LBrace) && !p.literal)
            });
        if let Some(u) = toks.get_mut(i) {
            u.joined = joined;
            u.unary = unary;
            u.literal = literal;
        }
    }

    // The `{` after `match subject` holds arms, one per line.
    let mut after_match = false;
    let mut depth = 0usize;
    let mut arm_braces: Vec<bool> = Vec::new();
    for t in toks.iter_mut().filter(|t| !t.is_comment()) {
        if t.is(K::Ident) && t.text == "match" {
            after_match = true;
            depth = 0;
        } else if t.is(K::LParen) || t.is(K::LBracket) {
            depth += 1;
        } else if t.is(K::RParen) || t.is(K::RBracket) {
            depth = depth.saturating_sub(1);
        } else if t.is(K::Semicolon) {
            after_match = false;
        }
        if t.is(K::LBrace) {
            let arms = after_match && depth == 0 && !t.literal;
            t.arms = arms;
            arm_braces.push(arms);
            if arms {
                after_match = false;
            }
        } else if t.is(K::RBrace) {
            t.arms = arm_braces.pop().unwrap_or(false);
        }
    }

    // `class Dog : Animal`, `Dog(...) : base(...)`.
    let code: Vec<usize> = (0..toks.len())
        .filter(|&i| toks.get(i).is_some_and(|t| !t.is_comment()))
        .collect();
    for (k, &i) in code.iter().enumerate() {
        if !toks.get(i).is_some_and(|t| t.is(K::Colon)) {
            continue;
        }
        let word = |d: isize| {
            k.checked_add_signed(d)
                .and_then(|k| code.get(k))
                .and_then(|&j| toks.get(j))
                .map(|t| t.text.clone())
                .unwrap_or_default()
        };
        let spaced = matches!(word(-2).as_str(), "struct" | "class" | "enum" | "interface")
            || word(1) == "base";
        if let Some(t) = toks.get_mut(i) {
            t.spaced = spaced;
        }
    }

    // Mark the `}` that closes each literal `{`.
    let mut stack: Vec<bool> = Vec::new();
    for t in toks.iter_mut() {
        if t.is(K::LBrace) {
            stack.push(t.literal);
        } else if t.is(K::RBrace) {
            t.literal = stack.pop().unwrap_or(false);
        }
    }
    Ok(toks)
}

/// The accessors of an auto-property starting at `toks[start]` (just
/// after its `{`): `{ get; private set; }` and the index of the `}`.
fn accessors(toks: &[Tok], start: usize) -> Option<(String, usize)> {
    let mut text = String::from("{");
    let mut has_get = false;
    for (i, t) in toks.iter().enumerate().skip(start) {
        if t.is(K::RBrace) {
            return has_get.then(|| (text + " }", i));
        }
        if t.is(K::Semicolon) {
            text.push(';');
            continue;
        }
        let word = t.is(K::Ident)
            && matches!(
                t.text.as_str(),
                "get" | "set" | "public" | "private" | "protected"
            );
        if !word {
            return None;
        }
        has_get |= t.text == "get";
        text.push(' ');
        text.push_str(&t.text);
    }
    None
}

/// Whether to write a space between `p` and `t` on the same line.
fn space(p: &Tok, t: &Tok, config: &Config) -> bool {
    let original = p.end_line != t.line || p.end_col < t.col;
    if t.joined || p.unary {
        return false;
    }
    if p.is_comment() || t.is_comment() {
        return true;
    }
    if t.is(K::Colon) && t.spaced {
        return true;
    }
    if t.is(K::Comma)
        || t.is(K::Semicolon)
        || t.is(K::RParen)
        || t.is(K::RBracket)
        || t.is(K::Dot)
        || t.is(K::Colon)
    {
        return false;
    }
    if p.is(K::LParen) || p.is(K::LBracket) || p.is(K::Dot) {
        return false;
    }
    if (p.is(K::Lt) && p.generic) || (t.is(K::Lt) && t.generic) || (t.is(K::Gt) && t.generic) {
        return false;
    }
    let value_end = p.name()
        || (p.is(K::Ident) && matches!(p.text.as_str(), "base" | "this"))
        || p.is(K::RParen)
        || p.is(K::RBracket)
        || (p.is(K::Gt) && p.generic);
    if t.is(K::LParen) {
        return !value_end;
    }
    if t.is(K::LBracket) {
        return !(value_end || p.is(K::String));
    }
    if t.is(K::LBrace) {
        return true;
    }
    if p.is(K::LBrace) && t.is(K::RBrace) {
        return false;
    }
    let operator = |x: &Tok| match x.kind {
        RawKind::Token(k) => is_binary(k) && !x.generic && !x.unary,
        _ => false,
    };
    if operator(p) || operator(t) {
        return if config.space_around_operators {
            true
        } else {
            original
        };
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Open {
    Block,
    /// The arms of a `match`: each starts on a new line.
    Arms,
    Bracket,
}

struct Writer<'a> {
    config: &'a Config,
    lines: Vec<String>,
    line: String,
    /// Open blocks and brackets; a bracket counts for indentation once a
    /// line break happened inside it.
    stack: Vec<(Open, bool)>,
    /// A statement continues on the next line.
    continued: bool,
}

impl Writer<'_> {
    fn level(&self) -> usize {
        let open: usize = self
            .stack
            .iter()
            .map(|(o, broken)| usize::from(*o != Open::Bracket || *broken))
            .sum();
        open + usize::from(self.continued)
    }

    fn end_line(&mut self) {
        let line = std::mem::take(&mut self.line);
        self.lines.push(line.trim_end().to_string());
    }

    /// Starts a new line with `blank` empty lines before it, indented for
    /// `t` (a closing bracket at the start of a line goes one level out).
    fn new_line(&mut self, blank: usize, closing: bool) {
        // A closing bracket goes one level out if its bracket was indented.
        let closing = closing
            && self
                .stack
                .last()
                .is_some_and(|(o, broken)| *o == Open::Bracket && *broken);
        if !self.line.trim().is_empty() {
            self.end_line();
        } else {
            self.line.clear();
        }
        for _ in 0..blank {
            // Never two blank lines in a row, never at the very start.
            if self.lines.last().is_some_and(|l| !l.is_empty()) {
                self.lines.push(String::new());
            }
        }
        let mut level = self.level();
        if closing {
            level = level.saturating_sub(1);
        }
        self.line = self.config.indent(level);
    }

    fn at_line_start(&self) -> bool {
        self.line.trim().is_empty()
    }

    fn push(&mut self, text: &str, space: bool) {
        if space && !self.at_line_start() {
            self.line.push(' ');
        }
        self.line.push_str(text);
    }
}

/// Formats Belsk2 source code.
pub fn format(source: &str, config: &Config) -> Result<String, Error> {
    let toks = tokens(source)?;
    let mut w = Writer {
        config,
        lines: Vec::new(),
        line: String::new(),
        stack: Vec::new(),
        continued: false,
    };
    // A line break is due before the next token (after `;`, `{`, `}`, a
    // line comment).
    let mut pending = false;
    let mut prev: Option<&Tok> = None;
    let mut i = 0;
    while let Some(t) = toks.get(i) {
        i += 1;
        let next = toks.get(i);
        let mut gap = prev.map(|p| t.line.saturating_sub(p.end_line)).unwrap_or(0);
        // `} else` with braces on the same line: always together.
        let else_after_brace = t.is(K::Ident)
            && t.text == "else"
            && prev.is_some_and(|p| p.is(K::RBrace) && !p.literal);
        if else_after_brace && config.brace_style == BraceStyle::SameLine {
            gap = 0;
        }
        let blank = gap.saturating_sub(1).min(config.max_blank_lines);
        let block_open = t.is(K::LBrace) && !t.literal;
        let block_close = t.is(K::RBrace) && !t.literal;
        let closing = t.is(K::RParen) || t.is(K::RBracket) || t.is(K::RBrace);

        // A comment on the same line as the previous token stays there.
        if t.is_comment() && gap == 0 && prev.is_some() {
            w.push(t.text.trim_end(), true);
            pending |= t.kind == RawKind::LineComment;
            prev = Some(t);
            continue;
        }

        let after_open = prev.is_some_and(|p| p.is(K::LBrace) && !p.literal);
        let blank = if after_open || block_close { 0 } else { blank };

        if block_close {
            // Close the block (and anything left open inside it).
            while let Some((open, _)) = w.stack.pop() {
                if open != Open::Bracket {
                    break;
                }
            }
            w.continued = false;
            w.new_line(0, false);
            w.push("}", false);
            // `};` after a `match` value, `},` after an arm, `})`.
            pending =
                !next.is_some_and(|n| n.is(K::Semicolon) || n.is(K::Comma) || n.is(K::RParen));
            // `} else` stays together (unless braces go on their own line).
            if next.is_some_and(|n| n.is(K::Ident) && n.text == "else")
                && config.brace_style == BraceStyle::SameLine
            {
                pending = false;
            }
            prev = Some(t);
            continue;
        }

        if pending {
            if let Some((Open::Bracket, broken)) = w.stack.last_mut() {
                *broken = true;
            }
            w.new_line(blank, closing);
            pending = false;
        } else if prev.is_some() && gap > 0 && !t.is(K::Semicolon) && !t.is(K::Comma) && !block_open
        {
            // Keep the author's line breaks inside a statement.
            match w.stack.last_mut() {
                Some((Open::Bracket, broken)) => *broken = true,
                _ => w.continued = true,
            }
            w.new_line(blank, closing);
        } else if let Some(p) = prev {
            if block_open && config.brace_style == BraceStyle::NextLine && !w.at_line_start() {
                w.continued = false;
                w.new_line(0, false);
            } else {
                let sp = space(p, t, config);
                w.push("", sp);
            }
        }

        if block_open {
            w.continued = false;
            // `{}` stays as is.
            if next.is_some_and(|n| n.is(K::RBrace)) {
                w.push("{}", false);
                i += 1;
                pending = true;
                prev = next;
                if toks
                    .get(i)
                    .is_some_and(|n| n.is(K::Ident) && n.text == "else")
                    && config.brace_style == BraceStyle::SameLine
                {
                    pending = false;
                }
                continue;
            }
            // An auto-property keeps `{ get; private set; }` on one line.
            if let Some((text, end)) = accessors(&toks, i) {
                w.push(&text, false);
                i = end + 1;
                prev = toks.get(end);
                pending = !toks.get(i).is_some_and(|n| n.is(K::Eq));
                continue;
            }
            w.push("{", false);
            let open = if t.arms { Open::Arms } else { Open::Block };
            w.stack.push((open, false));
            pending = true;
            prev = Some(t);
            continue;
        }

        let comment_own_line = t.is_comment() && w.line.trim().is_empty();
        if t.is(K::LParen) || t.is(K::LBracket) || (t.is(K::LBrace) && t.literal) {
            w.push(&t.text, false);
            w.stack.push((Open::Bracket, false));
        } else if closing {
            w.push(&t.text, false);
            w.stack.pop();
        } else {
            w.push(t.text.trim_end(), false);
        }

        if t.is(K::Semicolon) && !w.stack.last().is_some_and(|(o, _)| *o == Open::Bracket) {
            w.continued = false;
            pending = true;
        }
        if t.is(K::Comma) && w.stack.last().is_some_and(|(o, _)| *o == Open::Arms) {
            w.continued = false;
            pending = true;
        }
        // A comment on its own line keeps the next token on a new line.
        if t.kind == RawKind::LineComment || (t.is_comment() && comment_own_line) {
            pending = true;
        }
        prev = Some(t);
    }
    if !w.line.trim().is_empty() {
        w.end_line();
    }
    while w.lines.last().is_some_and(|l| l.is_empty()) {
        w.lines.pop();
    }

    let crlf = match config.line_ending {
        LineEnding::Crlf => true,
        LineEnding::Lf => false,
        LineEnding::Auto => source.contains("\r\n"),
    };
    let mut out = w.lines.join(if crlf { "\r\n" } else { "\n" });
    if config.trailing_newline && !out.is_empty() {
        out.push_str(if crlf { "\r\n" } else { "\n" });
    }

    // Safety net: the program must not change.
    let code = |s: &str| -> Result<Vec<(RawKind, String)>, Error> {
        Ok(tokenize_raw(s)?
            .into_iter()
            .map(|t| (t.kind, t.text.trim_end().to_string()))
            .collect())
    };
    if code(source)? != code(&out)? {
        return Err(Error::project(
            "the formatter would change the program; the file was left as it is (please report this)",
        ));
    }
    Ok(out)
}

/// Whether `source` is already formatted.
pub fn is_formatted(source: &str, config: &Config) -> Result<bool, Error> {
    Ok(format(source, config)? == source)
}
