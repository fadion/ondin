//! Arithmetic typed into a numeric field (§9.2).
//!
//! A width field takes `135 * 2` and reads 270 afterwards. That is the whole
//! feature: **one-shot evaluation**, the same rule §13 decision 1 already fixed
//! for the command bar's expressions — the result is a number and nothing about
//! the sum survives it. `135 * 2` is not a relationship the field remembers, so
//! nothing here is a step towards the constraint engine that decision forbids.
//!
//! It is deliberately a *calculator*, not a language: four operators, brackets,
//! a sign, and no names of any kind. There is no `parent.width`, no unit
//! conversion and no `50%` — the referent a percentage would need before it can
//! mean anything is `docs/vm.md`'s open question 3, which used to be filed under
//! `roadmap.md`'s *Units* and outlived it (§15 D358) — and a grammar that grows
//! identifiers is the seam through which the command bar's Luau (`docs/vm.md`)
//! would end up half-reimplemented in the inspector.
//!
//! **A unit is noise, not a conversion.** `%`, `°`, `px`, `pt` and `em` may
//! trail a value and are dropped. That is not politeness: several fields format
//! their own unit into the digits (`format!("{v:.0}%")`), so the text handed
//! back to be parsed *is* what the field just displayed, and a parser refusing
//! it would make those fields un-typeable. It is also why this replaced their
//! hand-rolled `trim_end_matches(['%', ' '])` parsers rather than sitting beside
//! them.
//!
//! **But where a field has a second unit to be confused with, a unit it is not
//! showing is a rejection** — [`eval_in`], which every `Length` field uses and
//! which [`eval`] is the no-second-unit version of. Dropping the word gives the
//! right answer exactly where there is only one unit it could have meant, and
//! the typography `Length` fields are the one place in the app where there are
//! two: `12px` typed into a field showing `%` used to read as twelve *percent*,
//! silently, and `150%` typed into a `px` field as a hundred and fifty px. The
//! digits are percent-scaled in the first, so even `1.5em` there means 1.5%
//! rather than the 150 it looks like. None of the three is converted — the unit
//! chip beside the field is the mechanism for that, and inventing a second here
//! is the mistake the old *Units* entry named — so all three are refused and the
//! field keeps the value it had (§15 D359).
//!
//! Geometry needs none of this and never will: **px is the only unit Ondin
//! has** outside typography, decided rather than deferred (§15 D358), so a
//! trailing `px` there is the noise it looks like.
//!
//! Everything egui's own default parser accepted still parses, which is the bar
//! this had to clear to become every field's parser: whitespace inside a number
//! is a thousands separator (`1 000`), and U+2212 is a minus.

/// Evaluate a field's text, or `None` if it is not an expression.
///
/// `None` is what a `DragValue`'s parser says for "leave the value alone", and
/// that is the right answer for every failure here — half-typed input included.
/// Fields update while editing (egui's `update_while_editing` default), so the
/// string is parsed at every keystroke: `135 *` has to be *rejected* on its way
/// to `135 * 2`, not leniently read as 135, or the field would walk the document
/// through the operands of the sum being typed.
///
/// A non-finite result is a failure too. `1/0` is an answer no field wants, and
/// the range clamp would quietly turn it into the field's maximum.
pub fn eval(text: &str) -> Option<f64> {
    evaluate(text, None)
}

/// The unit a field is showing, for [`eval_in`].
///
/// Two variants because two is how many units the app has that can be confused
/// with each other — `Length` is `Px | Em` and an `Em` field writes its digits as
/// a percentage. Everything else shows one unit or none, and [`eval`] is the
/// right parser for it.
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
    /// **`Pct` does not accept `em`**, though a percent field *is* an em field:
    /// `1.5em` and `150%` are the same length and not the same digits, so taking
    /// the first would mean multiplying by a hundred, and that is the conversion
    /// the chip exists for. Refusing it leaves the field alone, which is a visible
    /// no rather than a wrong number.
    fn accepts(self, w: Written) -> bool {
        matches!(
            (self, w),
            (Unit::Px, Written::Px) | (Unit::Pct, Written::Pct)
        )
    }
}

/// [`eval`] for a field that is showing one of two units.
///
/// A unit that is not the one on the field is a rejection — `None`, which is a
/// `DragValue` parser's "leave the value alone". Bare digits are always the
/// field's own unit, so nothing a user typed before this behaves differently.
pub fn eval_in(text: &str, showing: Unit) -> Option<f64> {
    evaluate(text, Some(showing))
}

/// The body of both, `showing` being `None` for a field with no second unit.
fn evaluate(text: &str, showing: Option<Unit>) -> Option<f64> {
    let tokens = lex(text)?;
    // Before the parse rather than inside it: a unit is dropped in `atom`, so by
    // the time arithmetic has run there is nothing left to disagree with, and
    // `12px * 2` has to fail as a whole rather than as its first operand.
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
    // A token the parse did not reach is a rejection: `(1+2))` and `1)` are not
    // sums with a tail, they are mistakes.
    //
    // ⚠️ **It is a check on *tokens*, so it catches less than "trailing anything"**,
    // and this comment claimed `1 2` until 2026-09-06. `1 2` is one token: `number`
    // treats a space with a digit after it as *inside* the number (the thousands
    // rule egui compatibility forces), so `1 2` is `Num(12.0)` and never reaches
    // here — nor does `1 + 2 3`, which is 24. A trailing bare unit gets through by
    // the other road: `atom` drops units, so `2px px` is 2. Measured, and
    // `trailing_rejects_tokens_the_parse_did_not_reach` holds all four.
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
    /// A unit trailing a value, which means nothing to the arithmetic — but
    /// carries which one it was, because [`eval_in`]'s fields care.
    Unit(Written),
}

/// A unit as it was written, so a field showing one can refuse the others.
///
/// `Pt` and `Deg` are here to be *accepted by nothing*: they are taken by the
/// lexer so that a geometry field goes on ignoring them ([`eval`]), and refused
/// by both [`Unit`]s, since neither a point nor a degree is a length this app
/// converts.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Written {
    Px,
    Pt,
    Em,
    Pct,
    Deg,
}

/// The units a value may trail, lowercased, and what each is. Anything else
/// word-shaped is a rejection — an unknown identifier is far likelier to be a
/// typo than a unit, and there is nothing here for a name to refer to.
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
        // `−` (U+2212) is the character a formatter or a word processor writes a
        // minus as — egui's own parser folds it too. `×` and `÷` are what a
        // keyboard layout or a paste produces for the two operators with no
        // ASCII glyph of their own.
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
/// **Whitespace inside a number is skipped rather than ended on**, the one place
/// this lexer is not free to be strict: egui's default parser filters whitespace
/// out of the whole string before parsing, so `1 000` has always been a thousand
/// and fields have always taken it. A space is only *inside* the number when a
/// digit follows it — `135 * 2` and `2 px` end the number at the space, because
/// what follows is not one.
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
                // Nothing but whitespace left is the end of the number, not a
                // failure — `"  135  "` is a number with a field's padding on it.
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
            // An exponent, and only where digits actually follow it: `2e3` is a
            // number where `2em` is a value with a unit on it, and the two are
            // told apart by what comes after the `e`.
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
    // A lone `.` reaches here as a dot and nothing else, and fails to parse.
    Some((s.parse().ok()?, i))
}

/// Precedence climbing over the token list — `+ -` under `* /`, brackets, and a
/// leading sign. Every rule returns `None` rather than recovering, so a partial
/// expression cannot evaluate to part of itself.
struct Parser<'a> {
    t: &'a [Tok],
    at: usize,
    /// How many nested rules are open, bounded by [`MAX_DEPTH`] (§15 D669).
    depth: u32,
}

/// How deep the grammar may nest before the parser gives up.
///
/// 🚨 **A bound on the *stack*, not a rule about expressions** (§15 D669,
/// `[S16.5-L1-02]`). The descent recurses once per open bracket and once per
/// leading sign with nothing counting, and a **stack overflow is not a panic**:
/// the process dies with `STATUS_STACK_OVERFLOW`, nothing unwinds, and neither
/// `OndinApp::on_exit`'s `disk_settle` nor the library writer's `Drop` runs — so
/// the unsaved document and the crash snapshot go with it. Measured at ~500 bytes
/// of stack per bracket in a debug build, which puts the threshold on the 8 MiB
/// main thread between 8,000 and 20,000 characters; a 1 MiB thread died at 2,000.
///
/// ⚠️ **64 is chosen against what a *field* can hold, not against the stack
/// figure.** This parser is the `custom_parser` on every numeric field in the app,
/// and nothing anybody types into one nests past a handful — the depth is a
/// property of the expression, not of its length, so `1+1+1+…` a thousand times
/// over stays at 1. Setting it near the measured limit would leave the failure
/// reachable on a thread with a smaller stack; setting it here costs nothing that
/// was ever going to be typed.
///
/// `None` is what the field already does with an expression it cannot read: leave
/// the value alone.
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

    /// The one rule on **both** recursion cycles, and therefore the only place the
    /// depth has to be counted (§15 D669).
    ///
    /// A bracket recurses `atom` → `expr` → `term` → `unary` → `atom`, and a
    /// leading sign recurses `unary` → `unary`; this sits on each. Counting here
    /// rather than at the two call sites means a rule added later that recurses
    /// through the grammar at all is bounded without anyone remembering to bound
    /// it — and a flat expression pays nothing, because the depth comes back down
    /// before the next term is read.
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
        // A unit belongs to the value it trails, so this is where it is dropped
        // — which also makes a unit with no value in front of it (`px`, `%`) the
        // rejection it should be, since nothing else consumes one. Which unit it
        // was has already been settled by `evaluate`; here they are all noise.
        while matches!(self.peek(), Some(Tok::Unit(_))) {
            self.at += 1;
        }
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `evaluate`'s last line rejects, and — the half that was wrong in its
    /// comment until 2026-09-06 — what it does not.
    ///
    /// The check is `p.at == tokens.len()`, i.e. *a token the parse did not
    /// reach*, which is narrower than the "trailing anything" the comment
    /// promised. Two roads go round it and both are asserted here, because the
    /// comment named one of them as an example of the rule: whitespace with a
    /// digit after it is inside a number (`number`'s thousands rule), and `atom`
    /// drops a unit token wherever it sits.
    ///
    /// ⚠️ **Flipped** by changing the last line's `p.at == tokens.len()` to
    /// `true`: `(1+2))` comes back `Some(3.0)` and `1)` `Some(1.0)`, so the two
    /// rejection assertions bite. The four acceptances stay green under that
    /// flip — which is the point of having them, since they are about the lexer
    /// and not about this check at all.
    #[test]
    fn trailing_rejects_tokens_the_parse_did_not_reach() {
        assert_eq!(eval("(1+2))"), None);
        assert_eq!(eval("1)"), None);

        // Not rejections, and each for its own stated reason.
        assert_eq!(eval("1 2"), Some(12.0), "a space with a digit after it");
        assert_eq!(eval("1 + 2 3"), Some(24.0), "so `2 3` is 23, and 1 + 23");
        assert_eq!(eval("2px px"), Some(2.0), "`atom` drops a unit token");
        assert_eq!(eval("  135  "), Some(135.0), "a field's own padding");
    }

    /// The reported gap, in the words it was reported in: a width of `135 * 2`
    /// has to come out 270.
    #[test]
    fn a_field_takes_a_sum_and_reads_its_answer() {
        assert_eq!(eval("135 * 2"), Some(270.0));
        assert_eq!(eval("135*2"), Some(270.0));
        assert_eq!(eval("100+20"), Some(120.0));
        assert_eq!(eval("270 / 2"), Some(135.0));
        assert_eq!(eval("100 - 20.5"), Some(79.5));
    }

    /// Precedence and brackets, because a calculator that evaluates left to
    /// right gives a different answer rather than a simpler one.
    #[test]
    fn multiplication_binds_tighter_than_addition_and_brackets_beat_both() {
        assert_eq!(eval("2 + 3 * 4"), Some(14.0));
        assert_eq!(eval("(2 + 3) * 4"), Some(20.0));
        assert_eq!(eval("2 * (3 + 4) / 2"), Some(7.0));
        assert_eq!(eval("((5))"), Some(5.0));
        // Left-associative, which only shows in the two operators that are not
        // commutative.
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

    /// This is now the parser every plain number goes through, so everything
    /// egui's default accepted has to still arrive.
    #[test]
    fn every_spelling_egui_used_to_parse_still_parses() {
        assert_eq!(eval("135"), Some(135.0));
        assert_eq!(eval("  135  "), Some(135.0));
        assert_eq!(eval("-0.5"), Some(-0.5));
        assert_eq!(eval(".5"), Some(0.5));
        assert_eq!(eval("1e3"), Some(1000.0));
        assert_eq!(eval("1.5e-2"), Some(0.015));
        // Whitespace as a thousands separator — egui filters it out of the whole
        // string, so this has always been a thousand.
        assert_eq!(eval("1 000"), Some(1000.0));
        assert_eq!(eval("1 000 000 / 1000"), Some(1000.0));
        // U+2212, the minus a formatter writes.
        assert_eq!(eval("−5"), Some(-5.0));
        assert_eq!(eval("10 − 4"), Some(6.0));
    }

    /// A unit is dropped wherever a value can carry one — including mid-sum,
    /// since a field that *displays* `50%` is exactly the one whose text arrives
    /// with a unit already on it.
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
        // Case is not a second spelling to remember.
        assert_eq!(eval("12PX"), Some(12.0));
    }

    /// Half-typed input must be rejected, not read leniently. The field parses
    /// at every keystroke, so `135 *` is a string this sees on the way to
    /// `135 * 2`.
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

    /// A field cannot hold an infinity, and the range clamp would hide one by
    /// turning it into the field's maximum.
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

    /// No names, of any kind — the seam through which this would stop being a
    /// calculator (§13 decision 1, `docs/vm.md`).
    ///
    /// `min(1, 2)` and `sqrt(4)` are in this list deliberately: **functions were
    /// considered and declined**, and §9.2 carries the reasoning — a function
    /// earns its keep on an operand nobody knows yet, and every operand a field
    /// can see is a literal somebody just typed.
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

    /// **The reported gap in the units it was reported in:** `12px` typed into a
    /// field showing `%` read as *twelve percent*, silently. The `Length` fields
    /// are the one place in the app that shows two units, so they are the one
    /// place dropping the word gives a wrong answer instead of the right one, and
    /// the answer is a refusal — the field keeps what it had — rather than the
    /// conversion the unit chip is for.
    ///
    /// ⚠️ **Flipped against the version somebody would actually have written**,
    /// which is `Unit::Pct` also accepting `Written::Em`: a percent field *is* an
    /// em field, so letting `em` through looks obviously right. It is not, because
    /// the digits are percent-scaled — `1.5em` would land as 1.5%, a hundredth of
    /// what was typed. That flip fails on the third assertion below and on nothing
    /// else, which is the assertion carrying the whole distinction. Flipping
    /// `accepts` to a bare `true` instead makes this `eval` again and fails on the
    /// first.
    #[test]
    fn a_unit_a_field_is_not_showing_is_refused_rather_than_read_as_the_other() {
        // The three wrong answers this closes.
        assert_eq!(eval_in("12px", Unit::Pct), None);
        assert_eq!(eval_in("150%", Unit::Px), None);
        assert_eq!(eval_in("1.5em", Unit::Pct), None);

        // Bare digits are the field's own unit, as they always were, and the
        // arithmetic is untouched.
        assert_eq!(eval_in("12", Unit::Px), Some(12.0));
        assert_eq!(eval_in("12", Unit::Pct), Some(12.0));
        assert_eq!(eval_in("135 * 2", Unit::Px), Some(270.0));

        // The unit the field *is* showing still round-trips, which is the reason
        // units are accepted at all: `%` is what the formatter wrote.
        assert_eq!(eval_in("12px", Unit::Px), Some(12.0));
        assert_eq!(eval_in("150%", Unit::Pct), Some(150.0));
        assert_eq!(eval_in("12PX", Unit::Px), Some(12.0));

        // Neither field takes a point or a degree: both would be conversions,
        // and `eval` goes on ignoring them for the geometry fields that have no
        // second unit to confuse them with.
        for (text, showing) in [
            ("12pt", Unit::Px),
            ("12pt", Unit::Pct),
            ("45°", Unit::Px),
            ("45deg", Unit::Pct),
        ] {
            assert_eq!(eval_in(text, showing), None, "{text:?} in {showing:?}");
            assert!(eval(text).is_some(), "{text:?} stopped parsing at all");
        }

        // A unit mid-sum fails the whole expression rather than its first
        // operand — the reason `evaluate` checks before parsing rather than
        // leaving it to `atom`, where the word is already gone.
        assert_eq!(eval_in("12px * 2", Unit::Pct), None);
        assert_eq!(eval_in("12px * 2", Unit::Px), Some(24.0));
    }

    /// A paste that used to take the process down now reads as an expression the
    /// field cannot use (§15 D669, `[S16.5-L1-02]`).
    ///
    /// 🚨 **This test could not have been written before the fix**, which is the
    /// unusual thing about it: the failure is `STATUS_STACK_OVERFLOW` rather than a
    /// panic, so the case below *aborted the test binary* instead of failing it —
    /// no assertion, no name, nothing in the output that says which test died.
    /// There is therefore no "run it red first" for this one, and the flip below is
    /// the whole of the evidence.
    ///
    /// **Flip:** raise `MAX_DEPTH` to 100_000 and `cargo test -p ondin-app expr`
    /// exits `0xc00000fd` with no test result at all, on the first case. Predicted
    /// correctly, and it is worth doing once rather than trusting this comment: the
    /// crash is what the guard is for, and a test that merely returns `None` looks
    /// identical whether the guard is doing anything or not.
    ///
    /// Both doors are covered because they are different rules — `atom`'s `Open`
    /// arm and `unary`'s sign arms — and `unary` is the one place that counts for
    /// both.
    #[test]
    fn a_deeply_nested_expression_is_refused_rather_than_crashing() {
        assert_eq!(eval(&"(".repeat(100_000)), None);
        assert_eq!(eval(&"-".repeat(100_000)), None);
        assert_eq!(
            eval(&format!("{}1{}", "(".repeat(1_000), ")".repeat(1_000))),
            None
        );
    }

    /// The bound is far enough out that nothing a field can hold reaches it.
    ///
    /// ⚠️ **The second assertion is the one that stops the guard being a silent
    /// narrowing**: an expression's depth is a property of its *shape*, not its
    /// length, so a thousand terms in a row must still evaluate. It does, because
    /// `unary` puts the depth back before the next term is read.
    ///
    /// **Flip:** delete the `self.depth -= 1;` and this fails on the **second**
    /// assertion — `left: None, right: Some(1000.0)` — while every other test in
    /// the file, this one's own first assertion included, stays green. ⚠️ That is
    /// the site correction worth carrying: 60 nested brackets still fit under 64
    /// without the decrement, so the case that pins it is the *flat* expression
    /// that does not nest at all.
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
