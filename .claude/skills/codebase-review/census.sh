#!/usr/bin/env bash
# Pass 0's census for the codebase-review and release-review skills.
#
# Every count comes from one scanner (SCAN below) that reads Rust source with its comments,
# strings and char literals understood, so a doc comment *discussing* `#[allow]` is never
# counted as one, and that classes each line P (production) or T (test). T is: a file under
# crates/*/{tests,benches,examples}/; the lines of each `#[cfg(test)]` item, brace-matched to
# the item's closing `}` (or its `;` / `,` when it has no body); and a file that a
# `#[cfg(test)] mod name;` pulls in. A split at the first `#[cfg(test)]` is wrong here:
# inspector.rs's first one sits far above its last production code.
#
# Usage (from the repository root, Git Bash: prefix MSYS_NO_PATHCONV=1):
#   census.sh selftest                 controls every row against fixtures; run it first
#   census.sh [--rev R] <row> [list]   whole tree at R (default HEAD): panics index unsafe
#                                      allow todo ignore docruns preview partialeq
#   census.sh range <base> <row>       lines added by <base>..HEAD: panics index allow todo
#                                      ignore testlinks testdelta docruns newmods clippy
#   census.sh scan <dir>               the classified lines themselves, for a spot check
set -euo pipefail
export LC_ALL=C
SELF=$(cd "$(dirname "$0")" && pwd)/$(basename "$0")

# Prints path \t line \t class \t code \t comment \t raw for every line of the .rs files under
# the directory given as `root`. `code` has comments removed and string / char contents
# emptied; `comment` is the comment text. Lines of out-of-line test modules are reclassed by
# `reclass` afterwards, from the M records this emits.
read -r -d '' SCAN <<'AWK' || true
function hashes(k,   s) { s = ""; while (k-- > 0) s = s "#"; return s }
function child(path, name,   dir, base) {
  dir = path; sub(/\/[^\/]*$/, "", dir); base = path; sub(/^.*\//, "", base)
  if (base != "main.rs" && base != "lib.rs" && base != "mod.rs") { sub(/\.rs$/, "", base); dir = dir "/" base }
  print "M", dir "/" name ".rs"; print "M", dir "/" name "/"
}
function startpend() { pend = 1; pdepth = depth; ppar = par; pang = 0; lineT = 1 }
BEGIN { OFS = "\t"; DQ = "\""; BS = "\\" }
FNR == 1 {
  blk = 0; instr = 0; raw = -1; depth = 0; par = 0; pend = 0; tdepth = -1
  path = FILENAME; sub("^" root "/", "", path)
  whole = (path ~ /^crates\/[^\/]+\/(tests|benches|examples)\//)
}
{
  line = $0; n = length(line); code = ""; com = ""; i = 1
  lineT = (tdepth >= 0 || pend)
  while (i <= n) {
    if (blk > 0) {
      two = substr(line, i, 2)
      if (two == "*/") { blk--; i += 2; continue }
      if (two == "/*") { blk++; i += 2; continue }
      com = com substr(line, i, 1); i++; continue
    }
    if (instr) {
      rest = substr(line, i)
      if (!match(rest, /[\\"]/)) { i = n + 1; continue }
      i += RSTART - 1; ch = substr(line, i, 1)
      if (ch == BS) { i += (raw < 0) ? 2 : 1; continue }
      if (raw < 0) { instr = 0; code = code DQ; i++; continue }
      if (substr(line, i + 1, raw) == hashes(raw)) { instr = 0; code = code DQ hashes(raw); i += 1 + raw; continue }
      i++; continue
    }
    rest = substr(line, i)
    if (!match(rest, /[\/"'{}()\[\];,<>#]/)) { code = code rest; break }
    code = code substr(rest, 1, RSTART - 1); i += RSTART - 1
    ch = substr(line, i, 1); nx = substr(line, i + 1, 1)
    if (ch == "/") {
      if (nx == "/") { com = com substr(line, i); break }
      if (nx == "*") { blk = 1; com = com "/*"; i += 2; continue }
      code = code ch; i++; continue
    }
    if (ch == DQ) {
      if (match(code, /(^|[^A-Za-z0-9_])[bc]?r#*$/)) { h = code; sub(/^.*r/, "", h); raw = length(h) } else raw = -1
      instr = 1; code = code DQ; i++; continue
    }
    if (ch == "'") {
      rest = substr(line, i)
      if (match(rest, /^'(\\(x[0-9a-fA-F][0-9a-fA-F]|u\{[0-9a-fA-F]+\}|.)|[^\\'\200-\377]|[\300-\377][\200-\277]+)'/)) {
        code = code "' '"; i += RLENGTH; continue
      }
      code = code ch; i++; continue
    }
    if (ch == "#") {
      if (tdepth < 0 && !pend && substr(line, i) ~ /^#\[cfg\(test\)\]/) startpend()
      code = code ch; i++; continue
    }
    if (ch == "(" || ch == "[") par++
    else if (ch == ")" || ch == "]") par--
    else if (ch == "{") {
      if (pend && depth == pdepth) { pend = 0; tdepth = pdepth }
      depth++
    } else if (ch == "}") {
      depth--
      if (tdepth >= 0 && depth == tdepth) tdepth = -1
      if (pend && depth < pdepth) pend = 0
    } else if (pend && depth == pdepth && par == ppar) {
      p = substr(line, i - 1, 1)
      if (ch == "<" && nx != "=") pang++
      else if (ch == ">" && p != "-" && p != "=" && nx != "=" && pang > 0) pang--
      else if (ch == ";" || (ch == "," && pang == 0)) {
        if (ch == ";" && match(code, /(^|[^A-Za-z0-9_])mod[ \t]+[A-Za-z0-9_]+[ \t]*$/)) {
          m = substr(code, RSTART, RLENGTH); sub(/^.*mod[ \t]+/, "", m); sub(/[ \t]*$/, "", m); child(path, m)
        }
        pend = 0
      }
    }
    code = code ch; i++
  }
  gsub(/\t/, " ", code); gsub(/\t/, " ", com); r = line; gsub(/\t/, " ", r); sub(/\r$/, "", r)
  print path, FNR, ((whole || lineT || tdepth >= 0 || pend) ? "T" : "P"), code, com, r
}
AWK

# Reclasses the lines of files pulled in by `#[cfg(test)] mod name;` as T, and drops the M records.
reclass() {
  awk -F'\t' -v OFS='\t' 'NR == FNR { if ($1 == "M") tm[$2] = 1; next }
    $1 == "M" { next }
    { for (p in tm) if ($1 == p || (p ~ /\/$/ && index($1, p) == 1)) { $3 = "T"; break } print }' "$1" "$1"
}

# scan_dir DIR OUT: the classified lines of every .rs file under DIR/crates.
scan_dir() {
  local tmp; tmp=$(mktemp)
  find "$1/crates" -name '*.rs' -type f | sort | xargs awk -v root="$1" "$SCAN" > "$tmp"
  reclass "$tmp" > "$2"; rm -f "$tmp"
}

# tree REV DIR: the tree of crates/ at REV, unpacked into DIR.
tree() { mkdir -p "$2"; git archive "$1" crates | tar -x -C "$2"; }

WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT

# --- the rows, each a filter over a scan file ($1) --------------------------------------------

# unwrap()/expect()/panic!/unreachable!/todo!/unimplemented! occurrences, per crate and class.
row_panics() {
  awk -F'\t' '{ c = $4; split($1, a, "/"); k = a[2] " " $3
    u = gsub(/\.unwrap\(\)/, "", c); e = gsub(/\.expect\(/, "", c)
    p = gsub(/(^|[^A-Za-z0-9_])panic!/, "", c); r = gsub(/(^|[^A-Za-z0-9_])unreachable!/, "", c)
    t = gsub(/(^|[^A-Za-z0-9_])(todo|unimplemented)!/, "", c)
    U[k] += u; E[k] += e; Pn[k] += p; R[k] += r; Td[k] += t; K[k] = 1; tot[$3] += u + e + p + r + t }
    END { printf "%-22s %7s %7s %7s %7s %7s\n", "crate class", "unwrap", "expect", "panic", "unreach", "todo"
      for (k in K) printf "%-22s %7d %7d %7d %7d %7d\n", k, U[k], E[k], Pn[k], R[k], Td[k] | "sort"
      close("sort"); printf "total P %d, T %d\n", tot["P"], tot["T"] }' "$1"
}

# Indexing and slicing expressions (`x[i]`, `x[a..b]`; `[..]` never panics and is skipped).
row_index() {
  awk -F'\t' '{ c = $4; gsub(/\[\.\.\]/, "", c); n = gsub(/[A-Za-z0-9_)\]]\[/, "", c); split($1, a, "/"); N[a[2] " " $3] += n; tot[$3] += n }
    END { for (k in N) printf "%-22s %6d\n", k, N[k] | "sort"; close("sort"); printf "total P %d, T %d\n", tot["P"], tot["T"] }' "$1"
}

# Every `unsafe` keyword in code, one site per line.
row_unsafe() {
  awk -F'\t' '$4 ~ /(^|[^A-Za-z0-9_])unsafe([^A-Za-z0-9_]|$)/ { n[$3]++; print $3, $1 ":" $2 ": " $6 }
    END { printf "total P %d, T %d\n", n["P"], n["T"] }' "$1"
}

# `#[allow(...)]` / `#![allow(...)]` attributes, and `cfg_attr(..., allow(...))`.
row_allow() {
  awk -F'\t' '$4 ~ /#!?\[[ ]*allow[ ]*\(/ || $4 ~ /#!?\[[ ]*cfg_attr\(.*allow[ ]*\(/ { n[$3]++; print $3, $1 ":" $2 ": " $6 }
    END { printf "total P %d, T %d\n", n["P"], n["T"] }' "$1"
}

# TODO / FIXME / XXX / HACK markers: the word opening a comment, or followed by `:` or `(`.
# Prose naming a dependency's TODO is a mention and is not counted.
row_todo() {
  awk -F'\t' '$5 ~ /^\/[\/*][\/!*]? *(TODO|FIXME|XXX|HACK)([^A-Za-z0-9_]|$)/ || $5 ~ /(^|[^A-Za-z0-9_])(TODO|FIXME|XXX|HACK) ?[:(]/ { n[$3]++; print $3, $1 ":" $2 ": " $6 }
    END { printf "total P %d, T %d\n", n["P"], n["T"] }' "$1"
}

# `#[ignore]` attributes; R = carries a reason string, - = none.
row_ignore() {
  awk -F'\t' '$4 ~ /#\[[ ]*ignore([ ]*[]=])/ { r = ($4 ~ /#\[[ ]*ignore[ ]*=/) ? "R" : "-"; n[r]++; print r, $1 ":" $2 ": " $6 }
    END { printf "total %d (with reason %d, without %d)\n", n["R"] + n["-"], n["R"], n["-"] }' "$1"
}

# Contiguous `///` runs, longest first, with the line each one sits above. Runs at line 1 skipped.
row_docruns() {
  awk -F'\t' 'function flush() { if (len > 0 && start > 1) print len "\t" f ":" start "\t" item; len = 0 }
    $1 != f { flush(); f = $1; pend = 0 }
    { r = $6; sub(/^ +/, "", r) }
    $4 ~ /^ *$/ && r ~ /^\/\/\/([^\/]|$)/ { if (len == 0) start = $2; len++; next }
    len > 0 && !pend { pend = 1; item = "" }
    pend && r != "" && r !~ /^#\[/ { item = substr(r, 1, 70); flush(); pend = 0 }
    END { flush() }' "$1" | sort -t$'\t' -k1,1nr | awk -v n="${TOP:-40}" 'NR <= n'
}

# `set_preview*` / `get_preview` calls beside `preview_guide` calls, per file (production).
row_preview() {
  awk -F'\t' '$3 == "P" { c = $4; s = gsub(/\.(set_preview[A-Za-z_]*|get_preview)\(/, "", c); g = gsub(/preview_guide\(/, "", c); S[$1] += s; G[$1] += g }
    END { printf "%-48s %6s %6s\n", "file", "set/get", "guide"; for (f in S) if (S[f] + G[f]) printf "%-48s %6d %6d\n", f, S[f], G[f] }' "$1"
}

# Structs deriving PartialEq, per crate (production) — the population whose unread fields hide.
row_partialeq() {
  awk -F'\t' '$3 == "P" && $4 ~ /#\[derive\(.*PartialEq/ { want = 1; next }
    want && $4 ~ /^ *#\[/ { next }
    want { if ($4 ~ /(^|[^A-Za-z0-9_])struct[ ]/) { split($1, a, "/"); N[a[2]]++; t++ } want = 0 }
    END { for (k in N) printf "%-14s %4d\n", k, N[k]; printf "total %d\n", t }' "$1"
}

# --- range mode --------------------------------------------------------------------------------

# diff_lines BASE: path \t line \t sign for every +/- line of BASE..HEAD under crates/ (.rs),
# + numbered in HEAD and - in BASE. Hunk sizes are counted down so that a removed line
# reading `-- x` is never taken for a file header.
diff_lines() {
  git diff -U0 --no-color --no-renames "$1" HEAD -- 'crates/*.rs' | awk -v OFS='\t' '
    ob + nb == 0 && /^--- / { old = substr($0, 5); sub(/^a\//, "", old); next }
    ob + nb == 0 && /^\+\+\+ / { new = substr($0, 5); sub(/^b\//, "", new); next }
    ob + nb == 0 && /^@@/ {
      split(substr($2, 2), o, ","); split(substr($3, 2), nn, ",")
      ol = o[1] + 0; ob = (2 in o) ? o[2] + 0 : 1; nl = nn[1] + 0; nb = (2 in nn) ? nn[2] + 0 : 1; next }
    ob + nb > 0 && /^\+/ { print new, nl++, "+"; nb--; next }
    ob + nb > 0 && /^-/ { print old, ol++, "-"; ob--; next }'
}

# joined BASE: the scan lines of every changed line, with the sign in front.
range_join() {
  local base=$1
  if [ ! -s "$WORK/joined" ]; then
    tree HEAD "$WORK/head"; scan_dir "$WORK/head" "$WORK/head.scan"
    tree "$base" "$WORK/base"; scan_dir "$WORK/base" "$WORK/base.scan"
    diff_lines "$base" > "$WORK/diff"
    awk -F'\t' -v OFS='\t' 'FILENAME ~ /diff$/ { W[$3 SUBSEP $1 SUBSEP $2] = 1; next }
      FILENAME ~ /head.scan$/ && ("+" SUBSEP $1 SUBSEP $2) in W { print "+", $0 }
      FILENAME ~ /base.scan$/ && ("-" SUBSEP $1 SUBSEP $2) in W { print "-", $0 }' \
      "$WORK/diff" "$WORK/head.scan" "$WORK/base.scan" > "$WORK/joined"
  fi
}

# The rows over added lines: strip the sign and reuse the whole-tree filter.
added() { awk -F'\t' -v OFS='\t' '$1 == "+" { $1 = ""; sub(/^\t/, ""); print }' "$WORK/joined" > "$WORK/added"; }

# Intra-doc links added in test prose: `///` or `//!` with a `[` in a T line or in build.rs.
range_testlinks() {
  awk -F'\t' '$1 == "+" && ($4 == "T" || $2 ~ /^crates\/[^\/]+\/build\.rs$/) && $6 ~ /^\/\/[\/!]/ && $6 ~ /\[/ { n++; print $2 ":" $3 ": " $7 }
    END { printf "total %d\n", n }' "$WORK/joined"
}

# `///` runs at HEAD longer than MIN (default 52) lines that contain an added line, with the item.
range_docruns() {
  awk -F'\t' -v min="${MIN:-52}" 'FILENAME ~ /diff$/ { if ($3 == "+") W[$1 SUBSEP $2] = 1; next }
    function flush() { if (len > min && start > 1 && hit) print len "\t" f ":" start "\t" item; len = 0; hit = 0 }
    $1 != f { flush(); f = $1; pend = 0 }
    { r = $6; sub(/^ +/, "", r) }
    $4 ~ /^ *$/ && r ~ /^\/\/\/([^\/]|$)/ { if (len == 0) start = $2; len++; if (($1 SUBSEP $2) in W) hit = 1; next }
    len > 0 && !pend { pend = 1; item = "" }
    pend && r != "" && r !~ /^#\[/ { item = substr(r, 1, 70); flush(); pend = 0 }
    END { flush() }' "$WORK/diff" "$WORK/head.scan" | sort -t$'\t' -k1,1nr
}

# Per changed file: +/- line counts, then the `#[test]` attributes in the whole file at base and
# at HEAD and their difference. Whole-file counts, because -U0 diffs often keep a moved
# `#[test]` line as unchanged context.
range_testdelta() {
  awk -F'\t' 'FILENAME ~ /diff$/ { if ($3 == "+") A[$1]++; else D[$1]++; seen[$1] = 1; next }
    $4 ~ /#\[[ ]*test[ ]*\]/ { if (FILENAME ~ /head.scan$/) H[$1]++; else B[$1]++ }
    END { printf "%-56s %6s %6s %6s %6s %6s\n", "file", "+lines", "-lines", "tests0", "tests1", "delta"
      for (f in seen) printf "%-56s %6d %6d %6d %6d %+6d\n", f, A[f], D[f], B[f], H[f], H[f] - B[f] | "sort"
      close("sort") }' "$WORK/diff" "$WORK/head.scan" "$WORK/base.scan"
}

# New modules under crates/*/src, and how many lines of docs/architecture.md and docs/design/
# name the file (`name.rs`, or `name/` for a mod.rs). A bare stem would match prose words.
range_newmods() {
  git diff --name-only --diff-filter=A --no-renames "$1" HEAD -- 'crates/*/src/*.rs' | while read -r f; do
    stem=${f##*/}; [ "$stem" = mod.rs ] && { stem=${f%/mod.rs}; stem=${stem##*/}/; }
    a=$(git show HEAD:docs/architecture.md | grep -cF "$stem" || true)
    d=$({ git grep -c -F "$stem" HEAD -- docs/design || true; } | awk -F: '{ s += $NF } END { print s + 0 }')
    printf '%s\tarchitecture.md %s\tdesign/ %s\n' "$f" "$a" "$d"
  done
}

# clippy_filter CHANGED: the short-format diagnostics on stdin whose file is listed in CHANGED.
clippy_filter() {
  tr '\\' '/' | awk -F: 'NR == FNR { c[$0] = 1; next } $1 in c' "$1" - | sort -u
}

# Clippy's pedantic + nursery warnings in files the range changed (workspace, then per package).
range_clippy() {
  git diff --name-only "$1" HEAD -- 'crates/*.rs' > "$WORK/changed"
  { cargo clippy --workspace --all-targets --message-format=short -- -W clippy::pedantic -W clippy::nursery 2>&1
    for p in $(ls crates); do cargo clippy -p "$p" --all-targets --message-format=short -- -W clippy::pedantic -W clippy::nursery 2>&1; done
  } | clippy_filter "$WORK/changed"
}

# --- selftest: each row against a fixture that must match and one that must not ----------------

selftest() {
  local d="$WORK/fx" fail=0
  mkdir -p "$d/crates/k/src/m" "$d/crates/k/tests"
  # must-match lines carry a `HIT` tag in a trailing comment; must-not lines carry none.
  cat > "$d/crates/k/src/lib.rs" <<'RS'
//! Module docs mention #[allow(dead_code)], #[ignore], .unwrap() and the TODO, none of which count.
/// A doc comment discussing `#[allow(x)]` and `x.unwrap()` is a mention.
/// It also mentions `#[cfg(test)]`, which must not open a test region.
#[allow(dead_code)]
fn prod(v: &[u8], o: Option<u8>) -> u8 {
    let s = "#[allow(x)] .unwrap() panic! { unsafe";
    let c = '{';
    let r = r#"a " } { "#;
    let t: Vec<[u8; 2]> = vec![[1, 2]];
    /* FIXME block .unwrap() #[allow(x)] { */
    // TODO: real marker
    unsafe { core::hint::unreachable_unchecked() };
    o.unwrap() + v[0] + v[..].len() as u8 + v[1..].len() as u8 + o.expect("e")
}
#[cfg(test)]
pub(crate) fn helper() -> u8 { None::<u8>.unwrap() }
struct S {
    #[cfg(test)]
    seen: Vec<Option<(u8, u8)>>,
    live: u8,
}
#[cfg(test)]
mod tests {
    fn far() {
        let c = '}';
        let _ = Some(1).unwrap();
    }
    #[test]
    #[ignore = "needs a GPU"]
    fn gpu() {}
    #[test]
    #[ignore]
    fn bare() {}
}
#[cfg(test)]
mod outline;
fn after_tests() { panic!("late production code") }
/// Three lines
/// right above
/// the item.
fn tail() {}
RS
  printf 'fn in_outline() { Some(1).unwrap(); }\n' > "$d/crates/k/src/outline.rs"
  printf '#[allow(unused)]\nfn t() { Some(1).unwrap(); }\n' > "$d/crates/k/tests/it.rs"
  scan_dir "$d" "$WORK/fx.scan"
  check() { # check <name> <expected> <actual>
    if [ "$2" = "$3" ]; then echo "ok    $1: $3"; else echo "FAIL  $1: expected [$2], got [$3]"; fail=1; fi
  }
  cls() { awk -F'\t' -v l="$1" -v f="$2" '$1 == f && $2 == l { print $3 }' "$WORK/fx.scan"; }
  L=crates/k/src/lib.rs
  check "line 5 fn prod is P" P "$(cls 5 $L)"
  check "line 16 cfg(test) fn helper is T" T "$(cls 16 $L)"
  check "line 17 struct S after it is P" P "$(cls 17 $L)"
  check "line 19 cfg(test) field is T" T "$(cls 19 $L)"
  check "line 20 next field is P" P "$(cls 20 $L)"
  check "line 26 inside mod tests past a '}' char is T" T "$(cls 26 $L)"
  check "line 37 after the test items is P" P "$(cls 37 $L)"
  check "outline.rs pulled in by cfg(test) mod is T" T "$(cls 1 crates/k/src/outline.rs)"
  check "tests/ file is T" T "$(cls 2 crates/k/tests/it.rs)"
  check "panics P (unwrap 1, expect 1, panic 1)" "total P 3, T 4" "$(row_panics "$WORK/fx.scan" | tail -1)"
  check "index: v[0] v[1..] only" "total P 2, T 0" "$(row_index "$WORK/fx.scan" | tail -1)"
  check "unsafe: the block, not the string" "total P 1, T 0" "$(row_unsafe "$WORK/fx.scan" | tail -1)"
  check "allow: attributes, not mentions" "total P 1, T 1" "$(row_allow "$WORK/fx.scan" | tail -1)"
  check "todo: the // and /* markers, not the //! mention or the string" "total P 2, T 0" "$(row_todo "$WORK/fx.scan" | tail -1)"
  check "ignore: one with reason, one without" "total 2 (with reason 1, without 1)" "$(row_ignore "$WORK/fx.scan" | tail -1)"
  check "docruns: longest first, each naming the item below (past an attribute)" \
    "$(printf '3\t%s:38\tfn tail() {}\n2\t%s:2\tfn prod(v: &[u8], o: Option<u8>) -> u8 {' $L $L)" "$(TOP=2 row_docruns "$WORK/fx.scan")"
  # The D827 trap, controlled: a backslash before a backtick in GNU ERE matches nothing.
  check "ERE \\\` is dead (0), plain \` is live (1)" "0 1" \
    "$(printf '/// see [`Foo`]\n' | { grep -cE '\[\`' || true; } ) $(printf '/// see [`Foo`]\n' | grep -cE '\[`')"
  # Range mode: a two-commit repository. The unchanged `fn old` line and the production doc
  # link must not count; everything the head commit adds must.
  local g="$WORK/rg" here=$PWD
  mkdir -p "$g/crates/k/src" "$g/crates/k/tests" "$g/docs/design"
  cd "$g"; git init -q; gc() { git -c user.name=t -c user.email=t@t -c core.autocrlf=false commit -qm "$1"; }
  printf '# Index\n' > docs/architecture.md; printf 'The `fresh.rs` module, and a lonely word.\n' > docs/design/x.md
  printf '%s\n' 'fn old() -> u8 { Some(1).unwrap() }' '#[cfg(test)]' 'mod tests {' '    #[test]' '    fn gone() {}' '}' > crates/k/src/lib.rs
  printf '%s\n' 'fn a(v: &[u8]) -> u8 { v[0] }' '/// Kept, unchanged.' 'fn b() {}' > crates/k/src/keep.rs
  git add -A; gc base
  cat > crates/k/src/lib.rs <<'RS'
/// Mentions .unwrap(), #[allow(x)] and a TODO. See [`old`].
#[allow(dead_code)]
fn old() -> u8 { Some(1).unwrap() }
fn new() -> u8 { Some(2).expect("added") + [1u8][0] }
// TODO: added marker
#[cfg(test)]
mod tests {
    /// See [`super::new`].
    #[test]
    #[ignore]
    fn added() { Some(3).unwrap(); }
    #[test]
    fn second() {}
}
RS
  printf '/// [`x`]\n#[test]\nfn t() {}\n' > crates/k/tests/it.rs
  printf 'fn f() {}\n' > crates/k/src/fresh.rs; printf 'fn f() {}\n' > crates/k/src/lonely.rs
  git add -A; gc head
  range_join HEAD~1; added
  check "range panics: the added expect (P) and unwrap (T) only" "total P 1, T 1" "$(row_panics "$WORK/added" | tail -1)"
  check "range index: the added [1u8][0], not keep.rs's unchanged v[0]" "total P 1, T 0" "$(row_index "$WORK/added" | tail -1)"
  check "range docruns (MIN=0): the added run in mod tests, not keep.rs's unchanged one" \
    "$(printf '1\tcrates/k/src/lib.rs:8\tfn added() { Some(3).unwrap(); }')" "$(MIN=0 range_docruns)"
  check "range allow" "total P 1, T 0" "$(row_allow "$WORK/added" | tail -1)"
  check "range todo: the marker, not the mention" "total P 1, T 0" "$(row_todo "$WORK/added" | tail -1)"
  check "range ignore: the bare one" "total 1 (with reason 0, without 1)" "$(row_ignore "$WORK/added" | tail -1)"
  check "range testlinks: cfg(test) prose and tests/, not the production link" "total 2" "$(range_testlinks | tail -1)"
  check "range testdelta: lib.rs goes from one #[test] to two, tests/it.rs from none to one" "1 2 +1 0 1 +1" \
    "$(range_testdelta | awk '$1 ~ /^crates\/k\/(src\/lib|tests\/it)\.rs$/ { printf "%s%s %s %s", (n++ ? " " : ""), $4, $5, $6 }')"
  check "range testdelta: +/- lines agree with git's numstat" \
    "$(git diff --numstat HEAD~1 HEAD -- crates/k/src/lib.rs | awk '{ print $1, $2 }')" \
    "$(range_testdelta | awk '$1 == "crates/k/src/lib.rs" { print $2, $3 }')"
  check "range newmods: fresh.rs named in design/, lonely.rs nowhere" "fresh 1 lonely 0" \
    "$(range_newmods HEAD~1 | awk -F'\t' '{ f = $1; sub(/^.*\//, "", f); sub(/\.rs$/, "", f); split($3, d, " "); printf "%s%s %s", (NR > 1 ? " " : ""), f, d[2] }')"
  printf 'crates/k/src/lib.rs\n' > "$WORK/fx.changed"
  check "range clippy filter: the changed file (backslashed, twice), not ui.rs or panels/lib.rs" \
    'crates/k/src/lib.rs:1:1: warning: a' \
    "$(printf '%s\n' 'crates\k\src\lib.rs:1:1: warning: a' 'crates/k/src/lib.rs:1:1: warning: a' \
      'crates/k/src/ui.rs:1:1: warning: b' 'crates/k/src/panels/lib.rs:2:1: warning: c' | clippy_filter "$WORK/fx.changed")"
  # The same rows through the script itself, where `set -e` is live: a row that dies early
  # (a no-match grep under pipefail) returns a short answer that reads as a clean one.
  local r out=""
  for r in panics index allow todo ignore testlinks testdelta docruns newmods; do
    bash "$SELF" range HEAD~1 "$r" > /dev/null 2>&1 && out="$out $r" || out="$out $r=died"
  done
  check "every range row exits 0 when run as a command" " panics index allow todo ignore testlinks testdelta docruns newmods" "$out"
  out=""
  for r in panics index unsafe allow todo ignore docruns preview partialeq; do
    bash "$SELF" "$r" > /dev/null 2>&1 && out="$out $r" || out="$out $r=died"
  done
  check "every tree row exits 0 when run as a command" " panics index unsafe allow todo ignore docruns preview partialeq" "$out"
  cd "$here"
  [ $fail = 0 ] && echo "selftest passed" || { echo "selftest FAILED"; return 1; }
}

# --- dispatch -----------------------------------------------------------------------------------

REV=HEAD
if [ "${1:-}" = --rev ]; then REV=$2; shift 2; fi
cmd=${1:?usage: see the header of census.sh}; shift || true
case $cmd in
  selftest) selftest ;;
  scan) scan_dir "${1:?dir}" /dev/stdout ;;
  panics|index|unsafe|allow|todo|ignore|docruns|preview|partialeq)
    tree "$REV" "$WORK/t"; scan_dir "$WORK/t" "$WORK/t.scan"; "row_$cmd" "$WORK/t.scan" ;;
  range)
    base=${1:?base}; row=${2:?row}
    case $row in
      newmods) range_newmods "$base" ;;
      clippy) range_clippy "$base" ;;
      testlinks|testdelta|docruns) range_join "$base"; "range_$row" ;;
      panics|index|allow|todo|ignore) range_join "$base"; added; "row_$row" "$WORK/added" ;;
      *) echo "unknown range row: $row" >&2; exit 2 ;;
    esac ;;
  *) echo "unknown row: $cmd" >&2; exit 2 ;;
esac
