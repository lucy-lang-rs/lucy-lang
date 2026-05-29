// LL(1) lexer — now emits (Token, Span) pairs

#![allow(unused, non_camel_case_types)]

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::span::Span;

#[derive(Clone, Debug, PartialEq)]
pub enum Token<'a> {
    STRING(&'a str),

    INT(i32),
    FLOAT(f64),

    BINOP(&'a str),
    UNARY(&'a str),
    PAREN(&'a str),
    AND,
    BANG,

    DECLARE,
    CLASS,
    TUPLE,
    CATEGORY,
    MACRO,
    IDENT(&'a str),

    FN,
    RETURN,
    PUNCT(&'a str),
    ARROW,
    LEFTARROW,
    FATARROW,
    END,

    IF,
    ELSE,
    ELSEIF,
    IN,

    WHILE,
    FOR,
    DO,

    PUB,
    MODULE,
    USE,

    STATIC,
    DYNAMIC,

    MATCH,
    THROW,
    AS,

    MUTABLE,

    SELF,
    SELFTYPE,

    DEFAULT,

    OPERATOR,
    TYPEOF,

    FSTRING_START,
    FSTRING_CHUNK(&'a str),
    FSTRING_EXPR_START,
    FSTRING_EXPR_END,
    FSTRING_END,

    MACRO_REP_START,

    MACRO_VAR(&'a str)
}

static KEYWORDS: OnceLock<HashMap<&'static str, Token<'static>>> = OnceLock::new();

fn keywords() -> &'static HashMap<&'static str, Token<'static>> {
    KEYWORDS.get_or_init(|| {
        let mut m = HashMap::new();
        m.insert("let",    Token::DECLARE);
        m.insert("func", Token::FN);
        m.insert("return",   Token::RETURN);
        m.insert("class",    Token::CLASS);
        m.insert("if",       Token::IF);
        m.insert("else",     Token::ELSE);
        m.insert("elseif",   Token::ELSEIF);
        m.insert("while",    Token::WHILE);
        m.insert("for",      Token::FOR);
        m.insert("do",       Token::DO);
        m.insert("end",      Token::END);
        m.insert("in",       Token::IN);
        m.insert("pub",   Token::PUB);
        m.insert("module",   Token::MODULE);
        m.insert("dynamic",  Token::DYNAMIC);
        m.insert("static",   Token::STATIC);
        m.insert("use",      Token::USE);
        m.insert("match",    Token::MATCH);
        m.insert("mut",      Token::MUTABLE);
        m.insert("operator", Token::OPERATOR);
        m.insert("as",       Token::AS);
        m.insert("self",     Token::SELF);
        m.insert("Self",     Token::SELFTYPE);
        m.insert("typeof",   Token::TYPEOF);
        m.insert("throw",    Token::THROW);
        m.insert("default",  Token::DEFAULT);
        m.insert("macro",  Token::MACRO);
        m.insert("tuple",  Token::TUPLE);
        m.insert("category",  Token::CATEGORY);
        m
    })
}
struct Cursor<'src> {
    src:  &'src str,
    pos:  usize,          // byte offset of the *next* char to be consumed
    rest: &'src str,      // src[pos..]
}

impl<'src> Cursor<'src> {
    fn new(src: &'src str) -> Self {
        Self { src, pos: 0, rest: src }
    }

    fn peek_char(&self) -> Option<char> {
        self.rest.chars().next()
    }

    fn peek2_char(&self) -> Option<char> {
        let mut it = self.rest.chars();
        it.next();
        it.next()
    }

    fn next_char(&mut self) -> Option<char> {
        let c = self.rest.chars().next()?;
        let len = c.len_utf8();
        self.pos  += len;
        self.rest  = &self.rest[len..];
        Some(c)
    }

    /// Consume the next char only if it equals `expected`.
    fn eat(&mut self, expected: char) -> bool {
        if self.peek_char() == Some(expected) {
            self.next_char();
            true
        } else {
            false
        }
    }

    fn current_pos(&self) -> usize {
        self.pos
    }

    /// Slice of the source between two byte offsets.
    fn slice(&self, start: usize, end: usize) -> &'src str {
        &self.src[start..end]
    }

    fn span(&self, start: usize) -> Span {
        Span::new(start, self.pos)
    }
}

pub type SpannedToken<'a> = (Token<'a>, Span);

pub fn tokenize(src: String) -> Vec<SpannedToken<'static>> {
    let src: &'static str = Box::leak(src.into_boxed_str());
    tokenize_str(src)
}

fn tokenize_str(src: &'static str) -> Vec<SpannedToken<'static>> {
    let mut tokens: Vec<SpannedToken<'static>> = Vec::new();
    let mut cur = Cursor::new(src);
    let kw = keywords();

    macro_rules! push {
        ($tok:expr, $start:expr) => {
            tokens.push(($tok, cur.span($start)))
        };
    }

    while let Some(c) = cur.next_char() {
        let tok_start = cur.pos - c.len_utf8(); // byte offset *before* c
        let prev = tokens.last().map(|(t, _)| t).cloned();

        if c.is_ascii_whitespace() {
            continue;
        }
        
        if c == '"' || c == '\'' {
            let quote = c;
            let mut lit = String::new();
            while let Some(ch) = cur.next_char() {
                if ch == '\\' {
                    if let Some(esc) = cur.next_char() {
                        lit.push(match esc {
                            'n' => '\n', 't' => '\t', 'r' => '\r',
                            '\\' => '\\', '"' => '"', '\'' => '\'',
                            other => other,
                        });
                    }
                } else if ch == quote {
                    break;
                } else {
                    lit.push(ch);
                }
            }
            let s: &'static str = Box::leak(lit.into_boxed_str());
            push!(Token::STRING(s), tok_start);
            continue;
        }

        // ── identifiers / keywords ──────────────────────────────────────────
        if c.is_alphabetic() || c == '_' {
            let id_start = tok_start;
            let mut ident = String::from(c);
            while matches!(cur.peek_char(), Some(ch) if ch.is_alphanumeric() || ch == '_') {
                ident.push(cur.next_char().unwrap());
            }
            let span = cur.span(id_start);
            if let Some(kw_tok) = kw.get(ident.as_str()) {
                tokens.push((kw_tok.clone(), span));
            } else {
                let s: &'static str = Box::leak(ident.into_boxed_str());
                tokens.push((Token::IDENT(s), span));
            }
            continue;
        }

        // ── numeric literals ─────────────────────────────────────────────────
        if c.is_ascii_digit()
            || (c == '-'
                && matches!(cur.peek_char(), Some(d) if d.is_ascii_digit()))
        {
            let num_start = tok_start;
            let mut num   = String::from(c);
            let mut is_float = false;
            loop {
                match cur.peek_char() {
                    Some(d) if d.is_ascii_digit() => { num.push(cur.next_char().unwrap()); }
                    Some('.') if !is_float && cur.peek2_char().map_or(false, |d| d.is_ascii_digit()) => {
                        is_float = true;
                        num.push(cur.next_char().unwrap());
                    }
                    _ => break,
                }
            }
            let span = cur.span(num_start);
            if is_float {
                tokens.push((Token::FLOAT(num.parse().unwrap()), span));
            } else {
                tokens.push((Token::INT(num.parse().unwrap()), span));
            }
            continue;
        }

        // ── all other characters ─────────────────────────────────────────────
        match c {
            // ── f-strings ───────────────────────────────────────────────────
            '`' => {
                push!(Token::FSTRING_START, tok_start);
                let mut chunk = String::new();
                let mut chunk_start = cur.pos;

                while let Some(ch) = cur.next_char() {
                    match ch {
                        '`' => {
                            if !chunk.is_empty() {
                                let s: &'static str = Box::leak(chunk.into_boxed_str());
                                tokens.push((Token::FSTRING_CHUNK(s), cur.span(chunk_start)));
                                chunk = String::new();
                            }
                            tokens.push((Token::FSTRING_END, cur.span(cur.pos - 1)));
                            break;
                        }
                        '{' => {
                            if !chunk.is_empty() {
                                let s: &'static str = Box::leak(chunk.into_boxed_str());
                                tokens.push((Token::FSTRING_CHUNK(s), cur.span(chunk_start)));
                                chunk = String::new();
                            }
                            let expr_start = cur.pos - 1;
                            tokens.push((Token::FSTRING_EXPR_START, cur.span(expr_start)));

                            let mut depth = 1usize;
                            let mut expr_src = String::new();
                            while let Some(inner) = cur.next_char() {
                                match inner {
                                    '{' => { depth += 1; expr_src.push(inner); }
                                    '}' => {
                                        depth -= 1;
                                        if depth == 0 { break; }
                                        expr_src.push(inner);
                                    }
                                    _ => expr_src.push(inner),
                                }
                            }
                            // Recurse — offsets inside the sub-string are
                            // relative to that string, not the outer source.
                            // If you need absolute offsets here, pass `tok_start`
                            // as a base and shift each span after the call.
                            let inner_tokens = tokenize(expr_src);
                            tokens.extend(inner_tokens);
                            tokens.push((Token::FSTRING_EXPR_END, cur.span(cur.pos - 1)));
                            chunk_start = cur.pos;
                        }
                        '\\' => {
                            if let Some(esc) = cur.next_char() {
                                chunk.push(match esc {
                                    'n' => '\n', 't' => '\t', 'r' => '\r',
                                    '\\' => '\\', '`' => '`',
                                    '{' => '{',   '}' => '}',
                                    other => other,
                                });
                            }
                        }
                        _ => chunk.push(ch),
                    }
                }
            }

            // ── operators & punctuation ──────────────────────────────────────
            '+' => {
                if cur.eat('=') { push!(Token::BINOP("+="), tok_start); }
                else            { push!(Token::BINOP("+"),  tok_start); }
            }

            '-' => {
                // Comments
                if cur.peek_char() == Some('-') {
                    cur.next_char(); // consume second '-'
                    if cur.peek_char() == Some('[') {
                        cur.next_char();
                        if cur.peek_char() == Some('[') {
                            cur.next_char();
                            // block comment: consume until ]]
                            loop {
                                match cur.next_char() {
                                    None => break,
                                    Some(']') if cur.peek_char() == Some(']') => {
                                        cur.next_char(); break;
                                    }
                                    _ => {}
                                }
                            }
                            continue;
                        }
                    }
                    // line comment
                    while cur.next_char().map_or(false, |ch| ch != '\n') {}
                    continue;
                }

                if cur.eat('>') { push!(Token::ARROW,       tok_start); }
                else if cur.eat('=') { push!(Token::BINOP("-="), tok_start); }
                else {
                    let unary = matches!(
                        &prev,
                        None
                        | Some(Token::BINOP(_))
                        | Some(Token::DECLARE)
                        | Some(Token::PAREN("("))
                        | Some(Token::PAREN("{"))
                        | Some(Token::PUNCT(","))
                    );
                    if unary { push!(Token::UNARY("-"), tok_start); }
                    else     { push!(Token::BINOP("-"), tok_start); }
                }
            }

            '!' => {
                if cur.eat('=') { push!(Token::BINOP("!="), tok_start); }
                else {
                    push!(Token::BANG, tok_start);
                }
            }

            '*' => {
                if      cur.eat('=') { push!(Token::BINOP("*="), tok_start); }
                else if cur.eat('>') { push!(Token::BINOP("*>"), tok_start); }
                else                 { push!(Token::BINOP("*"),  tok_start); }
            }
            '/' => {
                if cur.eat('=') { push!(Token::BINOP("/="), tok_start); }
                else            { push!(Token::BINOP("/"),  tok_start); }
            }
            '^' => {
                if cur.eat('=') { push!(Token::BINOP("^="), tok_start); }
                else            { push!(Token::BINOP("^"),  tok_start); }
            }
            '%' => {
                if cur.eat('=') { push!(Token::BINOP("%="), tok_start); }
                else            { push!(Token::BINOP("%"),  tok_start); }
            }

            '.' => {
                if cur.eat('.') { push!(Token::BINOP(".."), tok_start); }
                else            { push!(Token::PUNCT("."),  tok_start); }
            }
            ',' => push!(Token::PUNCT(","), tok_start),
            ':' => {
                if cur.eat(':') { push!(Token::BINOP("::"), tok_start); }
                else            { push!(Token::PUNCT(":"),  tok_start); }
            }
            '$' => {
                if cur.eat('(') {
                    push!(Token::MACRO_REP_START, tok_start);
                } else {
                    // peek at the next char to decide
                    match cur.peek_char() {
                        Some(c) if c.is_alphabetic() || c == '_' => {
                            let id_start = tok_start;
                            let mut ident = String::new();
                            while matches!(cur.peek_char(), Some(ch) if ch.is_alphanumeric() || ch == '_') {
                                ident.push(cur.next_char().unwrap());
                            }
                            let span = cur.span(id_start);
                            let s: &'static str = Box::leak(ident.into_boxed_str());
                            tokens.push((Token::MACRO_VAR(s), span));
                        }
                        _ => { /* bare `$`, ignore or error */ }
                    }
                }
            }
            '=' => {
                if      cur.eat('=') { push!(Token::BINOP("=="), tok_start); }
                else if cur.eat('>') { push!(Token::FATARROW,    tok_start); }
                else                 { push!(Token::BINOP("="),  tok_start); }
            }

            '(' => push!(Token::PAREN("("), tok_start),
            ')' => push!(Token::PAREN(")"), tok_start),
            '{' => push!(Token::PAREN("{"), tok_start),
            '}' => push!(Token::PAREN("}"), tok_start),
            '[' => push!(Token::PAREN("["), tok_start),
            ']' => push!(Token::PAREN("]"), tok_start),

            '<' => {
                if cur.eat('<') {
                    if cur.eat('=') { push!(Token::BINOP("<<="), tok_start); }
                    else            { push!(Token::BINOP("<<"),  tok_start); }
                } else if cur.eat('=') { push!(Token::BINOP("<="),  tok_start); }
                else if cur.eat('-')   { push!(Token::LEFTARROW,    tok_start); }
                else                   { push!(Token::BINOP("<"),   tok_start); }
            }
            '>' => {
                if cur.eat('>') {
                    if cur.eat('=') { push!(Token::BINOP(">>="), tok_start); }
                    else            { push!(Token::BINOP(">>"),  tok_start); }
                } else if cur.eat('=') { push!(Token::BINOP(">="), tok_start); }
                else                   { push!(Token::BINOP(">"),  tok_start); }
            }

            '&' => {
                if cur.eat('&') {
                    if cur.eat('=') { push!(Token::BINOP("&&="), tok_start); }
                    else            { push!(Token::BINOP("&&"),  tok_start); }
                } else if cur.eat('=') { push!(Token::BINOP("&="), tok_start); }
                else                   { push!(Token::AND,         tok_start); }
            }
            '|' => {
                if cur.eat('|') {
                    if cur.eat('=') { push!(Token::BINOP("||="), tok_start); }
                    else            { push!(Token::BINOP("||"),  tok_start); }
                } else if cur.eat('=') { push!(Token::BINOP("|="), tok_start); }
                else                   { push!(Token::BINOP("|"),  tok_start); }
            }

            _ => {} // unknown chars silently skipped, same as original
        }
    }

    tokens
}