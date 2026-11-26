//! Closed expression grammar of spec §4.3: a hand-written tokenizer and recursive-descent parser
//! producing an [`Expr`] AST, plus evaluation against a [`Resolver`].
//!
//! ```text
//! expr    = term , { ("+" | "-") , term } ;
//! term    = unary , { ("*" | "/") , unary } ;
//! unary   = [ "-" ] , primary ;
//! primary = number | field_ref | param | ("min" | "max") , "(" , expr , { "," , expr } , ")"
//!         | "(" , expr , ")" ;
//! ```
//!
//! Numbers have no exponent and no leading dot, identifiers are `[a-z][a-z0-9_]{0,31}`, `min`
//! and `max` are reserved function names (1–8 arguments), `id:field` is a field reference (no
//! whitespace around the colon) and everything else is a parse error carrying the character
//! position. Whitespace is ASCII space, tab, CR and LF. The parser never panics and nests at
//! most [`MAX_DEPTH`] levels deep; no general-purpose expression engine is involved.

use std::fmt;

use crate::model::ids::{FieldRef, Id};

/// Minimum expression length in characters (§4.2).
pub const MIN_EXPR_LEN: usize = 1;
/// Maximum expression length in characters (§4.2).
pub const MAX_EXPR_LEN: usize = 160;
/// Maximum identifier length.
pub const MAX_IDENT_LEN: usize = 32;
/// Maximum number of `min`/`max` arguments.
pub const MAX_CALL_ARGS: usize = 8;
/// Maximum parenthesis / call nesting accepted by the parser (a guard against stack overflow;
/// the 160-character limit already caps legal nesting below this).
pub const MAX_DEPTH: usize = 100;

/// Binary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
}

impl BinOp {
    fn symbol(self) -> char {
        match self {
            BinOp::Add => '+',
            BinOp::Sub => '-',
            BinOp::Mul => '*',
            BinOp::Div => '/',
        }
    }
}

/// The two reserved functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Func {
    /// `min(a, b, ...)`
    Min,
    /// `max(a, b, ...)`
    Max,
}

impl Func {
    fn name(self) -> &'static str {
        match self {
            Func::Min => "min",
            Func::Max => "max",
        }
    }
}

/// Expression AST.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// A non-negative decimal literal.
    Num(f64),
    /// A bare parameter name.
    Param(Id),
    /// An `id:field` reference.
    Field(FieldRef),
    /// Unary minus.
    Neg(Box<Expr>),
    /// Binary operation (left-associative in source).
    Bin(BinOp, Box<Expr>, Box<Expr>),
    /// `min(...)` / `max(...)` with 1–8 arguments.
    Call(Func, Vec<Expr>),
}

/// Why an expression failed to parse. Messages never echo the input text (§9.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseErrorKind {
    /// Length outside 1–160.
    Length,
    /// A character outside the grammar.
    InvalidChar,
    /// An identifier longer than 32 characters.
    IdentTooLong,
    /// `ident:` not followed by an identifier.
    BadFieldRef,
    /// A number literal that is not finite.
    BadNumber,
    /// A token the grammar does not allow here (includes trailing tokens).
    UnexpectedToken,
    /// The expression ended too early.
    UnexpectedEnd,
    /// `min`/`max` not followed by `(`.
    ExpectedCall,
    /// More than 8 arguments to `min`/`max`.
    TooManyArgs,
    /// Nesting deeper than [`MAX_DEPTH`].
    TooDeep,
}

/// A parse error with the 0-based character position it was detected at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError {
    /// 0-based character offset (the expression length for [`ParseErrorKind::UnexpectedEnd`]).
    pub position: usize,
    /// Error class.
    pub kind: ParseErrorKind,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self.kind {
            ParseErrorKind::Length => "expression length must be 1 to 160 characters",
            ParseErrorKind::InvalidChar => "character not allowed in an expression",
            ParseErrorKind::IdentTooLong => "identifier longer than 32 characters",
            ParseErrorKind::BadFieldRef => "malformed id:field reference",
            ParseErrorKind::BadNumber => "number is not finite",
            ParseErrorKind::UnexpectedToken => "unexpected token",
            ParseErrorKind::UnexpectedEnd => "unexpected end of expression",
            ParseErrorKind::ExpectedCall => "min and max must be followed by an argument list",
            ParseErrorKind::TooManyArgs => "min and max take at most 8 arguments",
            ParseErrorKind::TooDeep => "expression nested too deeply",
        };
        write!(f, "{what} at position {}", self.position)
    }
}

impl std::error::Error for ParseError {}

/// Why evaluation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalError {
    /// A bare name that is not a parameter (`binding_unknown_name`).
    UnknownParam(Id),
    /// A field reference the resolver could not read (`unknown_field` / `dangling_reference`).
    UnknownField(FieldRef),
    /// Division by zero (`binding_eval_error`).
    DivisionByZero,
    /// An intermediate or final result is NaN or infinite (`binding_eval_error`).
    NonFinite,
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EvalError::UnknownParam(p) => write!(f, "unknown parameter {p}"),
            EvalError::UnknownField(r) => write!(f, "cannot read field {r}"),
            EvalError::DivisionByZero => f.write_str("division by zero"),
            EvalError::NonFinite => f.write_str("result is not a finite number"),
        }
    }
}

impl std::error::Error for EvalError {}

/// Supplies parameter and field values to [`Expr::eval`].
pub trait Resolver {
    /// Value of the parameter `name`, if it exists.
    fn param(&self, name: &str) -> Option<f64>;
    /// Current value of the field `r`, if it exists and is readable.
    fn field(&self, r: &FieldRef) -> Option<f64>;
}

/// A pair of closures `(params, fields)` is a [`Resolver`].
impl<P, F> Resolver for (P, F)
where
    P: Fn(&str) -> Option<f64>,
    F: Fn(&FieldRef) -> Option<f64>,
{
    fn param(&self, name: &str) -> Option<f64> {
        (self.0)(name)
    }
    fn field(&self, r: &FieldRef) -> Option<f64> {
        (self.1)(r)
    }
}

// ---- tokenizer ------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Field(String, String),
    Op(char),
}

struct Token {
    tok: Tok,
    pos: usize,
}

fn err(position: usize, kind: ParseErrorKind) -> ParseError {
    ParseError { position, kind }
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_lowercase()
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'
}

/// Reads a maximal identifier starting at `i` (which must be an ident start).
fn read_ident(chars: &[char], i: usize) -> Result<(String, usize), ParseError> {
    let mut j = i;
    while j < chars.len() && is_ident_char(chars[j]) {
        j += 1;
    }
    if j - i > MAX_IDENT_LEN {
        return Err(err(i, ParseErrorKind::IdentTooLong));
    }
    Ok((chars[i..j].iter().collect(), j))
}

fn tokenize(chars: &[char]) -> Result<Vec<Token>, ParseError> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\r' | '\n' => i += 1,
            '+' | '-' | '*' | '/' | '(' | ')' | ',' => {
                out.push(Token {
                    tok: Tok::Op(c),
                    pos: i,
                });
                i += 1;
            }
            '0'..='9' => {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                if i + 1 < chars.len() && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
                    i += 1;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                let text: String = chars[start..i].iter().collect();
                match text.parse::<f64>() {
                    Ok(v) if v.is_finite() => out.push(Token {
                        tok: Tok::Num(v),
                        pos: start,
                    }),
                    _ => return Err(err(start, ParseErrorKind::BadNumber)),
                }
            }
            c if is_ident_start(c) => {
                let start = i;
                let (first, next) = read_ident(chars, i)?;
                i = next;
                if i < chars.len() && chars[i] == ':' {
                    let second_start = i + 1;
                    if second_start >= chars.len() || !is_ident_start(chars[second_start]) {
                        return Err(err(i, ParseErrorKind::BadFieldRef));
                    }
                    let (second, next) = read_ident(chars, second_start)?;
                    i = next;
                    out.push(Token {
                        tok: Tok::Field(first, second),
                        pos: start,
                    });
                } else {
                    out.push(Token {
                        tok: Tok::Ident(first),
                        pos: start,
                    });
                }
            }
            _ => return Err(err(i, ParseErrorKind::InvalidChar)),
        }
    }
    Ok(out)
}

// ---- parser ---------------------------------------------------------------------------------

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    end: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.toks.get(self.pos)
    }

    fn peek_op(&self) -> Option<char> {
        match self.peek() {
            Some(Token {
                tok: Tok::Op(c), ..
            }) => Some(*c),
            _ => None,
        }
    }

    fn here(&self) -> usize {
        self.peek().map_or(self.end, |t| t.pos)
    }

    fn unexpected(&self) -> ParseError {
        match self.peek() {
            Some(t) => err(t.pos, ParseErrorKind::UnexpectedToken),
            None => err(self.end, ParseErrorKind::UnexpectedEnd),
        }
    }

    fn expect_op(&mut self, op: char) -> Result<(), ParseError> {
        if self.peek_op() == Some(op) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.unexpected())
        }
    }

    fn expr(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.term()?;
        while let Some(c @ ('+' | '-')) = self.peek_op() {
            self.pos += 1;
            let rhs = self.term()?;
            let op = if c == '+' { BinOp::Add } else { BinOp::Sub };
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn term(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.unary()?;
        while let Some(c @ ('*' | '/')) = self.peek_op() {
            self.pos += 1;
            let rhs = self.unary()?;
            let op = if c == '*' { BinOp::Mul } else { BinOp::Div };
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr, ParseError> {
        if self.peek_op() == Some('-') {
            self.pos += 1;
            Ok(Expr::Neg(Box::new(self.primary()?)))
        } else {
            self.primary()
        }
    }

    fn nested(&mut self) -> Result<Expr, ParseError> {
        if self.depth >= MAX_DEPTH {
            return Err(err(self.here(), ParseErrorKind::TooDeep));
        }
        self.depth += 1;
        let r = self.expr();
        self.depth -= 1;
        r
    }

    fn primary(&mut self) -> Result<Expr, ParseError> {
        let Some(token) = self.peek() else {
            return Err(err(self.end, ParseErrorKind::UnexpectedEnd));
        };
        let pos = token.pos;
        match token.tok.clone() {
            Tok::Num(v) => {
                self.pos += 1;
                Ok(Expr::Num(v))
            }
            Tok::Field(a, b) => {
                self.pos += 1;
                FieldRef::new(format!("{a}:{b}"))
                    .map(Expr::Field)
                    .map_err(|_| err(pos, ParseErrorKind::BadFieldRef))
            }
            Tok::Ident(name) if name == "min" || name == "max" => {
                self.pos += 1;
                let func = if name == "min" { Func::Min } else { Func::Max };
                if self.peek_op() != Some('(') {
                    return Err(err(self.here(), ParseErrorKind::ExpectedCall));
                }
                self.pos += 1;
                let mut args = vec![self.nested()?];
                while self.peek_op() == Some(',') {
                    self.pos += 1;
                    if args.len() == MAX_CALL_ARGS {
                        return Err(err(self.here(), ParseErrorKind::TooManyArgs));
                    }
                    args.push(self.nested()?);
                }
                self.expect_op(')')?;
                Ok(Expr::Call(func, args))
            }
            Tok::Ident(name) => {
                self.pos += 1;
                Id::new(name)
                    .map(Expr::Param)
                    .map_err(|_| err(pos, ParseErrorKind::InvalidChar))
            }
            Tok::Op('(') => {
                self.pos += 1;
                let inner = self.nested()?;
                self.expect_op(')')?;
                Ok(inner)
            }
            Tok::Op(_) => Err(err(pos, ParseErrorKind::UnexpectedToken)),
        }
    }
}

impl Expr {
    /// Parses `text` according to §4.3 (length 1–160 characters). Never panics.
    pub fn parse(text: &str) -> Result<Expr, ParseError> {
        let chars: Vec<char> = text.chars().collect();
        if !(MIN_EXPR_LEN..=MAX_EXPR_LEN).contains(&chars.len()) {
            return Err(err(0, ParseErrorKind::Length));
        }
        let toks = tokenize(&chars)?;
        let mut p = Parser {
            toks,
            pos: 0,
            end: chars.len(),
            depth: 0,
        };
        let e = p.expr()?;
        if p.pos != p.toks.len() {
            return Err(p.unexpected());
        }
        Ok(e)
    }

    /// Field references used, in order of first appearance, without duplicates.
    pub fn refs(&self) -> Vec<FieldRef> {
        let mut out = Vec::new();
        self.walk(&mut |e| {
            if let Expr::Field(r) = e {
                if !out.contains(r) {
                    out.push(r.clone());
                }
            }
        });
        out
    }

    /// Parameter names used, in order of first appearance, without duplicates.
    pub fn params(&self) -> Vec<Id> {
        let mut out = Vec::new();
        self.walk(&mut |e| {
            if let Expr::Param(p) = e {
                if !out.contains(p) {
                    out.push(p.clone());
                }
            }
        });
        out
    }

    fn walk(&self, f: &mut dyn FnMut(&Expr)) {
        f(self);
        match self {
            Expr::Neg(a) => a.walk(f),
            Expr::Bin(_, a, b) => {
                a.walk(f);
                b.walk(f);
            }
            Expr::Call(_, args) => args.iter().for_each(|a| a.walk(f)),
            Expr::Num(_) | Expr::Param(_) | Expr::Field(_) => {}
        }
    }

    /// Evaluates the expression. Division by zero and any non-finite intermediate result are
    /// errors. Recursion depth equals AST depth (bounded by [`MAX_DEPTH`] for parsed trees).
    pub fn eval(&self, env: &dyn Resolver) -> Result<f64, EvalError> {
        fn finite(v: f64) -> Result<f64, EvalError> {
            if v.is_finite() {
                Ok(v)
            } else {
                Err(EvalError::NonFinite)
            }
        }
        match self {
            Expr::Num(v) => finite(*v),
            Expr::Param(p) => finite(
                env.param(p.as_str())
                    .ok_or_else(|| EvalError::UnknownParam(p.clone()))?,
            ),
            Expr::Field(r) => finite(
                env.field(r)
                    .ok_or_else(|| EvalError::UnknownField(r.clone()))?,
            ),
            Expr::Neg(a) => Ok(-a.eval(env)?),
            Expr::Bin(op, a, b) => {
                let (x, y) = (a.eval(env)?, b.eval(env)?);
                match op {
                    BinOp::Add => finite(x + y),
                    BinOp::Sub => finite(x - y),
                    BinOp::Mul => finite(x * y),
                    BinOp::Div if y == 0.0 => Err(EvalError::DivisionByZero),
                    BinOp::Div => finite(x / y),
                }
            }
            Expr::Call(func, args) => {
                let mut acc: Option<f64> = None;
                for a in args {
                    let v = a.eval(env)?;
                    acc = Some(match (acc, func) {
                        (None, _) => v,
                        (Some(m), Func::Min) => m.min(v),
                        (Some(m), Func::Max) => m.max(v),
                    });
                }
                // An empty call cannot come from the parser; hand-built trees get an error.
                acc.ok_or(EvalError::NonFinite)
            }
        }
    }

    fn is_atom(&self) -> bool {
        matches!(
            self,
            Expr::Num(_) | Expr::Param(_) | Expr::Field(_) | Expr::Call(..)
        )
    }
}

/// Prints a fully parenthesised form that [`Expr::parse`] maps back to an equal AST.
impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Num(v) => write!(f, "{v}"),
            Expr::Param(p) => write!(f, "{p}"),
            Expr::Field(r) => write!(f, "{r}"),
            Expr::Neg(a) if a.is_atom() => write!(f, "-{a}"),
            Expr::Neg(a) => write!(f, "-({a})"),
            Expr::Bin(op, a, b) => write!(f, "({a} {} {b})", op.symbol()),
            Expr::Call(func, args) => {
                write!(f, "{}(", func.name())?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{a}")?;
                }
                f.write_str(")")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> impl Resolver {
        (
            |n: &str| match n {
                "len" => Some(120.0),
                "wid" => Some(60.0),
                "margin" => Some(10.0),
                "huge" => Some(1e200),
                _ => None,
            },
            |r: &FieldRef| (r.as_str() == "k_d1:u").then_some(-50.0),
        )
    }

    fn run(text: &str) -> Result<f64, String> {
        let e = Expr::parse(text).map_err(|e| format!("parse: {e}"))?;
        e.eval(&env()).map_err(|e| format!("eval: {e}"))
    }

    #[test]
    fn accepts_closed_grammar() {
        for (text, expected) in [
            ("margin - len / 2", -50.0),
            ("len / 2 - margin", 50.0),
            ("-margin + 2 * (len - wid)", 110.0),
            ("max(margin, wid / 4, 3) - min(len, 1.5)", 13.5),
            ("k_d1:u * 2", -100.0),
            ("  min ( 1 ,2 )\t", 1.0),
            ("2 * -3", -6.0),
        ] {
            assert_eq!(run(text), Ok(expected), "{text}");
        }
    }

    #[test]
    fn rejects_everything_else() {
        for text in [
            "__import__('os')",
            "len ** 2",
            "len % 7",
            "len.real",
            "1e3",
            ".5",
            "1.",
            "--len",
            "+len",
            "len wid",
            "min",
            "max()",
            "(len",
            "len)",
            "abs(len)",
            "k_d1 : u",
            "k_d1:",
            "k_d1:1",
            "Len",
            "len[0]",
            "len = 1",
            "\"a\"",
            "\u{3000}len",
            "",
            "min(1,2,3,4,5,6,7,8,9)",
        ] {
            assert!(
                matches!(run(text), Err(e) if e.starts_with("parse")),
                "{text:?}"
            );
        }
        assert!(Expr::parse(&"x".repeat(161)).is_err());
        assert!(Expr::parse(&"x".repeat(33)).is_err());
        assert!(Expr::parse(&"x".repeat(32)).is_ok());
        assert!(Expr::parse("min(1,2,3,4,5,6,7,8)").is_ok());
        for text in [
            "unknown_param + 1",
            "len / 0",
            "len / (wid - 60)",
            "nope:x + 1",
        ] {
            assert!(
                matches!(run(text), Err(e) if e.starts_with("eval")),
                "{text}"
            );
        }
    }

    #[test]
    fn error_positions() {
        assert_eq!(
            Expr::parse("1 + $").unwrap_err(),
            err(4, ParseErrorKind::InvalidChar)
        );
        assert_eq!(
            Expr::parse("1 +").unwrap_err(),
            err(3, ParseErrorKind::UnexpectedEnd)
        );
        assert_eq!(
            Expr::parse("min + 1").unwrap_err(),
            err(4, ParseErrorKind::ExpectedCall)
        );
        assert_eq!(
            Expr::parse("1 2").unwrap_err(),
            err(2, ParseErrorKind::UnexpectedToken)
        );
    }

    #[test]
    fn non_finite_results_are_errors() {
        assert_eq!(
            run("huge * huge"),
            Err("eval: result is not a finite number".into())
        );
        assert_eq!(run("len / 0"), Err("eval: division by zero".into()));
        assert_eq!(run("nope + 1"), Err("eval: unknown parameter nope".into()));
    }

    #[test]
    fn deep_nesting_does_not_overflow() {
        let text = format!("{}1{}", "(".repeat(79), ")".repeat(79));
        assert_eq!(run(&text), Ok(1.0));
        let deep = format!("{}1{}", "(".repeat(1000), ")".repeat(1000));
        assert_eq!(Expr::parse(&deep).unwrap_err().kind, ParseErrorKind::Length);
        let calls = "min(".repeat(30) + "1" + &")".repeat(30);
        assert_eq!(run(&calls), Ok(1.0));
        // the depth guard itself (unreachable through `parse` because of the length cap)
        let chars: Vec<char> = "(".repeat(MAX_DEPTH + 2).chars().collect();
        let mut p = Parser {
            toks: tokenize(&chars).unwrap(),
            pos: 0,
            end: chars.len(),
            depth: 0,
        };
        assert_eq!(p.expr().unwrap_err().kind, ParseErrorKind::TooDeep);
    }

    #[test]
    fn refs_and_params() {
        let e = Expr::parse("a + b * a - v:x / min(w:y, v:x, c)").unwrap();
        let refs: Vec<String> = e.refs().iter().map(|r| r.to_string()).collect();
        assert_eq!(refs, ["v:x", "w:y"]);
        let ps: Vec<String> = e.params().iter().map(|r| r.to_string()).collect();
        assert_eq!(ps, ["a", "b", "c"]);
    }

    #[test]
    fn display_round_trips() {
        for text in [
            "-(1 + 2) * x:y",
            "1 - 2 - 3",
            "1 - (2 - 3)",
            "-min(1, 2)",
            "2 * -3",
        ] {
            let e = Expr::parse(text).unwrap();
            assert_eq!(Expr::parse(&e.to_string()).unwrap(), e, "{text}");
        }
        let neg_neg = Expr::Neg(Box::new(Expr::Neg(Box::new(Expr::Num(1.0)))));
        assert_eq!(Expr::parse(&neg_neg.to_string()).unwrap(), neg_neg);
    }
}
