//! Arithmetic typed into a numeric field (§9.2).
//!
//! A width field takes `135 * 2` and reads 270 afterwards: one-shot evaluation
//! (§13 decision 1). The result is a number and the sum is not remembered, so
//! nothing here is a step towards the constraint engine that decision forbids.
//!
//! It is a calculator, not a language: four operators, brackets, a sign, and no
//! names. No `parent.width`, no unit conversion, no `50%` (§15 D358); a grammar
//! that grows identifiers would half-reimplement the command bar's Luau
//! (`docs/vm.md`) in the inspector.
//!
//! A unit is noise, not a conversion: `%`, `°`, `px`, `pt` and `em` may trail a
//! value and are dropped, because several fields format their own unit into the
//! digits (`format!("{v:.0}%")`) and the text handed back must still parse.
//!
//! Where a field has a second unit to be confused with, a unit it is not showing
//! is a rejection: [`eval_in`], which every `Length` field uses. `12px` in a `%`
//! field would read as twelve percent, and `1.5em` there means 1.5% because the
//! digits are percent-scaled. None is converted (the unit chip does that), so
//! all are refused and the field keeps its value (§15 D359).
//!
//! Everything egui's default parser accepted still parses: whitespace inside a
//! number is a thousands separator (`1 000`), and U+2212 is a minus.

/// Evaluate a field's text, or `None` if it is not an expression.
///
/// `None` is a `DragValue` parser's "leave the value alone", right for every
/// failure including half-typed input: the string is parsed at every keystroke,
/// so `135 *` must be rejected on its way to `135 * 2`, not read as 135.
/// A non-finite result is a failure too (`1/0` would clamp to the field's maximum).
pub fn eval(text: &str) -> Option<f64> {
    evaluate(text, None)
}

/// The unit a field is showing, for [`eval_in`].
///
/// `Length` is `Px | Em` and an `Em` field writes its digits as a percentage;
/// every other field shows one unit or none, and [`eval`] fits it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Unit {
    /// Digits that are pixels.
    Px,
    /// Digits that are a percentage — `Length::Em` times a hundred.
    Pct,
}

impl Unit {
    /// Whether a value written with `w` belongs in a field showing this.
    ///
    /// `Pct` does not accept `em`: `1.5em` and `150%` are the same length but not
    /// the same digits, and converting is the unit chip's job.
    fn accepts(self, w: Written) -> bool {
        matches!(
            (self, w),
            (Unit::Px, Written::Px) | (Unit::Pct, Written::Pct)
        )
    }
}

/// [`eval`] for a field that is showing one of two units.
///
/// A unit that is not the one on the field is a rejection. Bare digits are
/// always the field's own unit.
pub fn eval_in(text: &str, showing: Unit) -> Option<f64> {
    evaluate(text, Some(showing))
}

/// The body of both, `showing` being `None` for a field with no second unit.
fn evaluate(text: &str, showing: Option<Unit>) -> Option<f64> {
    let tokens = lex(text)?;
    // Before the parse: `atom` drops units, and `12px * 2` must fail as a whole.
    if showing.is_some_and(|s| {
        tokens
            .iter()
            .any(|t| matches!(t, Tok::Unit(w) if !s.accepts(*w)))
    }) {
        return None;
    }
    let mut p = Parser {
        t: &tokens,
        at: 0,
        depth: 0,
    };
    let v = p.expr()?;
    // A token the parse did not reach is a rejection (`(1+2))`, `1)`). It checks
    // tokens only: `1 2` is one token (`Num(12.0)`, the thousands rule) and `atom`
    // drops trailing units, so `2px px` is 2. A test pins all four.
    (p.at == tokens.len() && v.is_finite()).then_some(v)
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tok {
    Num(f64),
    Add,
    Sub,
    Mul,
    Div,
    Open,
    Close,
    /// A unit trailing a value; it carries which one, for [`eval_in`].
    Unit(Written),
}

/// A unit as it was written, so a field showing one can refuse the others.
///
/// `Pt` and `Deg` are accepted by no [`Unit`]; the lexer takes them so [`eval`]
/// keeps ignoring them on geometry fields.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Written {
    Px,
    Pt,
    Em,
    Pct,
    Deg,
}

/// The units a value may trail, lowercased. Any other word is a rejection.
const UNITS: [(&str, Written); 4] = [
    ("px", Written::Px),
    ("pt", Written::Pt),
    ("em", Written::Em),
    ("deg", Written::Deg),
];

fn lex(text: &str) -> Option<Vec<Tok>> {
    let c: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
            continue;
        }
        // `−` (U+2212) is what a formatter writes as a minus (egui folds it too);
        // `×` and `÷` are what a paste produces for the operators.
        let tok = match ch {
            '+' => Tok::Add,
            '-' | '−' => Tok::Sub,
            '*' | '×' => Tok::Mul,
            '/' | '÷' => Tok::Div,
            '(' => Tok::Open,
            ')' => Tok::Close,
            '%' => Tok::Unit(Written::Pct),
            '°' => Tok::Unit(Written::Deg),
            _ => {
                if ch.is_ascii_digit() || ch == '.' {
                    let (v, next) = number(&c, i)?;
                    out.push(Tok::Num(v));
                    i = next;
                    continue;
                }
                if ch.is_alphabetic() {
                    let start = i;
                    while i < c.len() && c[i].is_alphabetic() {
                        i += 1;
                    }
                    let word: String = c[start..i].iter().flat_map(|c| c.to_lowercase()).collect();
                    let (_, written) = *UNITS.iter().find(|(u, _)| *u == word)?;
                    out.push(Tok::Unit(written));
                    continue;
                }
                return None;
            }
        };
        out.push(tok);
        i += 1;
    }
    Some(out)
}

/// Scan one number from `at`, returning it and the index after it.
///
/// Whitespace inside a number is skipped (egui's parser filtered it, so `1 000`
/// is a thousand), but only when a digit follows it: `135 * 2` and `2 px` end
/// the number at the space.
fn number(c: &[char], at: usize) -> Option<(f64, usize)> {
    let mut s = String::new();
    let mut i = at;
    let mut seen_dot = false;
    loop {
        match c.get(i) {
            Some(&d) if d.is_ascii_digit() => s.push(d),
            Some('.') if !seen_dot => {
                seen_dot = true;
                s.push('.');
            }
            Some(&w) if w.is_whitespace() => {
                // Only whitespace left ends the number (`"  135  "`), not a failure.
                let Some(next) = c[i..].iter().position(|c| !c.is_whitespace()) else {
                    break;
                };
                let next = i + next;
                if !c[next].is_ascii_digit() {
                    break;
                }
                i = next;
                continue;
            }
            // An exponent only where digits follow: `2e3` is a number, `2em` a unit.
            Some(&e) if (e == 'e' || e == 'E') && !s.is_empty() => {
                let sign = matches!(c.get(i + 1), Some('+' | '-'));
                let digit = i + 1 + usize::from(sign);
                if !c.get(digit).is_some_and(char::is_ascii_digit) {
                    break;
                }
                s.push('e');
                if sign {
                    s.push(c[i + 1]);
                }
                i = digit;
                s.push(c[digit]);
            }
            _ => break,
        }
        i += 1;
    }
    // A lone `.` fails to parse.
    Some((s.parse().ok()?, i))
}

/// Precedence climbing over the token list — `+ -` under `* /`, brackets, and a
/// leading sign. Every rule returns `None` rather than recovering, so a partial
/// expression cannot evaluate to part of itself.
struct Parser<'a> {
    t: &'a [Tok],
    at: usize,
    /// How many nested rules are open, bounded by [`MAX_DEPTH`].
    depth: u32,
}

/// How deep the grammar may nest before the parser gives up (§15 D669).
///
/// A bound on the stack, not a rule about expressions: a stack overflow is not a
/// panic, nothing unwinds, and neither `OndinApp::on_exit`'s `disk_settle` nor the
/// library writer's `Drop` runs, so the unsaved document goes with it. 64 is
/// chosen against what a field can hold (nothing typed nests past a handful;
/// depth is a property of shape, not length), not against the stack limit, so
/// the failure stays unreachable on a smaller thread stack.
const MAX_DEPTH: u32 = 64;

impl Parser<'_> {
    fn peek(&self) -> Option<Tok> {
        self.t.get(self.at).copied()
    }

    fn eat(&mut self, tok: Tok) -> bool {
        let hit = self.peek() == Some(tok);
        self.at += usize::from(hit);
        hit
    }

    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        while let Some(op @ (Tok::Add | Tok::Sub)) = self.peek() {
            self.at += 1;
            let rhs = self.term()?;
            v = match op {
                Tok::Add => v + rhs,
                _ => v - rhs,
            };
        }
        Some(v)
    }

    fn term(&mut self) -> Option<f64> {
        let mut v = self.unary()?;
        while let Some(op @ (Tok::Mul | Tok::Div)) = self.peek() {
            self.at += 1;
            let rhs = self.unary()?;
            v = match op {
                Tok::Mul => v * rhs,
                _ => v / rhs,
            };
        }
        Some(v)
    }

    /// The one rule on both recursion cycles (a bracket goes `atom` → `expr` →
    /// `term` → `unary` → `atom`, a sign `unary` → `unary`), so the only place
    /// depth is counted. A flat expression pays nothing: the depth comes back
    /// down before the next term is read.
    fn unary(&mut self) -> Option<f64> {
        if self.depth >= MAX_DEPTH {
            return None;
        }
        self.depth += 1;
        let v = self.unary_nested();
        self.depth -= 1;
        v
    }

    fn unary_nested(&mut self) -> Option<f64> {
        match self.peek()? {
            Tok::Sub => {
                self.at += 1;
                Some(-self.unary()?)
            }
            Tok::Add => {
                self.at += 1;
                self.unary()
            }
            _ => self.atom(),
        }
    }

    fn atom(&mut self) -> Option<f64> {
        let v = match self.peek()? {
            Tok::Num(v) => {
                self.at += 1;
                v
            }
            Tok::Open => {
                self.at += 1;
                let v = self.expr()?;
                if !self.eat(Tok::Close) {
                    return None;
                }
                v
            }
            _ => return None,
        };
        // A unit is dropped here, so one with no value before it (`px`, `%`) is
        // rejected; `evaluate` has already checked which unit it was.
        while matches!(self.peek(), Some(Tok::Unit(_))) {
            self.at += 1;
        }
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins what `evaluate`'s last line rejects (a token the parse did not reach)
    /// and what slips past it (a space before a digit, a trailing unit).
    ///
    /// flip: `p.at == tokens.len()` → `true` fails at the first two assertions;
    /// the four acceptances stay green, as they concern the lexer.
    #[test]
    fn trailing_rejects_tokens_the_parse_did_not_reach() {
        assert_eq!(eval("(1+2))"), None);
        assert_eq!(eval("1)"), None);

        // Not rejections, each for its own stated reason.
        assert_eq!(eval("1 2"), Some(12.0), "a space with a digit after it");
        assert_eq!(eval("1 + 2 3"), Some(24.0), "so `2 3` is 23, and 1 + 23");
        assert_eq!(eval("2px px"), Some(2.0), "`atom` drops a unit token");
        assert_eq!(eval("  135  "), Some(135.0), "a field's own padding");
    }

    /// A width of `135 * 2` comes out 270.
    #[test]
    fn a_field_takes_a_sum_and_reads_its_answer() {
        assert_eq!(eval("135 * 2"), Some(270.0));
        assert_eq!(eval("135*2"), Some(270.0));
        assert_eq!(eval("100+20"), Some(120.0));
        assert_eq!(eval("270 / 2"), Some(135.0));
        assert_eq!(eval("100 - 20.5"), Some(79.5));
    }

    /// Precedence, brackets and left-associativity.
    #[test]
    fn multiplication_binds_tighter_than_addition_and_brackets_beat_both() {
        assert_eq!(eval("2 + 3 * 4"), Some(14.0));
        assert_eq!(eval("(2 + 3) * 4"), Some(20.0));
        assert_eq!(eval("2 * (3 + 4) / 2"), Some(7.0));
        assert_eq!(eval("((5))"), Some(5.0));
        // Left-associative, visible only in the non-commutative operators.
        assert_eq!(eval("10 - 3 - 2"), Some(5.0));
        assert_eq!(eval("100 / 5 / 2"), Some(10.0));
    }

    #[test]
    fn a_sign_is_a_prefix_and_can_follow_an_operator() {
        assert_eq!(eval("-5"), Some(-5.0));
        assert_eq!(eval("- 5"), Some(-5.0));
        assert_eq!(eval("3 * -2"), Some(-6.0));
        assert_eq!(eval("3 - -2"), Some(5.0));
        assert_eq!(eval("--5"), Some(5.0));
        assert_eq!(eval("+7"), Some(7.0));
        assert_eq!(eval("-(2 + 3)"), Some(-5.0));
    }

    /// Everything egui's default parser accepted still parses.
    #[test]
    fn every_spelling_egui_used_to_parse_still_parses() {
        assert_eq!(eval("135"), Some(135.0));
        assert_eq!(eval("  135  "), Some(135.0));
        assert_eq!(eval("-0.5"), Some(-0.5));
        assert_eq!(eval(".5"), Some(0.5));
        assert_eq!(eval("1e3"), Some(1000.0));
        assert_eq!(eval("1.5e-2"), Some(0.015));
        // Whitespace as a thousands separator.
        assert_eq!(eval("1 000"), Some(1000.0));
        assert_eq!(eval("1 000 000 / 1000"), Some(1000.0));
        // U+2212, the minus a formatter writes.
        assert_eq!(eval("−5"), Some(-5.0));
        assert_eq!(eval("10 − 4"), Some(6.0));
    }

    /// A unit is dropped wherever a value can carry one, mid-sum included.
    #[test]
    fn a_trailing_unit_is_dropped_and_never_converts() {
        assert_eq!(eval("50%"), Some(50.0));
        assert_eq!(eval("45°"), Some(45.0));
        assert_eq!(eval("12px"), Some(12.0));
        assert_eq!(eval("12 px"), Some(12.0));
        assert_eq!(eval("1.5em"), Some(1.5));
        assert_eq!(eval("90 deg / 2"), Some(45.0));
        assert_eq!(eval("50% * 2"), Some(100.0));
        assert_eq!(eval("(50 + 10)%"), Some(60.0));
        // Case-insensitive.
        assert_eq!(eval("12PX"), Some(12.0));
    }

    /// Half-typed input is rejected, not read as its prefix.
    #[test]
    fn a_partial_expression_is_a_rejection_rather_than_its_own_prefix() {
        for half in ["135 *", "135 +", "-", "(", "(1 + 2", "1 +", "*"] {
            assert_eq!(eval(half), None, "{half:?} evaluated to something");
        }
    }

    #[test]
    fn nothing_that_is_not_arithmetic_parses() {
        for junk in [
            "", "   ", "abc", "1 + abc", "px", "%", "1)", "(1))", "1 2 +", ")", "1..2", "e3",
        ] {
            assert_eq!(eval(junk), None, "{junk:?} evaluated to something");
        }
    }

    /// A non-finite result is rejected; the range clamp would hide it.
    #[test]
    fn a_non_finite_answer_is_no_answer() {
        assert_eq!(eval("1/0"), None);
        assert_eq!(eval("-1/0"), None);
        assert_eq!(eval("0/0"), None);
        assert_eq!(eval("1e400"), None);
        assert_eq!(eval("1e200 * 1e200"), None);
        // Zero divided by anything is still a number.
        assert_eq!(eval("0/5"), Some(0.0));
    }

    /// No names of any kind parse; functions (`min`, `sqrt`) are declined on
    /// purpose (§9.2).
    #[test]
    fn there_are_no_identifiers_to_grow_a_language_from() {
        for named in [
            "parent.width",
            "width * 2",
            "pi",
            "min(1, 2)",
            "sqrt(4)",
            ".width + 5",
        ] {
            assert_eq!(eval(named), None, "{named:?} evaluated to something");
        }
    }

    /// A field showing one unit refuses a value written in another.
    ///
    /// flip: `Unit::Pct` also accepting `Written::Em` → fails at the third
    /// assertion only; `accepts` → `true` fails at the first.
    #[test]
    fn a_unit_a_field_is_not_showing_is_refused_rather_than_read_as_the_other() {
        // The three wrong answers this closes.
        assert_eq!(eval_in("12px", Unit::Pct), None);
        assert_eq!(eval_in("150%", Unit::Px), None);
        assert_eq!(eval_in("1.5em", Unit::Pct), None);

        // Bare digits are the field's own unit; the arithmetic is untouched.
        assert_eq!(eval_in("12", Unit::Px), Some(12.0));
        assert_eq!(eval_in("12", Unit::Pct), Some(12.0));
        assert_eq!(eval_in("135 * 2", Unit::Px), Some(270.0));

        // The unit the field is showing still round-trips.
        assert_eq!(eval_in("12px", Unit::Px), Some(12.0));
        assert_eq!(eval_in("150%", Unit::Pct), Some(150.0));
        assert_eq!(eval_in("12PX", Unit::Px), Some(12.0));

        // Neither field takes a point or a degree; `eval` still ignores them.
        for (text, showing) in [
            ("12pt", Unit::Px),
            ("12pt", Unit::Pct),
            ("45°", Unit::Px),
            ("45deg", Unit::Pct),
        ] {
            assert_eq!(eval_in(text, showing), None, "{text:?} in {showing:?}");
            assert!(eval(text).is_some(), "{text:?} stopped parsing at all");
        }

        // A unit mid-sum fails the whole expression.
        assert_eq!(eval_in("12px * 2", Unit::Pct), None);
        assert_eq!(eval_in("12px * 2", Unit::Px), Some(24.0));
    }

    /// A deeply nested paste reads as an expression the field cannot use, covering
    /// both recursion doors (`atom`'s `Open` arm and `unary`'s sign arms).
    ///
    /// flip: `MAX_DEPTH` → 100_000 aborts the test binary (`0xc00000fd`, a stack
    /// overflow, no assertion) on the first case.
    #[test]
    fn a_deeply_nested_expression_is_refused_rather_than_crashing() {
        assert_eq!(eval(&"(".repeat(100_000)), None);
        assert_eq!(eval(&"-".repeat(100_000)), None);
        assert_eq!(
            eval(&format!("{}1{}", "(".repeat(1_000), ")".repeat(1_000))),
            None
        );
    }

    /// Nothing a field can hold reaches the bound: 60 nested brackets and a
    /// thousand flat terms still evaluate.
    ///
    /// flip: delete `self.depth -= 1;` in `unary` → fails at the second assertion
    /// only (60 brackets still fit under 64 without the decrement).
    #[test]
    fn nesting_a_field_could_plausibly_hold_still_evaluates() {
        assert_eq!(
            eval(&format!("{}7{}", "(".repeat(60), ")".repeat(60))),
            Some(7.0)
        );
        let flat = std::iter::repeat_n("1", 1_000)
            .collect::<Vec<_>>()
            .join("+");
        assert_eq!(eval(&flat), Some(1_000.0));
    }
}
