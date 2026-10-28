//! sqlmini — a small embedded SQL engine for the LingOS kernel.
//!
//! Goal: let programs written against SQLite (the Ling Wallet node) run
//! unchanged.  Supports the subset of SQL the node uses, with SQLite's
//! semantics for type affinity, comparison, arithmetic, NULL handling,
//! conflict resolution, rowids and `sqlite3_changes`.
//!
//! Only `core` + `alloc` are used, so the same file builds in the kernel
//! (`no_std`) and on a host test crate (`extern crate alloc;` +
//! `#[path = ".../sqlmini.rs"] mod sqlmini;`).
//!
//! Storage model: every table is a `BTreeMap<rowid, row>`; every PRIMARY KEY
//! (unless it is an INTEGER PRIMARY KEY rowid alias), UNIQUE constraint and
//! CREATE INDEX is a `BTreeSet<(key columns..., rowid)>` ordered with SQLite's
//! value ordering (NULL < numbers < text, numbers compared by value, text
//! bytewise).  The planner uses the rowid map / index prefixes for `col = expr`
//! and `col IN (SELECT ...)`, and walks the rowid map or an index in order to
//! satisfy ORDER BY (with early exit for LIMIT) when it can.
//!
//! Supported statements:
//!   CREATE TABLE [IF NOT EXISTS] t (col type [PRIMARY KEY [ASC|DESC] [AUTOINCREMENT]]
//!        [NOT NULL] [NULL] [UNIQUE] [DEFAULT literal] [COLLATE BINARY], ...,
//!        [PRIMARY KEY (a, b)], [UNIQUE (a, b)])
//!   CREATE [UNIQUE] INDEX [IF NOT EXISTS] i ON t (a, b)
//!   DROP TABLE [IF EXISTS] t
//!   INSERT [OR IGNORE|OR REPLACE|OR ABORT] INTO t [(cols)] VALUES (...), ... | DEFAULT VALUES
//!        [ON CONFLICT [(cols)] DO NOTHING | DO UPDATE SET c = expr, ... [WHERE expr]]
//!   REPLACE INTO ...
//!   UPDATE t SET c = expr, ... [WHERE expr]
//!   DELETE FROM t [WHERE expr]
//!   BEGIN [DEFERRED|IMMEDIATE|EXCLUSIVE] [TRANSACTION], COMMIT / END, ROLLBACK
//!   SELECT [DISTINCT] cols FROM t [alias] | (subquery) [alias]
//!        [[INNER|CROSS] JOIN ... [ON expr] | , ...] [WHERE] [GROUP BY [HAVING]]
//!        [ORDER BY expr [ASC|DESC], ...] [LIMIT n [OFFSET m | , m]]
//! Expressions: literals, ?NNN / ? / :name params, [t.]col, rowid/oid/_rowid_,
//!   OR AND NOT = == != <> < <= > >= IS [NOT] IS NULL ISNULL NOTNULL
//!   [NOT] IN (list|subquery) [NOT] BETWEEN, + - * / % ||, unary - +,
//!   scalar subqueries, EXISTS, COUNT/SUM/TOTAL/AVG/MIN/MAX [DISTINCT],
//!   abs coalesce ifnull nullif length lower upper typeof min/max(a, b, ...).
//! Anything else is rejected with `Err("sqlmini: unsupported ...")` (LEFT
//! JOIN, UNION, LIKE, CASE, CAST, window functions, bit operators, DESC or
//! partial indexes, non-BINARY collations, INSERT ... SELECT, RETURNING,
//! WITHOUT ROWID, CHECK / FOREIGN KEY) rather than being run with different
//! semantics.
//!
//! Durability (all integers little-endian; `str` = u32 byte length + UTF-8;
//! `value` = u8 tag 0 NULL | 1 i64 | 2 f64 bits as u64 | 3 str):
//!
//! * `snapshot()`: `"SQLMINI1"`, u32 table count, then per table (name order):
//!   str CREATE TABLE sql, u32 n + n × str CREATE INDEX sql, i64 AUTOINCREMENT
//!   sequence, u32 column count, u64 row count, rows as (i64 rowid, column
//!   values); finally u64 FNV-1a-64 of all preceding bytes.  Constraint indexes
//!   and CREATE INDEX contents are rebuilt by `restore()`.
//! * `take_log()`: a sequence of units, one per committed transaction (or per
//!   autocommit statement that changed something): `'U'`, u32 statement count,
//!   u32 body length, body, u32 FNV-1a-32 of the body.  The body is that many
//!   (str sql, u32 param count, params as values).  Statements of a rolled
//!   back transaction (or a failed statement) never reach the log; statements
//!   that changed nothing are omitted.  `replay()` applies each unit inside
//!   BEGIN/COMMIT and stops quietly at a torn (truncated / corrupt) final unit.
//!   Rowid assignment is deterministic (max rowid + 1), so replaying the log on
//!   the snapshot it started from reproduces the exact database.
//!
//! Known differences from SQLite (none reachable from the wallet node's SQL):
//! * numeric-looking TEXT converted to REAL by affinity uses Rust's correctly
//!   rounded parser; SQLite's own parser can be 1 ulp off at extreme exponents
//!   (e.g. '1e-300').  The node passes numbers as REAL parameters.
//! * SUM/AVG of reals add naively (SQLite uses Kahan-Babuska summation).
//! * bare columns in an aggregate query come from the group's last row (what
//!   SQLite does in practice), without SQLite's min()/max() special case.
//! * when the largest rowid is i64::MAX a new rowid is an error, not a random
//!   search.  A snapshot taken inside an open transaction includes its
//!   uncommitted changes (take snapshots between transactions).
#![allow(dead_code)]

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::cmp::Ordering;

// ════════════════════════════════════════════════════════════════════════
// Values
// ════════════════════════════════════════════════════════════════════════

#[derive(Clone, Debug, PartialEq)]
pub enum SqlVal {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
}

/// Column / expression affinity (SQLite's BLOB affinity == "no affinity" for
/// columns; `None` is the affinity of a plain expression).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Aff {
    None,
    Blob,
    Text,
    Numeric,
    Integer,
    Real,
}

impl Aff {
    fn is_num(self) -> bool {
        matches!(self, Aff::Numeric | Aff::Integer | Aff::Real)
    }
}

/// SQLite's affinity-from-declared-type rules (section 3.1 of datatype3).
fn decl_affinity(decl: &str) -> Aff {
    let u = decl.to_ascii_uppercase();
    if u.contains("INT") {
        Aff::Integer
    } else if u.contains("CHAR") || u.contains("CLOB") || u.contains("TEXT") {
        Aff::Text
    } else if u.contains("BLOB") || u.trim().is_empty() {
        Aff::Blob
    } else if u.contains("REAL") || u.contains("FLOA") || u.contains("DOUB") {
        Aff::Real
    } else {
        Aff::Numeric
    }
}

/// sqlite3CompareAffinity: affinity to apply to both operands of a comparison.
fn cmp_affinity(a: Aff, b: Aff) -> Aff {
    let ha = a != Aff::None;
    let hb = b != Aff::None;
    if ha && hb {
        if a.is_num() || b.is_num() {
            Aff::Numeric
        } else {
            Aff::Blob
        }
    } else if ha {
        a
    } else {
        b
    }
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// Longest numeric prefix of `b`: (length, is-integer-literal).
fn num_prefix(b: &[u8]) -> (usize, bool) {
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let ds = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut nd = i - ds;
    let mut is_int = true;
    if i < b.len() && b[i] == b'.' {
        let k = i + 1;
        let mut m = k;
        while m < b.len() && b[m].is_ascii_digit() {
            m += 1;
        }
        if nd + (m - k) > 0 {
            nd += m - k;
            i = m;
            is_int = false;
        }
    }
    if nd == 0 {
        return (0, false);
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut k = i + 1;
        if k < b.len() && (b[k] == b'+' || b[k] == b'-') {
            k += 1;
        }
        let ks = k;
        while k < b.len() && b[k].is_ascii_digit() {
            k += 1;
        }
        if k > ks {
            i = k;
            is_int = false;
        }
    }
    (i, is_int)
}

fn trim_space(s: &str) -> &str {
    let b = s.as_bytes();
    let mut i = 0;
    let mut j = b.len();
    while i < j && is_space(b[i]) {
        i += 1;
    }
    while j > i && is_space(b[j - 1]) {
        j -= 1;
    }
    s.get(i..j).unwrap_or("")
}

/// Whole-string numeric parse (text that "looks like a number").
fn parse_num_full(s: &str) -> Option<SqlVal> {
    let t = trim_space(s);
    let (len, is_int) = num_prefix(t.as_bytes());
    if len == 0 || len != t.len() {
        return None;
    }
    if is_int {
        if let Ok(v) = t.parse::<i64>() {
            return Some(SqlVal::Int(v));
        }
    }
    match t.parse::<f64>() {
        Ok(r) if !r.is_nan() => Some(SqlVal::Real(r)),
        _ => None,
    }
}

/// Numeric value of text for arithmetic (longest prefix; no digits -> 0).
fn text_to_num(s: &str) -> SqlVal {
    let t = trim_space(s);
    let (len, is_int) = num_prefix(t.as_bytes());
    if len == 0 {
        return SqlVal::Int(0);
    }
    let p = t.get(..len).unwrap_or("0");
    if is_int {
        if let Ok(v) = p.parse::<i64>() {
            return SqlVal::Int(v);
        }
    }
    match p.parse::<f64>() {
        Ok(r) if !r.is_nan() => SqlVal::Real(r),
        _ => SqlVal::Int(0),
    }
}

fn text_to_real(s: &str) -> f64 {
    match text_to_num(s) {
        SqlVal::Int(i) => i as f64,
        SqlVal::Real(r) => r,
        _ => 0.0,
    }
}

/// sqlite3VdbeIntegerAffinity condition: a real that is exactly an i64.
fn real_exact_int(r: f64) -> Option<i64> {
    if !(r > -9223372036854775808.0 && r < 9223372036854775808.0) {
        return None;
    }
    let i = r as i64;
    if (i as f64) == r && i > i64::MIN && i < i64::MAX {
        Some(i)
    } else {
        None
    }
}

/// Dekker double-double multiply (port of SQLite's dekkerMul2).
fn dekker_mul2(x: &mut [f64; 2], y: f64, yy: f64) {
    let hx = f64::from_bits(x[0].to_bits() & 0xffff_ffff_fc00_0000);
    let tx = x[0] - hx;
    let hy = f64::from_bits(y.to_bits() & 0xffff_ffff_fc00_0000);
    let ty = y - hy;
    let p = hx * hy;
    let q = hx * ty + tx * hy;
    let c = p + q;
    let mut cc = p - c + q + tx * ty;
    cc = x[0] * yy + x[1] * y + cc;
    x[0] = c + cc;
    x[1] = c - x[0];
    x[1] += cc;
}

/// Port of SQLite's sqlite3FpDecode(r, iRound, mxRound) for a finite r > 0:
/// returns (significant digits without trailing zeros, decimal point position).
fn fp_decode(r: f64, iround: usize, mxround: usize) -> (Vec<u8>, i32) {
    let mut rr = [r, 0.0f64];
    let mut exp: i32 = 0;
    if rr[0] > 9.223372036854774784e+18 {
        while rr[0] > 9.223372036854774784e+118 {
            exp += 100;
            dekker_mul2(&mut rr, 1.0e-100, -1.99918998026028836196e-117);
        }
        while rr[0] > 9.223372036854774784e+28 {
            exp += 10;
            dekker_mul2(&mut rr, 1.0e-10, -3.6432197315497741579e-27);
        }
        while rr[0] > 9.223372036854774784e+18 {
            exp += 1;
            dekker_mul2(&mut rr, 1.0e-01, -5.5511151231257827021e-18);
        }
    } else {
        while rr[0] < 9.223372036854774784e-83 {
            exp -= 100;
            dekker_mul2(&mut rr, 1.0e+100, -1.5902891109759918046e+83);
        }
        while rr[0] < 9.223372036854774784e+07 {
            exp -= 10;
            dekker_mul2(&mut rr, 1.0e+10, 0.0);
        }
        while rr[0] < 9.22337203685477478e+17 {
            exp -= 1;
            dekker_mul2(&mut rr, 1.0e+01, 0.0);
        }
    }
    let mut v: u64 = if rr[1] < 0.0 {
        (rr[0] as u64).wrapping_sub((-rr[1]) as u64)
    } else {
        (rr[0] as u64).wrapping_add(rr[1] as u64)
    };
    let mut d: Vec<u8> = Vec::with_capacity(24);
    if v == 0 {
        d.push(0);
    }
    while v > 0 {
        d.push((v % 10) as u8);
        v /= 10;
    }
    d.reverse();
    let mut idp = d.len() as i32 + exp;
    let n = d.len();
    if iround > 0 && (iround < n || n > mxround) {
        let ir = iround.min(mxround);
        let up = d.get(ir).map_or(false, |&x| x >= 5);
        d.truncate(ir);
        if up {
            let mut j = ir;
            loop {
                if j == 0 {
                    d.insert(0, 1);
                    idp += 1;
                    break;
                }
                j -= 1;
                d[j] += 1;
                if d[j] <= 9 {
                    break;
                }
                d[j] = 0;
            }
        }
    }
    while d.len() > 1 && d.last() == Some(&0) {
        d.pop();
    }
    (d, idp)
}

/// SQLite renders reals with "%!.15g" (digit generation ported exactly).
fn real_to_text(r: f64) -> String {
    if r.is_nan() {
        return String::from("NaN");
    }
    if r.is_infinite() {
        return String::from(if r > 0.0 { "Inf" } else { "-Inf" });
    }
    if r == 0.0 {
        return String::from("0.0"); // also -0.0 (SQLite prints no sign)
    }
    let neg = r < 0.0;
    let (dg, idp) = fp_decode(if neg { -r } else { r }, 15, 26);
    let digits: String = dg.iter().map(|&x| (b'0' + x) as char).collect();
    let digits = digits.as_str();
    let exp = idp - 1;
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if exp < -4 || exp > 14 {
        out.push_str(digits.get(..1).unwrap_or("0"));
        out.push('.');
        match digits.get(1..) {
            Some(rest) if !rest.is_empty() => out.push_str(rest),
            _ => out.push('0'),
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        let ae = exp.unsigned_abs();
        if ae < 10 {
            out.push('0');
        }
        out.push_str(&ae.to_string());
    } else if exp >= 0 {
        let e = exp as usize;
        if digits.len() > e + 1 {
            out.push_str(digits.get(..=e).unwrap_or(""));
            out.push('.');
            out.push_str(digits.get(e + 1..).unwrap_or(""));
        } else {
            out.push_str(digits);
            for _ in digits.len()..e + 1 {
                out.push('0');
            }
            out.push_str(".0");
        }
    } else {
        out.push_str("0.");
        for _ in 0..(-exp - 1) {
            out.push('0');
        }
        out.push_str(digits);
    }
    out
}

fn val_to_text(v: &SqlVal) -> String {
    match v {
        SqlVal::Null => String::new(),
        SqlVal::Int(i) => i.to_string(),
        SqlVal::Real(r) => real_to_text(*r),
        SqlVal::Text(s) => s.clone(),
    }
}

/// Column affinity applied when a value is stored.
fn apply_col_aff(a: Aff, v: SqlVal) -> SqlVal {
    match a {
        Aff::Text => match v {
            SqlVal::Int(_) | SqlVal::Real(_) => SqlVal::Text(val_to_text(&v)),
            o => o,
        },
        Aff::Numeric | Aff::Integer => match v {
            SqlVal::Text(s) => match parse_num_full(&s) {
                Some(SqlVal::Real(r)) => match real_exact_int(r) {
                    Some(i) => SqlVal::Int(i),
                    None => SqlVal::Real(r),
                },
                Some(n) => n,
                None => SqlVal::Text(s),
            },
            SqlVal::Real(r) => match real_exact_int(r) {
                Some(i) => SqlVal::Int(i),
                None => SqlVal::Real(r),
            },
            o => o,
        },
        Aff::Real => match v {
            SqlVal::Text(s) => match parse_num_full(&s) {
                Some(SqlVal::Int(i)) => SqlVal::Real(i as f64),
                Some(n) => n,
                None => SqlVal::Text(s),
            },
            SqlVal::Int(i) => SqlVal::Real(i as f64),
            // SQLite stores integral reals as integers and reads them back
            // as (double)i, which turns -0.0 into 0.0.
            SqlVal::Real(r) if r == 0.0 => SqlVal::Real(0.0),
            o => o,
        },
        Aff::Blob | Aff::None => v,
    }
}

/// Affinity applied to comparison operands.
fn apply_cmp_aff(a: Aff, v: SqlVal) -> SqlVal {
    match a {
        Aff::Numeric | Aff::Integer | Aff::Real => match v {
            SqlVal::Text(s) => match parse_num_full(&s) {
                Some(n) => n,
                None => SqlVal::Text(s),
            },
            o => o,
        },
        Aff::Text => match v {
            SqlVal::Int(_) | SqlVal::Real(_) => SqlVal::Text(val_to_text(&v)),
            o => o,
        },
        _ => v,
    }
}

fn int_real_cmp(i: i64, r: f64) -> Ordering {
    if r.is_nan() {
        return Ordering::Greater;
    }
    if r < -9223372036854775808.0 {
        return Ordering::Greater;
    }
    if r >= 9223372036854775808.0 {
        return Ordering::Less;
    }
    let y = r as i64;
    match i.cmp(&y) {
        Ordering::Equal => {
            let f = r - (y as f64);
            if f > 0.0 {
                Ordering::Less
            } else if f < 0.0 {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        }
        o => o,
    }
}

fn val_rank(v: &SqlVal) -> u8 {
    match v {
        SqlVal::Null => 0,
        SqlVal::Int(_) | SqlVal::Real(_) => 1,
        SqlVal::Text(_) => 2,
    }
}

/// SQLite's total value ordering with BINARY collation.
fn cmp_vals(a: &SqlVal, b: &SqlVal) -> Ordering {
    match (a, b) {
        (SqlVal::Int(x), SqlVal::Int(y)) => x.cmp(y),
        (SqlVal::Int(x), SqlVal::Real(y)) => int_real_cmp(*x, *y),
        (SqlVal::Real(x), SqlVal::Int(y)) => int_real_cmp(*y, *x).reverse(),
        (SqlVal::Real(x), SqlVal::Real(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (SqlVal::Text(x), SqlVal::Text(y)) => x.as_bytes().cmp(y.as_bytes()),
        _ => val_rank(a).cmp(&val_rank(b)),
    }
}

fn truthy(v: &SqlVal) -> Option<bool> {
    match v {
        SqlVal::Null => None,
        SqlVal::Int(i) => Some(*i != 0),
        SqlVal::Real(r) => Some(*r != 0.0),
        SqlVal::Text(s) => Some(text_to_real(s) != 0.0),
    }
}

fn to_num(v: SqlVal) -> SqlVal {
    match v {
        SqlVal::Text(s) => text_to_num(&s),
        o => o,
    }
}

fn num_f64(v: &SqlVal) -> f64 {
    match v {
        SqlVal::Int(i) => *i as f64,
        SqlVal::Real(r) => *r,
        SqlVal::Text(s) => text_to_real(s),
        SqlVal::Null => 0.0,
    }
}

/// OP_MustBeInt (used for LIMIT / OFFSET).
fn must_int(v: SqlVal) -> Result<i64, String> {
    match apply_col_aff(Aff::Integer, v) {
        SqlVal::Int(i) => Ok(i),
        _ => Err(String::from("datatype mismatch")),
    }
}

fn norm_param(v: &SqlVal) -> SqlVal {
    match v {
        SqlVal::Real(r) if r.is_nan() => SqlVal::Null,
        o => o.clone(),
    }
}

// ── index / group keys ────────────────────────────────────────────────────

#[derive(Clone, Debug)]
enum KeyV {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    /// Sorts before every value (lower bound of a prefix range).
    Min,
    /// Sorts after every value (upper bound of a prefix range).
    Max,
}

impl KeyV {
    fn of(v: &SqlVal) -> KeyV {
        match v {
            SqlVal::Null => KeyV::Null,
            SqlVal::Int(i) => KeyV::Int(*i),
            SqlVal::Real(r) if r.is_nan() => KeyV::Null,
            SqlVal::Real(r) => KeyV::Real(*r),
            SqlVal::Text(s) => KeyV::Text(s.clone()),
        }
    }
    fn val(&self) -> SqlVal {
        match self {
            KeyV::Null | KeyV::Min | KeyV::Max => SqlVal::Null,
            KeyV::Int(i) => SqlVal::Int(*i),
            KeyV::Real(r) => SqlVal::Real(*r),
            KeyV::Text(s) => SqlVal::Text(s.clone()),
        }
    }
    fn rank(&self) -> u8 {
        match self {
            KeyV::Min => 0,
            KeyV::Null => 1,
            KeyV::Int(_) | KeyV::Real(_) => 2,
            KeyV::Text(_) => 3,
            KeyV::Max => 4,
        }
    }
}

impl Ord for KeyV {
    fn cmp(&self, o: &KeyV) -> Ordering {
        match (self, o) {
            (KeyV::Int(x), KeyV::Int(y)) => x.cmp(y),
            (KeyV::Int(x), KeyV::Real(y)) => int_real_cmp(*x, *y),
            (KeyV::Real(x), KeyV::Int(y)) => int_real_cmp(*y, *x).reverse(),
            (KeyV::Real(x), KeyV::Real(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
            (KeyV::Text(x), KeyV::Text(y)) => x.as_bytes().cmp(y.as_bytes()),
            _ => self.rank().cmp(&o.rank()),
        }
    }
}
impl PartialOrd for KeyV {
    fn partial_cmp(&self, o: &KeyV) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl PartialEq for KeyV {
    fn eq(&self, o: &KeyV) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}
impl Eq for KeyV {}

/// Index entry: key columns (first two inline, the rest boxed) + rowid.
/// Every entry of one index has the same shape; columns past the index's
/// width are Null.
#[derive(Clone, Debug)]
struct IKey {
    a: KeyV,
    b: KeyV,
    rest: Box<[KeyV]>,
    rid: i64,
}

impl IKey {
    fn new(mut v: Vec<KeyV>, rid: i64) -> IKey {
        let rest: Box<[KeyV]> = if v.len() > 2 { v.split_off(2).into_boxed_slice() } else { Box::new([]) };
        let mut it = v.into_iter();
        let a = it.next().unwrap_or(KeyV::Null);
        let b = it.next().unwrap_or(KeyV::Null);
        IKey { a, b, rest, rid }
    }
    /// Inclusive range covering every entry whose first columns equal `prefix`
    /// in an index of width `m`.
    fn bounds(prefix: Vec<KeyV>, m: usize) -> (IKey, IKey) {
        let mut lo = prefix;
        let mut hi = lo.clone();
        while lo.len() < m {
            lo.push(KeyV::Min);
            hi.push(KeyV::Max);
        }
        (IKey::new(lo, i64::MIN), IKey::new(hi, i64::MAX))
    }
}

impl Ord for IKey {
    fn cmp(&self, o: &IKey) -> Ordering {
        self.a
            .cmp(&o.a)
            .then_with(|| self.b.cmp(&o.b))
            .then_with(|| self.rest.cmp(&o.rest))
            .then_with(|| self.rid.cmp(&o.rid))
    }
}
impl PartialOrd for IKey {
    fn partial_cmp(&self, o: &IKey) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl PartialEq for IKey {
    fn eq(&self, o: &IKey) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}
impl Eq for IKey {}

// ════════════════════════════════════════════════════════════════════════
// Lexer
// ════════════════════════════════════════════════════════════════════════

#[derive(Clone, Debug, PartialEq)]
enum T {
    Id(String),
    QId(String),
    Str(String),
    Num(String),
    /// `?NNN` (Some) or `?` (None)
    Param(Option<u32>),
    NParam(String),
    Sym(&'static str),
    End,
}

#[derive(Clone, Debug)]
struct Tok {
    t: T,
    s: usize,
    e: usize,
}

const SYMS: [&str; 24] = [
    "||", "<=", ">=", "==", "!=", "<>", "<<", ">>", "(", ")", ",", ".", ";", "*", "+", "-", "/",
    "%", "=", "<", ">", "&", "|", "~",
];

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    let sub = |a: usize, z: usize| -> String { src.get(a..z).unwrap_or("").to_string() };
    while i < b.len() {
        let c = b[i];
        if is_space(c) {
            i += 1;
            continue;
        }
        if c == b'-' && b.get(i + 1) == Some(&b'-') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            loop {
                if i + 1 >= b.len() {
                    i = b.len();
                    break;
                }
                if b[i] == b'*' && b[i + 1] == b'/' {
                    i += 2;
                    break;
                }
                i += 1;
            }
            continue;
        }
        let s = i;
        if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$' || b[i] >= 0x80) {
                i += 1;
            }
            out.push(Tok { t: T::Id(sub(s, i)), s, e: i });
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).map_or(false, |d| d.is_ascii_digit())) {
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i < b.len() && b[i] == b'.' {
                i += 1;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                let mut k = i + 1;
                if k < b.len() && (b[k] == b'+' || b[k] == b'-') {
                    k += 1;
                }
                if k < b.len() && b[k].is_ascii_digit() {
                    while k < b.len() && b[k].is_ascii_digit() {
                        k += 1;
                    }
                    i = k;
                } else {
                    return Err(format!("unrecognized token: \"{}\"", sub(s, k)));
                }
            }
            if i < b.len() && (b[i].is_ascii_alphabetic() || b[i] == b'_') {
                return Err(format!("unrecognized token: \"{}\"", sub(s, i + 1)));
            }
            out.push(Tok { t: T::Num(sub(s, i)), s, e: i });
        } else if c == b'\'' {
            let mut v = String::new();
            i += 1;
            let mut a = i;
            loop {
                if i >= b.len() {
                    return Err(String::from("unrecognized token: unterminated string"));
                }
                if b[i] == b'\'' {
                    v.push_str(src.get(a..i).unwrap_or(""));
                    if b.get(i + 1) == Some(&b'\'') {
                        v.push('\'');
                        i += 2;
                        a = i;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push(Tok { t: T::Str(v), s, e: i });
        } else if c == b'"' || c == b'`' || c == b'[' {
            let close = if c == b'[' { b']' } else { c };
            let mut v = String::new();
            i += 1;
            let mut a = i;
            loop {
                if i >= b.len() {
                    return Err(String::from("unrecognized token: unterminated identifier"));
                }
                if b[i] == close {
                    v.push_str(src.get(a..i).unwrap_or(""));
                    if close != b']' && b.get(i + 1) == Some(&close) {
                        v.push(close as char);
                        i += 2;
                        a = i;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push(Tok { t: T::QId(v), s, e: i });
        } else if c == b'?' {
            i += 1;
            let ds = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i > ds {
                let n: u32 = src.get(ds..i).unwrap_or("").parse().map_err(|_| String::from("variable number out of range"))?;
                if n == 0 || n > 32766 {
                    return Err(String::from("variable number must be between ?1 and ?32766"));
                }
                out.push(Tok { t: T::Param(Some(n)), s, e: i });
            } else {
                out.push(Tok { t: T::Param(None), s, e: i });
            }
        } else if c == b':' || c == b'@' || c == b'$' {
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] >= 0x80) {
                i += 1;
            }
            if i == s + 1 {
                return Err(format!("unrecognized token: \"{}\"", c as char));
            }
            out.push(Tok { t: T::NParam(sub(s, i)), s, e: i });
        } else {
            let mut found = None;
            for sy in SYMS.iter() {
                if b[i..].starts_with(sy.as_bytes()) {
                    found = Some(*sy);
                    break;
                }
            }
            match found {
                Some(sy) => {
                    i += sy.len();
                    out.push(Tok { t: T::Sym(sy), s, e: i });
                }
                None => return Err(format!("unrecognized token: \"{}\"", sub(s, s + 1))),
            }
        }
    }
    out.push(Tok { t: T::End, s: b.len(), e: b.len() });
    Ok(out)
}

// ════════════════════════════════════════════════════════════════════════
// AST
// ════════════════════════════════════════════════════════════════════════

const ROWID: u32 = u32::MAX;
/// Max nesting of parentheses / subqueries / unary operators.
const MAX_DEPTH: u32 = 64;
/// Max depth of any expression tree (left-deep operator chains count too),
/// bounding the recursion of compile/eval on small kernel stacks.
const MAX_TREE: u32 = 400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Is,
    IsNot,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Concat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UOp {
    Neg,
    Pos,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SubKind {
    Scalar,
    Exists,
    In,
}

#[derive(Clone, Debug)]
struct SubQ {
    kind: SubKind,
    sel: Select,
    lhs: Option<Ex>,
    not: bool,
    id: u32,
    corr: bool,
    aff: Aff,
}

#[derive(Clone, Debug)]
enum Ex {
    Lit(SqlVal),
    Param(usize),
    Name(Option<String>, String),
    Col { up: u32, src: u32, col: u32, aff: Aff },
    Un(UOp, Box<Ex>),
    Bin(BOp, Box<Ex>, Box<Ex>),
    Between(Box<Ex>, Box<Ex>, Box<Ex>, bool),
    InList(Box<Ex>, Vec<Ex>, bool),
    Sub(Box<SubQ>),
    Func(String, Vec<Ex>, bool, bool),
    Agg(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AggF {
    Count,
    Sum,
    Total,
    Avg,
    Min,
    Max,
}

#[derive(Clone, Debug)]
struct AggDef {
    f: AggF,
    arg: Option<Ex>,
    distinct: bool,
}

#[derive(Clone, Debug)]
struct RCol {
    ex: Ex,
    star: Option<Option<String>>,
    alias: Option<String>,
    span: String,
}

#[derive(Clone, Debug)]
struct FromItem {
    /// Table key (lower-case) after compile.
    name: Option<String>,
    sub: Option<Box<Select>>,
    alias: Option<String>,
    on: Option<Ex>,
}

#[derive(Clone, Debug)]
enum Access {
    Scan,
    Rowid(Ex, Aff),
    Index(usize, Vec<(Ex, Aff)>),
    RowidIn(Ex),
    IndexIn(usize, Ex),
}

#[derive(Clone, Debug)]
struct Level {
    access: Access,
    conds: Vec<Ex>,
    need_row: bool,
    reverse: bool,
}

#[derive(Clone, Debug, Default)]
struct Select {
    distinct: bool,
    cols: Vec<RCol>,
    from: Vec<FromItem>,
    where_: Option<Ex>,
    group: Vec<Ex>,
    having: Option<Ex>,
    order: Vec<(Ex, bool)>,
    limit: Option<Ex>,
    offset: Option<Ex>,
    // ── compiled ──
    names: Vec<String>,
    affs: Vec<Aff>,
    aggs: Vec<AggDef>,
    levels: Vec<Level>,
    pre: Vec<Ex>,
    is_agg: bool,
    sort: bool,
    needs_rep: bool,
}

#[derive(Clone, Debug)]
struct ColAst {
    name: String,
    decl: String,
    notnull: bool,
    default: Option<SqlVal>,
    pk: bool,
    pk_desc: bool,
    autoinc: bool,
    unique: bool,
}

#[derive(Clone, Debug)]
struct CreateTable {
    name: String,
    ine: bool,
    cols: Vec<ColAst>,
    pk: Vec<String>,
    uniques: Vec<Vec<String>>,
    sql: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Conf {
    Abort,
    Ignore,
    Replace,
}

#[derive(Clone, Debug)]
enum Target {
    Rowid,
    Index(usize),
}

#[derive(Clone, Debug)]
struct Upsert {
    target_names: Vec<String>,
    target: Option<Target>,
    /// None = DO NOTHING
    sets: Option<Vec<(String, Ex)>>,
    sets_c: Vec<(u32, Ex)>,
    where_: Option<Ex>,
}

#[derive(Clone, Debug)]
struct Insert {
    table: String,
    alias: Option<String>,
    mode: Conf,
    col_names: Option<Vec<String>>,
    cols: Vec<u32>,
    rows: Vec<Vec<Ex>>,
    upsert: Option<Upsert>,
}

#[derive(Clone, Debug)]
enum Stmt {
    Select(Select),
    CreateTable(CreateTable),
    CreateIndex { name: String, table: String, cols: Vec<String>, unique: bool, ine: bool, sql: String },
    DropTable { name: String, if_exists: bool },
    Insert(Insert),
    Delete { table: String, sel: Select },
    Update { table: String, sets: Vec<(String, Ex)>, set_cols: Vec<u32>, sel: Select },
    Begin,
    Commit,
    Rollback,
}

// ════════════════════════════════════════════════════════════════════════
// Parser
// ════════════════════════════════════════════════════════════════════════

const RESERVED: [&str; 56] = [
    "ALL", "AND", "AS", "ASC", "BETWEEN", "BY", "CASE", "CROSS", "DESC", "DISTINCT", "DO", "ELSE",
    "END", "ESCAPE", "EXCEPT", "EXISTS", "FROM", "FULL", "GLOB", "GROUP", "HAVING", "IN", "INNER",
    "INTERSECT", "IS", "ISNULL", "JOIN", "LEFT", "LIKE", "LIMIT", "NATURAL", "NOT", "NOTNULL",
    "NULL", "OFFSET", "ON", "OR", "ORDER", "OUTER", "RIGHT", "SELECT", "SET", "THEN", "UNION",
    "USING", "VALUES", "WHEN", "WHERE", "WINDOW", "RETURNING", "COLLATE", "NULLS", "INDEXED",
    "REGEXP", "MATCH", "FILTER",
];

fn is_reserved(s: &str) -> bool {
    RESERVED.iter().any(|k| k.eq_ignore_ascii_case(s))
}

const COL_CONSTRAINT_KW: [&str; 11] = [
    "CONSTRAINT", "PRIMARY", "NOT", "NULL", "UNIQUE", "CHECK", "DEFAULT", "COLLATE", "REFERENCES",
    "GENERATED", "AS",
];

struct Parser<'s> {
    src: &'s str,
    t: Vec<Tok>,
    i: usize,
    depth: u32,
    tree: u32,
    maxp: usize,
    named: Vec<String>,
    named_idx: Vec<usize>,
}

fn eqi(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

impl<'s> Parser<'s> {
    fn tok(&self, k: usize) -> &Tok {
        let n = self.t.len();
        &self.t[(self.i + k).min(n - 1)]
    }
    fn peek(&self) -> &T {
        &self.tok(0).t
    }
    fn bump(&mut self) -> T {
        let t = self.peek().clone();
        if self.i < self.t.len() - 1 {
            self.i += 1;
        }
        t
    }
    fn is_kw_at(&self, k: usize, w: &str) -> bool {
        matches!(&self.tok(k).t, T::Id(s) if eqi(s, w))
    }
    fn is_kw(&self, w: &str) -> bool {
        self.is_kw_at(0, w)
    }
    fn eat_kw(&mut self, w: &str) -> bool {
        if self.is_kw(w) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn err(&self) -> String {
        let tk = self.tok(0);
        if tk.t == T::End {
            String::from("incomplete input")
        } else {
            format!("near \"{}\": syntax error", self.src.get(tk.s..tk.e).unwrap_or("?"))
        }
    }
    fn want_kw(&mut self, w: &str) -> Result<(), String> {
        if self.eat_kw(w) {
            Ok(())
        } else {
            Err(self.err())
        }
    }
    fn is_sym(&self, s: &str) -> bool {
        matches!(self.peek(), T::Sym(x) if *x == s)
    }
    fn eat_sym(&mut self, s: &str) -> bool {
        if self.is_sym(s) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn want_sym(&mut self, s: &str) -> Result<(), String> {
        if self.eat_sym(s) {
            Ok(())
        } else {
            Err(self.err())
        }
    }
    fn name(&mut self) -> Result<String, String> {
        match self.peek().clone() {
            T::Id(s) if !is_reserved(&s) => {
                self.bump();
                Ok(s)
            }
            T::QId(s) => {
                self.bump();
                Ok(s)
            }
            T::Str(s) => {
                self.bump();
                Ok(s)
            }
            _ => Err(self.err()),
        }
    }
    /// Optional alias after a table / result column.
    fn opt_alias(&mut self) -> Result<Option<String>, String> {
        if self.eat_kw("AS") {
            return Ok(Some(self.name()?));
        }
        match self.peek().clone() {
            T::Id(s) if !is_reserved(&s) => {
                self.bump();
                Ok(Some(s))
            }
            T::QId(s) | T::Str(s) => {
                self.bump();
                Ok(Some(s))
            }
            _ => Ok(None),
        }
    }
    /// One more level of expression tree.
    fn grow(&mut self) -> Result<(), String> {
        self.tree += 1;
        if self.tree > MAX_TREE {
            return Err(String::from("expression tree is too large (maximum depth 400)"));
        }
        Ok(())
    }

    fn unsupported(&self, what: &str) -> String {
        format!("sqlmini: unsupported {}", what)
    }

    fn stmt(&mut self) -> Result<Stmt, String> {
        let st = if self.is_kw("SELECT") {
            Stmt::Select(self.select()?)
        } else if self.eat_kw("CREATE") {
            self.create()?
        } else if self.is_kw("INSERT") || self.is_kw("REPLACE") {
            Stmt::Insert(self.insert()?)
        } else if self.eat_kw("DELETE") {
            self.want_kw("FROM")?;
            let table = self.name()?;
            let where_ = if self.eat_kw("WHERE") { Some(self.expr()?) } else { None };
            let sel = Select {
                from: vec![FromItem { name: Some(table.clone()), sub: None, alias: None, on: None }],
                where_,
                ..Default::default()
            };
            Stmt::Delete { table, sel }
        } else if self.eat_kw("UPDATE") {
            if self.eat_kw("OR") {
                if !self.eat_kw("ABORT") {
                    return Err(self.unsupported("UPDATE OR conflict mode"));
                }
            }
            let table = self.name()?;
            self.want_kw("SET")?;
            let mut sets = Vec::new();
            loop {
                let c = self.name()?;
                self.want_sym("=")?;
                let e = self.expr()?;
                sets.push((c, e));
                if !self.eat_sym(",") {
                    break;
                }
            }
            let where_ = if self.eat_kw("WHERE") { Some(self.expr()?) } else { None };
            let sel = Select {
                from: vec![FromItem { name: Some(table.clone()), sub: None, alias: None, on: None }],
                where_,
                ..Default::default()
            };
            Stmt::Update { table, sets, set_cols: Vec::new(), sel }
        } else if self.eat_kw("BEGIN") {
            let _ = self.eat_kw("DEFERRED") || self.eat_kw("IMMEDIATE") || self.eat_kw("EXCLUSIVE");
            if self.eat_kw("TRANSACTION") {
                if let T::Id(_) = self.peek() {
                    self.bump();
                }
            }
            Stmt::Begin
        } else if self.eat_kw("COMMIT") || self.eat_kw("END") {
            let _ = self.eat_kw("TRANSACTION");
            Stmt::Commit
        } else if self.eat_kw("ROLLBACK") {
            let _ = self.eat_kw("TRANSACTION");
            if self.is_kw("TO") {
                return Err(self.unsupported("ROLLBACK TO"));
            }
            Stmt::Rollback
        } else if self.eat_kw("DROP") {
            if !self.eat_kw("TABLE") {
                return Err(self.unsupported("DROP"));
            }
            let if_exists = if self.eat_kw("IF") {
                self.want_kw("EXISTS")?;
                true
            } else {
                false
            };
            Stmt::DropTable { name: self.name()?, if_exists }
        } else {
            return Err(self.err());
        };
        while self.eat_sym(";") {}
        if *self.peek() != T::End {
            return Err(self.err());
        }
        Ok(st)
    }

    fn create(&mut self) -> Result<Stmt, String> {
        let _ = self.eat_kw("TEMP") || self.eat_kw("TEMPORARY");
        let unique = self.eat_kw("UNIQUE");
        if self.eat_kw("INDEX") {
            let ine = self.if_not_exists()?;
            let name = self.name()?;
            self.want_kw("ON")?;
            let table = self.name()?;
            let cols = self.name_list(true)?;
            if self.is_kw("WHERE") {
                return Err(self.unsupported("partial index"));
            }
            return Ok(Stmt::CreateIndex { name, table, cols, unique, ine, sql: self.src.to_string() });
        }
        if unique {
            return Err(self.err());
        }
        self.want_kw("TABLE")?;
        let ine = self.if_not_exists()?;
        let name = self.name()?;
        if self.is_kw("AS") {
            return Err(self.unsupported("CREATE TABLE AS"));
        }
        self.want_sym("(")?;
        let mut ct = CreateTable { name, ine, cols: Vec::new(), pk: Vec::new(), uniques: Vec::new(), sql: self.src.to_string() };
        loop {
            if self.eat_kw("CONSTRAINT") {
                self.name()?;
            }
            if self.eat_kw("PRIMARY") {
                self.want_kw("KEY")?;
                if !ct.pk.is_empty() || ct.cols.iter().any(|c| c.pk) {
                    return Err(format!("table \"{}\" has more than one primary key", ct.name));
                }
                ct.pk = self.name_list(false)?;
                self.no_conflict_clause()?;
            } else if self.eat_kw("UNIQUE") {
                let l = self.name_list(false)?;
                self.no_conflict_clause()?;
                ct.uniques.push(l);
            } else if self.is_kw("CHECK") || self.is_kw("FOREIGN") {
                return Err(self.unsupported("table constraint"));
            } else {
                let c = self.coldef()?;
                if c.pk && (!ct.pk.is_empty() || ct.cols.iter().any(|x| x.pk)) {
                    return Err(format!("table \"{}\" has more than one primary key", ct.name));
                }
                ct.cols.push(c);
            }
            if self.eat_sym(",") {
                continue;
            }
            self.want_sym(")")?;
            break;
        }
        if self.is_kw("WITHOUT") || self.is_kw("STRICT") {
            return Err(self.unsupported("table option"));
        }
        if ct.cols.is_empty() {
            return Err(self.err());
        }
        Ok(Stmt::CreateTable(ct))
    }

    fn if_not_exists(&mut self) -> Result<bool, String> {
        if self.eat_kw("IF") {
            self.want_kw("NOT")?;
            self.want_kw("EXISTS")?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn no_conflict_clause(&mut self) -> Result<(), String> {
        if self.is_kw("ON") && self.is_kw_at(1, "CONFLICT") {
            return Err(self.unsupported("constraint ON CONFLICT clause"));
        }
        Ok(())
    }

    /// `( a [COLLATE BINARY] [ASC|DESC], ... )`
    fn name_list(&mut self, index: bool) -> Result<Vec<String>, String> {
        self.want_sym("(")?;
        let mut v = Vec::new();
        loop {
            v.push(self.name()?);
            if self.eat_kw("COLLATE") {
                let c = self.name()?;
                if !eqi(&c, "BINARY") {
                    return Err(self.unsupported("collation"));
                }
            }
            if self.eat_kw("DESC") {
                if index {
                    return Err(self.unsupported("DESC index column"));
                }
            } else {
                let _ = self.eat_kw("ASC");
            }
            if !self.eat_sym(",") {
                break;
            }
        }
        self.want_sym(")")?;
        Ok(v)
    }

    fn coldef(&mut self) -> Result<ColAst, String> {
        let name = self.name()?;
        let mut decl = String::new();
        while let T::Id(s) = self.peek().clone() {
            if COL_CONSTRAINT_KW.iter().any(|k| eqi(k, &s)) {
                break;
            }
            if !decl.is_empty() {
                decl.push(' ');
            }
            decl.push_str(&s);
            self.bump();
        }
        if self.is_sym("(") {
            let s = self.tok(0).s;
            let mut depth = 0i32;
            loop {
                match self.bump() {
                    T::Sym("(") => depth += 1,
                    T::Sym(")") => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    T::End => return Err(self.err()),
                    _ => {}
                }
            }
            let e = self.t.get(self.i.saturating_sub(1)).map_or(s, |t| t.e);
            decl.push_str(self.src.get(s..e).unwrap_or(""));
        }
        let mut c = ColAst { name, decl, notnull: false, default: None, pk: false, pk_desc: false, autoinc: false, unique: false };
        loop {
            if self.eat_kw("CONSTRAINT") {
                self.name()?;
                continue;
            }
            if self.eat_kw("PRIMARY") {
                self.want_kw("KEY")?;
                c.pk = true;
                if self.eat_kw("DESC") {
                    c.pk_desc = true;
                } else {
                    let _ = self.eat_kw("ASC");
                }
                self.no_conflict_clause()?;
                if self.eat_kw("AUTOINCREMENT") {
                    c.autoinc = true;
                }
            } else if self.eat_kw("NOT") {
                self.want_kw("NULL")?;
                c.notnull = true;
                self.no_conflict_clause()?;
            } else if self.eat_kw("NULL") {
            } else if self.eat_kw("UNIQUE") {
                c.unique = true;
                self.no_conflict_clause()?;
            } else if self.eat_kw("DEFAULT") {
                let e = if self.eat_sym("(") {
                    let e = self.expr()?;
                    self.want_sym(")")?;
                    e
                } else {
                    self.unary()?
                };
                c.default = Some(const_value(&e).ok_or_else(|| self.unsupported("non-constant DEFAULT"))?);
            } else if self.eat_kw("COLLATE") {
                let n = self.name()?;
                if !eqi(&n, "BINARY") {
                    return Err(self.unsupported("collation"));
                }
            } else if self.is_kw("CHECK") || self.is_kw("REFERENCES") || self.is_kw("GENERATED") || self.is_kw("AS") {
                return Err(self.unsupported("column constraint"));
            } else {
                break;
            }
        }
        Ok(c)
    }

    fn insert(&mut self) -> Result<Insert, String> {
        let mut mode = Conf::Abort;
        if self.eat_kw("REPLACE") {
            mode = Conf::Replace;
        } else {
            self.want_kw("INSERT")?;
            if self.eat_kw("OR") {
                mode = if self.eat_kw("IGNORE") {
                    Conf::Ignore
                } else if self.eat_kw("REPLACE") {
                    Conf::Replace
                } else if self.eat_kw("ABORT") {
                    Conf::Abort
                } else {
                    return Err(self.unsupported("INSERT OR mode"));
                };
            }
        }
        self.want_kw("INTO")?;
        let table = self.name()?;
        let alias = if self.eat_kw("AS") { Some(self.name()?) } else { None };
        let col_names = if self.is_sym("(") { Some(self.name_list(false)?) } else { None };
        let mut rows = Vec::new();
        if self.eat_kw("DEFAULT") {
            self.want_kw("VALUES")?;
            rows.push(Vec::new());
        } else if self.eat_kw("VALUES") {
            loop {
                self.want_sym("(")?;
                let mut r = Vec::new();
                loop {
                    r.push(self.expr()?);
                    if !self.eat_sym(",") {
                        break;
                    }
                }
                self.want_sym(")")?;
                rows.push(r);
                if !self.eat_sym(",") {
                    break;
                }
            }
        } else if self.is_kw("SELECT") {
            return Err(self.unsupported("INSERT ... SELECT"));
        } else {
            return Err(self.err());
        }
        let mut upsert = None;
        if self.eat_kw("ON") {
            self.want_kw("CONFLICT")?;
            let target_names = if self.is_sym("(") { self.name_list(false)? } else { Vec::new() };
            if self.is_kw("WHERE") {
                return Err(self.unsupported("upsert target WHERE"));
            }
            self.want_kw("DO")?;
            let mut u = Upsert { target_names, target: None, sets: None, sets_c: Vec::new(), where_: None };
            if self.eat_kw("NOTHING") {
            } else {
                self.want_kw("UPDATE")?;
                self.want_kw("SET")?;
                let mut sets = Vec::new();
                loop {
                    let c = self.name()?;
                    self.want_sym("=")?;
                    sets.push((c, self.expr()?));
                    if !self.eat_sym(",") {
                        break;
                    }
                }
                u.sets = Some(sets);
                if self.eat_kw("WHERE") {
                    u.where_ = Some(self.expr()?);
                }
            }
            if self.is_kw("ON") {
                return Err(self.unsupported("multiple ON CONFLICT clauses"));
            }
            upsert = Some(u);
        }
        if self.is_kw("RETURNING") {
            return Err(self.unsupported("RETURNING"));
        }
        Ok(Insert { table, alias, mode, col_names, cols: Vec::new(), rows, upsert })
    }

    fn select(&mut self) -> Result<Select, String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(String::from("parser stack overflow"));
        }
        let r = self.select_inner();
        self.depth -= 1;
        r
    }

    fn select_inner(&mut self) -> Result<Select, String> {
        self.want_kw("SELECT")?;
        let mut sel = Select::default();
        if self.eat_kw("DISTINCT") {
            sel.distinct = true;
        } else {
            let _ = self.eat_kw("ALL");
        }
        loop {
            if self.eat_sym("*") {
                sel.cols.push(RCol { ex: Ex::Lit(SqlVal::Null), star: Some(None), alias: None, span: String::from("*") });
            } else if matches!(self.peek(), T::Id(_) | T::QId(_))
                && matches!(self.tok(1).t, T::Sym("."))
                && matches!(self.tok(2).t, T::Sym("*"))
            {
                let n = match self.bump() {
                    T::Id(s) | T::QId(s) => s,
                    _ => String::new(),
                };
                self.bump();
                self.bump();
                sel.cols.push(RCol { ex: Ex::Lit(SqlVal::Null), star: Some(Some(n)), alias: None, span: String::from("*") });
            } else {
                let s = self.tok(0).s;
                let ex = self.expr()?;
                let e = self.t.get(self.i.saturating_sub(1)).map_or(s, |t| t.e);
                let span = self.src.get(s..e.max(s)).unwrap_or("").to_string();
                let alias = self.opt_alias()?;
                sel.cols.push(RCol { ex, star: None, alias, span });
            }
            if !self.eat_sym(",") {
                break;
            }
        }
        if self.eat_kw("FROM") {
            sel.from.push(self.from_item()?);
            loop {
                if self.eat_sym(",") {
                    sel.from.push(self.from_item()?);
                    continue;
                }
                if self.is_kw("LEFT") || self.is_kw("RIGHT") || self.is_kw("FULL") || self.is_kw("NATURAL") {
                    return Err(self.unsupported("outer/natural join"));
                }
                let join = if self.eat_kw("INNER") || self.eat_kw("CROSS") {
                    self.want_kw("JOIN")?;
                    true
                } else {
                    self.eat_kw("JOIN")
                };
                if !join {
                    break;
                }
                let mut it = self.from_item()?;
                if self.eat_kw("ON") {
                    it.on = Some(self.expr()?);
                } else if self.is_kw("USING") {
                    return Err(self.unsupported("USING"));
                }
                sel.from.push(it);
            }
        }
        if self.eat_kw("WHERE") {
            sel.where_ = Some(self.expr()?);
        }
        if self.eat_kw("GROUP") {
            self.want_kw("BY")?;
            loop {
                sel.group.push(self.expr()?);
                if !self.eat_sym(",") {
                    break;
                }
            }
        }
        if self.eat_kw("HAVING") {
            sel.having = Some(self.expr()?);
        }
        if self.is_kw("UNION") || self.is_kw("EXCEPT") || self.is_kw("INTERSECT") {
            return Err(self.unsupported("compound SELECT"));
        }
        if self.eat_kw("ORDER") {
            self.want_kw("BY")?;
            loop {
                let e = self.expr()?;
                if self.eat_kw("COLLATE") {
                    let c = self.name()?;
                    if !eqi(&c, "BINARY") {
                        return Err(self.unsupported("collation"));
                    }
                }
                let desc = if self.eat_kw("DESC") {
                    true
                } else {
                    let _ = self.eat_kw("ASC");
                    false
                };
                if self.eat_kw("NULLS") {
                    let first = if self.eat_kw("FIRST") {
                        true
                    } else {
                        self.want_kw("LAST")?;
                        false
                    };
                    if first == desc {
                        return Err(self.unsupported("NULLS FIRST/LAST"));
                    }
                }
                sel.order.push((e, desc));
                if !self.eat_sym(",") {
                    break;
                }
            }
        }
        if self.eat_kw("LIMIT") {
            let a = self.expr()?;
            if self.eat_kw("OFFSET") {
                sel.limit = Some(a);
                sel.offset = Some(self.expr()?);
            } else if self.eat_sym(",") {
                sel.offset = Some(a);
                sel.limit = Some(self.expr()?);
            } else {
                sel.limit = Some(a);
            }
        }
        Ok(sel)
    }

    fn from_item(&mut self) -> Result<FromItem, String> {
        if self.eat_sym("(") {
            if !self.is_kw("SELECT") {
                return Err(self.unsupported("parenthesized join"));
            }
            let s = self.select()?;
            self.want_sym(")")?;
            let alias = self.opt_alias()?;
            return Ok(FromItem { name: None, sub: Some(Box::new(s)), alias, on: None });
        }
        let name = self.name()?;
        if self.eat_sym(".") {
            // schema-qualified: only "main" is meaningful
            let n2 = self.name()?;
            if !eqi(&name, "main") {
                return Err(format!("unknown database {}", name));
            }
            let alias = self.opt_alias()?;
            return Ok(FromItem { name: Some(n2), sub: None, alias, on: None });
        }
        let alias = self.opt_alias()?;
        if self.is_kw("INDEXED") || (self.is_kw("NOT") && self.is_kw_at(1, "INDEXED")) {
            return Err(self.unsupported("INDEXED BY"));
        }
        Ok(FromItem { name: Some(name), sub: None, alias, on: None })
    }

    // ── expressions ──

    fn expr(&mut self) -> Result<Ex, String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(String::from("parser stack overflow"));
        }
        let saved = self.tree;
        let r = self.grow().and_then(|_| self.or_ex());
        self.tree = saved;
        self.depth -= 1;
        r
    }

    fn or_ex(&mut self) -> Result<Ex, String> {
        let saved = self.tree;
        let r = self.or_ex_inner();
        self.tree = saved;
        r
    }

    fn or_ex_inner(&mut self) -> Result<Ex, String> {
        let mut l = self.and_ex()?;
        while self.eat_kw("OR") {
            let r = self.and_ex()?;
            l = Ex::Bin(BOp::Or, Box::new(l), Box::new(r));
            self.grow()?;
        }
        Ok(l)
    }

    fn and_ex(&mut self) -> Result<Ex, String> {
        let saved = self.tree;
        let r = self.and_ex_inner();
        self.tree = saved;
        r
    }

    fn and_ex_inner(&mut self) -> Result<Ex, String> {
        let mut l = self.not_ex()?;
        while self.eat_kw("AND") {
            let r = self.not_ex()?;
            l = Ex::Bin(BOp::And, Box::new(l), Box::new(r));
            self.grow()?;
        }
        Ok(l)
    }

    fn not_ex(&mut self) -> Result<Ex, String> {
        if self.eat_kw("NOT") {
            self.depth += 1;
            if self.depth > MAX_DEPTH {
                return Err(String::from("parser stack overflow"));
            }
            let e = self.not_ex();
            self.depth -= 1;
            return Ok(Ex::Un(UOp::Not, Box::new(e?)));
        }
        self.eq_ex()
    }

    fn eq_ex(&mut self) -> Result<Ex, String> {
        let saved = self.tree;
        let r = self.eq_ex_inner();
        self.tree = saved;
        r
    }

    fn eq_ex_inner(&mut self) -> Result<Ex, String> {
        let mut l = self.cmp_ex()?;
        loop {
            let op = if self.eat_sym("=") || self.eat_sym("==") {
                Some(BOp::Eq)
            } else if self.eat_sym("!=") || self.eat_sym("<>") {
                Some(BOp::Ne)
            } else if self.eat_kw("IS") {
                if self.eat_kw("NOT") {
                    Some(BOp::IsNot)
                } else {
                    Some(BOp::Is)
                }
            } else {
                None
            };
            if let Some(op) = op {
                let r = self.cmp_ex()?;
                l = Ex::Bin(op, Box::new(l), Box::new(r));
                self.grow()?;
                continue;
            }
            if self.eat_kw("ISNULL") {
                l = Ex::Bin(BOp::Is, Box::new(l), Box::new(Ex::Lit(SqlVal::Null)));
                self.grow()?;
                continue;
            }
            if self.eat_kw("NOTNULL") {
                l = Ex::Bin(BOp::IsNot, Box::new(l), Box::new(Ex::Lit(SqlVal::Null)));
                self.grow()?;
                continue;
            }
            let not = if self.is_kw("NOT")
                && (self.is_kw_at(1, "IN") || self.is_kw_at(1, "BETWEEN") || self.is_kw_at(1, "NULL") || self.is_kw_at(1, "LIKE") || self.is_kw_at(1, "GLOB"))
            {
                self.bump();
                true
            } else {
                false
            };
            if self.eat_kw("NULL") {
                // NOT NULL postfix
                l = Ex::Bin(BOp::IsNot, Box::new(l), Box::new(Ex::Lit(SqlVal::Null)));
                self.grow()?;
                continue;
            }
            if self.eat_kw("IN") {
                self.want_sym("(")?;
                if self.is_kw("SELECT") {
                    let s = self.select()?;
                    self.want_sym(")")?;
                    l = Ex::Sub(Box::new(SubQ { kind: SubKind::In, sel: s, lhs: Some(l), not, id: 0, corr: false, aff: Aff::None }));
                    self.grow()?;
                } else {
                    let mut list = Vec::new();
                    if !self.is_sym(")") {
                        loop {
                            list.push(self.expr()?);
                            if !self.eat_sym(",") {
                                break;
                            }
                        }
                    }
                    self.want_sym(")")?;
                    l = Ex::InList(Box::new(l), list, not);
                    self.grow()?;
                }
                continue;
            }
            if self.eat_kw("BETWEEN") {
                let a = self.cmp_ex()?;
                self.want_kw("AND")?;
                let b = self.cmp_ex()?;
                l = Ex::Between(Box::new(l), Box::new(a), Box::new(b), not);
                self.grow()?;
                continue;
            }
            if self.is_kw("LIKE") || self.is_kw("GLOB") || self.is_kw("REGEXP") || self.is_kw("MATCH") {
                return Err(self.unsupported("LIKE/GLOB"));
            }
            if not {
                return Err(self.err());
            }
            break;
        }
        Ok(l)
    }

    fn cmp_ex(&mut self) -> Result<Ex, String> {
        let saved = self.tree;
        let r = self.cmp_ex_inner();
        self.tree = saved;
        r
    }

    fn cmp_ex_inner(&mut self) -> Result<Ex, String> {
        let mut l = self.bit_ex()?;
        loop {
            let op = if self.eat_sym("<") {
                BOp::Lt
            } else if self.eat_sym("<=") {
                BOp::Le
            } else if self.eat_sym(">") {
                BOp::Gt
            } else if self.eat_sym(">=") {
                BOp::Ge
            } else {
                break;
            };
            let r = self.bit_ex()?;
            l = Ex::Bin(op, Box::new(l), Box::new(r));
            self.grow()?;
        }
        Ok(l)
    }

    fn bit_ex(&mut self) -> Result<Ex, String> {
        let l = self.add_ex()?;
        if self.is_sym("&") || self.is_sym("|") || self.is_sym("<<") || self.is_sym(">>") {
            return Err(self.unsupported("bitwise operator"));
        }
        Ok(l)
    }

    fn add_ex(&mut self) -> Result<Ex, String> {
        let saved = self.tree;
        let r = self.add_ex_inner();
        self.tree = saved;
        r
    }

    fn add_ex_inner(&mut self) -> Result<Ex, String> {
        let mut l = self.mul_ex()?;
        loop {
            let op = if self.eat_sym("+") {
                BOp::Add
            } else if self.eat_sym("-") {
                BOp::Sub
            } else {
                break;
            };
            let r = self.mul_ex()?;
            l = Ex::Bin(op, Box::new(l), Box::new(r));
            self.grow()?;
        }
        Ok(l)
    }

    fn mul_ex(&mut self) -> Result<Ex, String> {
        let saved = self.tree;
        let r = self.mul_ex_inner();
        self.tree = saved;
        r
    }

    fn mul_ex_inner(&mut self) -> Result<Ex, String> {
        let mut l = self.cat_ex()?;
        loop {
            let op = if self.eat_sym("*") {
                BOp::Mul
            } else if self.eat_sym("/") {
                BOp::Div
            } else if self.eat_sym("%") {
                BOp::Rem
            } else {
                break;
            };
            let r = self.cat_ex()?;
            l = Ex::Bin(op, Box::new(l), Box::new(r));
            self.grow()?;
        }
        Ok(l)
    }

    fn cat_ex(&mut self) -> Result<Ex, String> {
        let saved = self.tree;
        let r = self.cat_ex_inner();
        self.tree = saved;
        r
    }

    fn cat_ex_inner(&mut self) -> Result<Ex, String> {
        let mut l = self.unary()?;
        while self.eat_sym("||") {
            let r = self.unary()?;
            l = Ex::Bin(BOp::Concat, Box::new(l), Box::new(r));
            self.grow()?;
        }
        Ok(l)
    }

    fn unary(&mut self) -> Result<Ex, String> {
        if self.is_sym("-") || self.is_sym("+") {
            let neg = self.is_sym("-");
            self.bump();
            if neg {
                if let T::Num(n) = self.peek() {
                    if n == "9223372036854775808" {
                        self.bump();
                        return Ok(Ex::Lit(SqlVal::Int(i64::MIN)));
                    }
                }
            }
            self.depth += 1;
            if self.depth > MAX_DEPTH {
                return Err(String::from("parser stack overflow"));
            }
            let e = self.unary();
            self.depth -= 1;
            let e = e?;
            if neg {
                match e {
                    Ex::Lit(SqlVal::Int(i)) if i != i64::MIN => return Ok(Ex::Lit(SqlVal::Int(-i))),
                    Ex::Lit(SqlVal::Real(r)) => return Ok(Ex::Lit(SqlVal::Real(-r))),
                    e => return Ok(Ex::Un(UOp::Neg, Box::new(e))),
                }
            }
            return Ok(Ex::Un(UOp::Pos, Box::new(e)));
        }
        if self.is_sym("~") {
            return Err(self.unsupported("bitwise operator"));
        }
        self.primary()
    }

    fn param_index(&mut self, t: &T) -> Result<usize, String> {
        match t {
            T::Param(Some(n)) => {
                let n = *n as usize;
                if n > self.maxp {
                    self.maxp = n;
                }
                Ok(n - 1)
            }
            T::Param(None) => {
                self.maxp += 1;
                Ok(self.maxp - 1)
            }
            T::NParam(name) => {
                if let Some(p) = self.named.iter().position(|x| x == name) {
                    // named params get the index assigned at first appearance
                    return Ok(self.named_idx[p]);
                }
                self.maxp += 1;
                self.named.push(name.clone());
                self.named_idx.push(self.maxp - 1);
                Ok(self.maxp - 1)
            }
            _ => Err(self.err()),
        }
    }

    fn primary(&mut self) -> Result<Ex, String> {
        let t = self.peek().clone();
        match t {
            T::Num(n) => {
                self.bump();
                if !n.contains(|c: char| c == '.' || c == 'e' || c == 'E') {
                    if let Ok(i) = n.parse::<i64>() {
                        return Ok(Ex::Lit(SqlVal::Int(i)));
                    }
                }
                match n.parse::<f64>() {
                    Ok(r) => Ok(Ex::Lit(SqlVal::Real(r))),
                    Err(_) => Err(format!("unrecognized token: \"{}\"", n)),
                }
            }
            T::Str(s) => {
                self.bump();
                Ok(Ex::Lit(SqlVal::Text(s)))
            }
            T::Param(_) | T::NParam(_) => {
                self.bump();
                Ok(Ex::Param(self.param_index(&t)?))
            }
            T::Sym("(") => {
                self.bump();
                if self.is_kw("SELECT") {
                    let s = self.select()?;
                    self.want_sym(")")?;
                    return Ok(Ex::Sub(Box::new(SubQ { kind: SubKind::Scalar, sel: s, lhs: None, not: false, id: 0, corr: false, aff: Aff::None })));
                }
                let e = self.expr()?;
                if self.is_sym(",") {
                    return Err(self.unsupported("row value"));
                }
                self.want_sym(")")?;
                Ok(e)
            }
            T::Id(s) => {
                if eqi(&s, "NULL") {
                    self.bump();
                    return Ok(Ex::Lit(SqlVal::Null));
                }
                if eqi(&s, "EXISTS") && matches!(self.tok(1).t, T::Sym("(")) {
                    self.bump();
                    self.bump();
                    let sel = self.select()?;
                    self.want_sym(")")?;
                    return Ok(Ex::Sub(Box::new(SubQ { kind: SubKind::Exists, sel, lhs: None, not: false, id: 0, corr: false, aff: Aff::None })));
                }
                if eqi(&s, "CASE") || eqi(&s, "CAST") || eqi(&s, "RAISE") {
                    return Err(self.unsupported(&s));
                }
                if eqi(&s, "CURRENT_TIME") || eqi(&s, "CURRENT_DATE") || eqi(&s, "CURRENT_TIMESTAMP") {
                    return Err(self.unsupported(&s));
                }
                if matches!(self.tok(1).t, T::Sym("(")) {
                    self.bump();
                    self.bump();
                    let fname = s.to_ascii_lowercase();
                    let mut args = Vec::new();
                    let mut star = false;
                    let mut distinct = false;
                    if self.eat_sym("*") {
                        star = true;
                    } else if !self.is_sym(")") {
                        if self.eat_kw("DISTINCT") {
                            distinct = true;
                        } else {
                            let _ = self.eat_kw("ALL");
                        }
                        loop {
                            args.push(self.expr()?);
                            if !self.eat_sym(",") {
                                break;
                            }
                        }
                    }
                    self.want_sym(")")?;
                    if self.is_kw("FILTER") || self.is_kw("OVER") {
                        return Err(self.unsupported("window/filter"));
                    }
                    return Ok(Ex::Func(fname, args, star, distinct));
                }
                if is_reserved(&s) {
                    return Err(self.err());
                }
                self.bump();
                self.col_ref(s)
            }
            T::QId(s) => {
                self.bump();
                self.col_ref(s)
            }
            _ => Err(self.err()),
        }
    }

    fn col_ref(&mut self, first: String) -> Result<Ex, String> {
        if self.eat_sym(".") {
            let second = self.name()?;
            if self.eat_sym(".") {
                // schema.table.column
                let third = self.name()?;
                return Ok(Ex::Name(Some(second), third));
            }
            return Ok(Ex::Name(Some(first), second));
        }
        Ok(Ex::Name(None, first))
    }
}

// The named-parameter index table lives next to `named`; declared separately
// so the struct literal stays readable.
impl<'s> Parser<'s> {
    fn new(src: &'s str) -> Result<Parser<'s>, String> {
        Ok(Parser { src, t: lex(src)?, i: 0, depth: 0, tree: 0, maxp: 0, named: Vec::new(), named_idx: Vec::new() })
    }
}

fn const_value(e: &Ex) -> Option<SqlVal> {
    match e {
        Ex::Lit(v) => Some(v.clone()),
        Ex::Un(UOp::Pos, x) => const_value(x),
        Ex::Un(UOp::Neg, x) => match const_value(x)? {
            SqlVal::Int(i) => Some(match i.checked_neg() {
                Some(n) => SqlVal::Int(n),
                None => SqlVal::Real(-(i as f64)),
            }),
            SqlVal::Real(r) => Some(SqlVal::Real(-r)),
            _ => None,
        },
        _ => None,
    }
}

fn parse(sql: &str) -> Result<Stmt, String> {
    let mut p = Parser::new(sql)?;
    p.stmt()
}

// ════════════════════════════════════════════════════════════════════════
// Schema
// ════════════════════════════════════════════════════════════════════════

#[derive(Clone, Debug)]
struct Col {
    name: String,
    aff: Aff,
    notnull: bool,
    default: Option<SqlVal>,
}

#[derive(Clone, Debug)]
struct Index {
    name: String,
    cols: Vec<usize>,
    unique: bool,
    set: BTreeSet<IKey>,
    /// Source SQL for explicit CREATE INDEX (None for constraint indexes).
    sql: Option<String>,
}

#[derive(Clone, Debug)]
struct Table {
    name: String,
    sql: String,
    cols: Vec<Col>,
    alias: Option<usize>,
    autoinc: bool,
    seq: i64,
    rows: BTreeMap<i64, Vec<SqlVal>>,
    indexes: Vec<Index>,
}

impl Table {
    fn col_index(&self, n: &str) -> Option<usize> {
        self.cols.iter().position(|c| eqi(&c.name, n))
    }

    fn key_for(idx: &Index, vals: &[SqlVal], rid: i64) -> IKey {
        let mut k = Vec::with_capacity(idx.cols.len());
        for &c in &idx.cols {
            k.push(KeyV::of(vals.get(c).unwrap_or(&SqlVal::Null)));
        }
        IKey::new(k, rid)
    }

    fn raw_insert(&mut self, rid: i64, vals: Vec<SqlVal>) {
        for i in 0..self.indexes.len() {
            let k = Table::key_for(&self.indexes[i], &vals, rid);
            self.indexes[i].set.insert(k);
        }
        self.rows.insert(rid, vals);
    }

    fn raw_delete(&mut self, rid: i64) -> Option<Vec<SqlVal>> {
        let vals = self.rows.remove(&rid)?;
        for i in 0..self.indexes.len() {
            let k = Table::key_for(&self.indexes[i], &vals, rid);
            self.indexes[i].set.remove(&k);
        }
        Some(vals)
    }

    /// Row (other than `exclude`) that has the same key in unique index `x`.
    fn find_conflict(&self, x: usize, vals: &[SqlVal], exclude: Option<i64>) -> Option<i64> {
        let idx = self.indexes.get(x)?;
        let mut lo = Vec::with_capacity(idx.cols.len() + 1);
        for &c in &idx.cols {
            let v = vals.get(c).unwrap_or(&SqlVal::Null);
            if *v == SqlVal::Null {
                return None;
            }
            lo.push(KeyV::of(v));
        }
        let (lo, hi) = IKey::bounds(lo, idx.cols.len());
        for k in idx.set.range(lo..=hi) {
            if Some(k.rid) != exclude {
                return Some(k.rid);
            }
        }
        None
    }

    fn unique_msg(&self, x: Option<usize>) -> String {
        match x.and_then(|x| self.indexes.get(x)) {
            Some(idx) => {
                let cols: Vec<String> = idx.cols.iter().map(|&c| format!("{}.{}", self.name, self.cols.get(c).map_or("?", |c| c.name.as_str()))).collect();
                format!("UNIQUE constraint failed: {}", cols.join(", "))
            }
            None => {
                let n = self.alias.and_then(|a| self.cols.get(a)).map_or("rowid", |c| c.name.as_str());
                format!("UNIQUE constraint failed: {}.{}", self.name, n)
            }
        }
    }

    fn is_rowid_col(&self, c: u32) -> bool {
        c == ROWID || self.alias.map_or(false, |a| a as u32 == c)
    }

    fn new_rowid(&self) -> Result<i64, String> {
        let last = self.rows.keys().next_back().copied().unwrap_or(0);
        let base = if self.autoinc { last.max(self.seq) } else { last };
        if base == i64::MAX {
            return Err(String::from("database or disk is full"));
        }
        Ok(base + 1)
    }
}

// ════════════════════════════════════════════════════════════════════════
// Compile (name resolution + planning)
// ════════════════════════════════════════════════════════════════════════

struct SrcInfo {
    name: String,
    cols: Vec<(String, Aff)>,
    alias: Option<usize>,
    table: bool,
}

struct Scope<'a> {
    srcs: &'a [SrcInfo],
    parent: Option<&'a Scope<'a>>,
}

fn is_rowid_name(s: &str) -> bool {
    eqi(s, "rowid") || eqi(s, "oid") || eqi(s, "_rowid_")
}

fn lookup(sc: &Scope, t: &Option<String>, c: &str) -> Result<Ex, String> {
    let mut up = 0u32;
    let mut cur = Some(sc);
    while let Some(s) = cur {
        let mut found: Option<Ex> = None;
        let mut rowid_found: Option<Ex> = None;
        for (si, src) in s.srcs.iter().enumerate() {
            if let Some(tn) = t {
                if !eqi(tn, &src.name) {
                    continue;
                }
            }
            if let Some(ci) = src.cols.iter().position(|(n, _)| eqi(n, c)) {
                if found.is_some() {
                    return Err(format!("ambiguous column name: {}", c));
                }
                found = Some(Ex::Col { up, src: si as u32, col: ci as u32, aff: src.cols[ci].1 });
            } else if src.table && is_rowid_name(c) {
                if rowid_found.is_some() {
                    return Err(format!("ambiguous column name: {}", c));
                }
                rowid_found = Some(Ex::Col { up, src: si as u32, col: ROWID, aff: Aff::Integer });
            }
        }
        if let Some(f) = found.or(rowid_found) {
            return Ok(f);
        }
        up += 1;
        cur = s.parent;
    }
    Err(match t {
        Some(t) => format!("no such column: {}.{}", t, c),
        None => format!("no such column: {}", c),
    })
}

fn expr_aff(e: &Ex) -> Aff {
    match e {
        Ex::Col { aff, .. } => *aff,
        Ex::Sub(s) if s.kind == SubKind::Scalar => s.sel.affs.first().copied().unwrap_or(Aff::None),
        _ => Aff::None,
    }
}

fn for_children(e: &Ex, f: &mut dyn FnMut(&Ex)) {
    match e {
        Ex::Un(_, a) => f(a),
        Ex::Bin(_, a, b) => {
            f(a);
            f(b)
        }
        Ex::Between(a, b, c, _) => {
            f(a);
            f(b);
            f(c)
        }
        Ex::InList(a, l, _) => {
            f(a);
            for x in l {
                f(x)
            }
        }
        Ex::Sub(s) => {
            if let Some(l) = &s.lhs {
                f(l)
            }
        }
        Ex::Func(_, a, _, _) => {
            for x in a {
                f(x)
            }
        }
        _ => {}
    }
}

fn for_children_mut(e: &mut Ex, f: &mut dyn FnMut(&mut Ex) -> Result<(), String>) -> Result<(), String> {
    match e {
        Ex::Un(_, a) => f(a),
        Ex::Bin(_, a, b) => {
            f(a)?;
            f(b)
        }
        Ex::Between(a, b, c, _) => {
            f(a)?;
            f(b)?;
            f(c)
        }
        Ex::InList(a, l, _) => {
            f(a)?;
            for x in l {
                f(x)?
            }
            Ok(())
        }
        Ex::Sub(s) => {
            if let Some(l) = &mut s.lhs {
                f(l)?
            }
            Ok(())
        }
        Ex::Func(_, a, _, _) => {
            for x in a {
                f(x)?
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Visit every top-level expression of a compiled select.
fn sel_exprs(sel: &Select, f: &mut dyn FnMut(&Ex)) {
    for c in &sel.cols {
        f(&c.ex)
    }
    for c in &sel.pre {
        f(c)
    }
    for l in &sel.levels {
        for c in &l.conds {
            f(c)
        }
        match &l.access {
            Access::Rowid(e, _) => f(e),
            Access::Index(_, p) => {
                for (e, _) in p {
                    f(e)
                }
            }
            Access::RowidIn(e) | Access::IndexIn(_, e) => f(e),
            Access::Scan => {}
        }
    }
    if let Some(w) = &sel.where_ {
        f(w)
    }
    for fi in &sel.from {
        if let Some(on) = &fi.on {
            f(on)
        }
    }
    for g in &sel.group {
        f(g)
    }
    if let Some(h) = &sel.having {
        f(h)
    }
    for (o, _) in &sel.order {
        f(o)
    }
    for a in &sel.aggs {
        if let Some(x) = &a.arg {
            f(x)
        }
    }
    if let Some(l) = &sel.limit {
        f(l)
    }
    if let Some(o) = &sel.offset {
        f(o)
    }
}

/// Bitmask of sources (of the select `d` levels out) referenced by `e`.
fn mask(e: &Ex, d: u32) -> u64 {
    let mut m = 0u64;
    match e {
        Ex::Col { up, src, .. } => {
            if *up == d && *src < 64 {
                m |= 1u64 << *src;
            }
        }
        Ex::Sub(s) => {
            if let Some(l) = &s.lhs {
                m |= mask(l, d);
            }
            m |= sel_mask(&s.sel, d + 1);
        }
        _ => for_children(e, &mut |c| m |= mask(c, d)),
    }
    m
}

fn sel_mask(sel: &Select, d: u32) -> u64 {
    let mut m = 0u64;
    sel_exprs(sel, &mut |e| m |= mask(e, d));
    for fi in &sel.from {
        if let Some(s) = &fi.sub {
            // FROM-subqueries share the enclosing select's parent scope.
            // (at d == 0 its refs are to its own sources, not ours)
            if d > 0 {
                m |= sel_mask(s, d);
            }
        }
    }
    m
}

/// Does `e` (at nesting depth d inside the subquery) reference a scope
/// outside the subquery?
fn escapes(e: &Ex, d: u32) -> bool {
    match e {
        Ex::Col { up, .. } => *up > d,
        Ex::Sub(s) => s.lhs.as_ref().map_or(false, |l| escapes(l, d)) || sel_escapes(&s.sel, d + 1),
        _ => {
            let mut r = false;
            for_children(e, &mut |c| r = r || escapes(c, d));
            r
        }
    }
}

fn sel_escapes(sel: &Select, d: u32) -> bool {
    let mut r = false;
    sel_exprs(sel, &mut |e| r = r || escapes(e, d));
    for fi in &sel.from {
        if let Some(s) = &fi.sub {
            r = r || sel_escapes(s, d);
        }
    }
    r
}

fn has_sub(e: &Ex) -> bool {
    match e {
        Ex::Sub(_) => true,
        _ => {
            let mut r = false;
            for_children(e, &mut |c| r = r || has_sub(c));
            r
        }
    }
}

fn has_agg(e: &Ex) -> bool {
    match e {
        Ex::Agg(_) => true,
        Ex::Func(n, a, star, _) if agg_kind(n, a.len(), *star).is_some() => true,
        _ => {
            let mut r = false;
            for_children(e, &mut |c| r = r || has_agg(c));
            r
        }
    }
}

fn agg_kind(n: &str, nargs: usize, star: bool) -> Option<AggF> {
    match n {
        "count" if star || nargs <= 1 => Some(AggF::Count),
        "sum" if nargs == 1 => Some(AggF::Sum),
        "total" if nargs == 1 => Some(AggF::Total),
        "avg" if nargs == 1 => Some(AggF::Avg),
        "min" if nargs == 1 => Some(AggF::Min),
        "max" if nargs == 1 => Some(AggF::Max),
        _ => None,
    }
}

fn extract_aggs(e: &mut Ex, aggs: &mut Vec<AggDef>) -> Result<(), String> {
    if let Ex::Func(n, args, star, distinct) = e {
        if let Some(f) = agg_kind(n, args.len(), *star) {
            let arg = if *star { None } else { args.pop() };
            if let Some(a) = &arg {
                if has_agg(a) {
                    return Err(String::from("misuse of aggregate function"));
                }
            }
            let d = *distinct;
            aggs.push(AggDef { f, arg, distinct: d });
            *e = Ex::Agg((aggs.len() - 1) as u32);
            return Ok(());
        }
    }
    if let Ex::Sub(s) = e {
        if let Some(l) = &mut s.lhs {
            extract_aggs(l, aggs)?;
        }
        return Ok(());
    }
    for_children_mut(e, &mut |c| extract_aggs(c, aggs))
}

fn split_and(e: Ex, out: &mut Vec<Ex>) {
    match e {
        Ex::Bin(BOp::And, a, b) => {
            split_and(*a, out);
            split_and(*b, out);
        }
        o => out.push(o),
    }
}

const SCALAR_FNS: [&str; 10] = ["abs", "coalesce", "ifnull", "nullif", "length", "lower", "upper", "typeof", "min", "max"];

impl Db {
    fn table(&self, name: &str) -> Result<&Table, String> {
        self.tables.get(&name.to_ascii_lowercase()).ok_or_else(|| format!("no such table: {}", name))
    }

    fn resolve(&self, e: &mut Ex, sc: &Scope, ids: &mut u32) -> Result<(), String> {
        match e {
            Ex::Name(t, c) => {
                *e = lookup(sc, t, c)?;
                Ok(())
            }
            Ex::Sub(s) => {
                if let Some(l) = &mut s.lhs {
                    self.resolve(l, sc, ids)?;
                }
                self.compile_select(&mut s.sel, Some(sc), ids)?;
                *ids += 1;
                s.id = *ids;
                s.corr = sel_escapes(&s.sel, 0);
                if s.kind != SubKind::Exists && s.sel.names.len() != 1 {
                    return Err(format!("sub-select returns {} columns - expected 1", s.sel.names.len()));
                }
                if s.kind == SubKind::In {
                    let la = s.lhs.as_ref().map_or(Aff::None, expr_aff);
                    s.aff = cmp_affinity(la, s.sel.affs.first().copied().unwrap_or(Aff::None));
                }
                Ok(())
            }
            Ex::Func(n, a, star, _) => {
                if agg_kind(n, a.len(), *star).is_none() && !SCALAR_FNS.contains(&n.as_str()) {
                    return Err(format!("no such function: {}", n));
                }
                if agg_kind(n, a.len(), *star).is_none() && (*star || a.is_empty()) {
                    return Err(format!("wrong number of arguments to function {}()", n));
                }
                for x in a.iter_mut() {
                    self.resolve(x, sc, ids)?;
                }
                Ok(())
            }
            _ => for_children_mut(e, &mut |c| self.resolve(c, sc, ids)),
        }
    }

    fn compile_select(&self, sel: &mut Select, parent: Option<&Scope>, ids: &mut u32) -> Result<(), String> {
        let mut srcs: Vec<SrcInfo> = Vec::new();
        for f in sel.from.iter_mut() {
            if let Some(sub) = &mut f.sub {
                self.compile_select(sub, parent, ids)?;
                srcs.push(SrcInfo {
                    name: f.alias.clone().unwrap_or_default(),
                    cols: sub.names.iter().cloned().zip(sub.affs.iter().copied()).collect(),
                    alias: None,
                    table: false,
                });
            } else {
                let n = f.name.clone().unwrap_or_default();
                let t = self.table(&n)?;
                f.name = Some(n.to_ascii_lowercase());
                srcs.push(SrcInfo {
                    name: f.alias.clone().unwrap_or_else(|| t.name.clone()),
                    cols: t.cols.iter().map(|c| (c.name.clone(), c.aff)).collect(),
                    alias: t.alias,
                    table: true,
                });
            }
        }
        if srcs.len() > 63 {
            return Err(String::from("too many FROM items"));
        }
        // expand stars
        let mut cols = Vec::new();
        for rc in core::mem::take(&mut sel.cols) {
            match &rc.star {
                Some(which) => {
                    let mut any = false;
                    for (si, s) in srcs.iter().enumerate() {
                        if let Some(w) = which {
                            if !eqi(w, &s.name) {
                                continue;
                            }
                        }
                        any = true;
                        for (ci, (n, a)) in s.cols.iter().enumerate() {
                            cols.push(RCol { ex: Ex::Col { up: 0, src: si as u32, col: ci as u32, aff: *a }, star: None, alias: Some(n.clone()), span: n.clone() });
                        }
                    }
                    if !any {
                        return Err(match which {
                            Some(w) => format!("no such table: {}", w),
                            None => String::from("no tables specified"),
                        });
                    }
                }
                None => cols.push(rc),
            }
        }
        sel.cols = cols;
        let scope = Scope { srcs: &srcs, parent };
        for rc in sel.cols.iter_mut() {
            self.resolve(&mut rc.ex, &scope, ids)?;
        }
        if let Some(w) = &mut sel.where_ {
            self.resolve(w, &scope, ids)?;
            if has_agg(w) {
                return Err(String::from("misuse of aggregate function"));
            }
        }
        for f in sel.from.iter_mut() {
            if let Some(on) = &mut f.on {
                self.resolve(on, &scope, ids)?;
                if has_agg(on) {
                    return Err(String::from("misuse of aggregate function"));
                }
            }
        }
        // result-column aliases usable from GROUP BY / ORDER BY
        let alias_expr = |name: &str, cols: &[RCol]| -> Option<Ex> {
            cols.iter().find(|c| c.alias.as_deref().map_or(false, |a| eqi(a, name))).map(|c| c.ex.clone())
        };
        let mut group = core::mem::take(&mut sel.group);
        for g in group.iter_mut() {
            if let Ex::Lit(SqlVal::Int(k)) = g {
                let k = *k;
                if k < 1 || k as usize > sel.cols.len() {
                    return Err(String::from("GROUP BY term out of range"));
                }
                *g = sel.cols[(k - 1) as usize].ex.clone();
                continue;
            }
            if let Ex::Name(None, n) = g {
                if lookup(&scope, &None, n).is_err() {
                    if let Some(x) = alias_expr(n, &sel.cols) {
                        *g = x;
                        continue;
                    }
                }
            }
            self.resolve(g, &scope, ids)?;
            if has_agg(g) {
                return Err(String::from("aggregate functions are not allowed in the GROUP BY clause"));
            }
        }
        sel.group = group;
        if let Some(h) = &mut sel.having {
            self.resolve(h, &scope, ids)?;
        }
        let mut order = core::mem::take(&mut sel.order);
        for (o, _) in order.iter_mut() {
            if let Ex::Lit(SqlVal::Int(k)) = o {
                let k = *k;
                if k < 1 || k as usize > sel.cols.len() {
                    return Err(String::from("ORDER BY term out of range"));
                }
                *o = sel.cols[(k - 1) as usize].ex.clone();
                continue;
            }
            if let Ex::Name(None, n) = o {
                if let Some(x) = alias_expr(n, &sel.cols) {
                    *o = x;
                    continue;
                }
            }
            self.resolve(o, &scope, ids)?;
        }
        sel.order = order;
        let lscope = Scope { srcs: &[], parent };
        if let Some(l) = &mut sel.limit {
            self.resolve(l, &lscope, ids)?;
        }
        if let Some(o) = &mut sel.offset {
            self.resolve(o, &lscope, ids)?;
        }
        // names + affinities
        sel.names.clear();
        sel.affs.clear();
        for rc in &sel.cols {
            let name = if let Some(a) = &rc.alias {
                a.clone()
            } else if let Ex::Col { up: 0, src, col, .. } = &rc.ex {
                let s = &srcs[*src as usize];
                if *col == ROWID {
                    s.alias.and_then(|a| s.cols.get(a)).map_or(String::from("rowid"), |c| c.0.clone())
                } else {
                    s.cols.get(*col as usize).map_or(rc.span.clone(), |c| c.0.clone())
                }
            } else {
                rc.span.clone()
            };
            sel.names.push(name);
            sel.affs.push(expr_aff(&rc.ex));
        }
        // aggregates
        let mut aggs = Vec::new();
        for rc in sel.cols.iter_mut() {
            extract_aggs(&mut rc.ex, &mut aggs)?;
        }
        if let Some(h) = &mut sel.having {
            extract_aggs(h, &mut aggs)?;
        }
        for (o, _) in sel.order.iter_mut() {
            extract_aggs(o, &mut aggs)?;
        }
        sel.aggs = aggs;
        sel.is_agg = !sel.aggs.is_empty() || !sel.group.is_empty();
        if sel.having.is_some() && !sel.is_agg {
            return Err(String::from("a GROUP BY clause is required before HAVING"));
        }
        self.plan(sel, &srcs)
    }

    fn plan(&self, sel: &mut Select, _srcs: &[SrcInfo]) -> Result<(), String> {
        let n = sel.from.len();
        let mut conj: Vec<Ex> = Vec::new();
        if let Some(w) = sel.where_.take() {
            split_and(w, &mut conj);
        }
        for f in sel.from.iter_mut() {
            if let Some(on) = f.on.take() {
                split_and(on, &mut conj);
            }
        }
        let mut levels: Vec<Level> = (0..n).map(|_| Level { access: Access::Scan, conds: Vec::new(), need_row: true, reverse: false }).collect();
        let mut consumed = vec![false; conj.len()];
        for i in 0..n {
            if sel.from[i].sub.is_some() {
                continue;
            }
            let t = self.table(sel.from[i].name.as_deref().unwrap_or(""))?;
            let below: u64 = (1u64 << i) - 1;
            // (conj index, column, probe expr, comparison affinity)
            let mut eqs: Vec<(usize, u32, Ex, Aff)> = Vec::new();
            let mut ins: Vec<(usize, u32)> = Vec::new();
            for (ci, c) in conj.iter().enumerate() {
                if let Ex::Bin(BOp::Eq, l, r) = c {
                    for (a, b) in [(l, r), (r, l)] {
                        if let Ex::Col { up: 0, src, col, aff } = **a {
                            if src as usize == i && !has_sub(b) && mask(b, 0) & !below == 0 {
                                let x = cmp_affinity(aff, expr_aff(b));
                                let col_aff = if t.is_rowid_col(col) { Aff::Integer } else { aff };
                                if x.is_num() && !col_aff.is_num() {
                                    continue;
                                }
                                eqs.push((ci, col, (**b).clone(), x));
                                break;
                            }
                        }
                    }
                }
                if let Ex::Sub(s) = c {
                    if s.kind == SubKind::In && !s.not && !s.corr {
                        if let Some(Ex::Col { up: 0, src, col, aff }) = &s.lhs {
                            if *src as usize == i {
                                let col_aff = if t.is_rowid_col(*col) { Aff::Integer } else { *aff };
                                if !(s.aff.is_num() && !col_aff.is_num()) {
                                    ins.push((ci, *col));
                                }
                            }
                        }
                    }
                }
            }
            if let Some(e) = eqs.iter().find(|e| t.is_rowid_col(e.1)) {
                levels[i].access = Access::Rowid(e.2.clone(), e.3);
                consumed[e.0] = true;
                continue;
            }
            let mut best: Option<(usize, Vec<usize>)> = None;
            for (xi, idx) in t.indexes.iter().enumerate() {
                let mut chosen = Vec::new();
                for &c in &idx.cols {
                    match eqs.iter().position(|e| e.1 == c as u32) {
                        Some(k) => chosen.push(k),
                        None => break,
                    }
                }
                if chosen.is_empty() {
                    continue;
                }
                let better = match &best {
                    None => true,
                    Some((bx, bc)) => {
                        let bfull = bc.len() == t.indexes[*bx].cols.len() && t.indexes[*bx].unique;
                        let full = chosen.len() == idx.cols.len() && idx.unique;
                        (full && !bfull) || (full == bfull && chosen.len() > bc.len())
                    }
                };
                if better {
                    best = Some((xi, chosen));
                }
            }
            if let Some((xi, chosen)) = best {
                let mut probes = Vec::new();
                for k in chosen {
                    consumed[eqs[k].0] = true;
                    probes.push((eqs[k].2.clone(), eqs[k].3));
                }
                levels[i].access = Access::Index(xi, probes);
                continue;
            }
            if let Some(&(ci, _)) = ins.iter().find(|x| t.is_rowid_col(x.1)) {
                levels[i].access = Access::RowidIn(conj[ci].clone());
                consumed[ci] = true;
                continue;
            }
            for &(ci, col) in &ins {
                if let Some(xi) = t.indexes.iter().position(|ix| ix.cols.first().map_or(false, |&c| c as u32 == col)) {
                    levels[i].access = Access::IndexIn(xi, conj[ci].clone());
                    consumed[ci] = true;
                    break;
                }
            }
        }
        sel.pre.clear();
        for (ci, c) in conj.into_iter().enumerate() {
            if consumed[ci] {
                continue;
            }
            let m = mask(&c, 0);
            if m == 0 {
                sel.pre.push(c);
            } else {
                let lvl = (63 - m.leading_zeros()) as usize;
                match levels.get_mut(lvl) {
                    Some(l) => l.conds.push(c),
                    None => return Err(String::from("internal: bad source mask")),
                }
            }
        }
        sel.levels = levels;
        // which sources' row values are actually read
        let mut need = 0u64;
        {
            let mut acc = |e: &Ex| need |= mask(e, 0);
            for c in &sel.cols {
                acc(&c.ex)
            }
            for l in &sel.levels {
                for c in &l.conds {
                    acc(c)
                }
                match &l.access {
                    Access::Rowid(e, _) => acc(e),
                    Access::Index(_, p) => {
                        for (e, _) in p {
                            acc(e)
                        }
                    }
                    _ => {}
                }
            }
            for g in &sel.group {
                acc(g)
            }
            if let Some(h) = &sel.having {
                acc(h)
            }
            for (o, _) in &sel.order {
                acc(o)
            }
            for a in &sel.aggs {
                if let Some(x) = &a.arg {
                    acc(x)
                }
            }
        }
        for (i, l) in sel.levels.iter_mut().enumerate() {
            l.need_row = need & (1u64 << i) != 0;
        }
        let mut rep = 0u64;
        for c in &sel.cols {
            rep |= mask(&c.ex, 0);
        }
        if let Some(h) = &sel.having {
            rep |= mask(h, 0);
        }
        for (o, _) in &sel.order {
            rep |= mask(o, 0);
        }
        sel.needs_rep = rep != 0;
        // ORDER BY satisfied by the access order of the outermost source?
        sel.sort = !sel.order.is_empty();
        if sel.sort && !sel.is_agg && !sel.distinct && n > 0 && sel.from[0].sub.is_none() {
            let desc = sel.order[0].1;
            if sel.order.iter().all(|o| o.1 == desc) {
                let t = self.table(sel.from[0].name.as_deref().unwrap_or(""))?;
                let col_of = |e: &Ex| -> Option<u32> {
                    match e {
                        Ex::Col { up: 0, src: 0, col, .. } => Some(*col),
                        _ => None,
                    }
                };
                let ok = match &sel.levels[0].access {
                    Access::Scan => sel.order.len() == 1 && col_of(&sel.order[0].0).map_or(false, |c| t.is_rowid_col(c)),
                    Access::Rowid(..) => true,
                    Access::Index(xi, probes) => {
                        let idx = &t.indexes[*xi];
                        let k = probes.len();
                        sel.order.len() <= idx.cols.len() - k
                            && sel.order.iter().enumerate().all(|(j, (e, _))| col_of(e) == Some(idx.cols[k + j] as u32))
                    }
                    _ => false,
                };
                if ok {
                    sel.sort = false;
                    sel.levels[0].reverse = desc;
                }
            }
        }
        Ok(())
    }

    fn compile(&self, st: &mut Stmt) -> Result<(), String> {
        let mut ids = 0u32;
        match st {
            Stmt::Select(sel) => self.compile_select(sel, None, &mut ids),
            Stmt::Delete { sel, .. } => self.compile_select(sel, None, &mut ids),
            Stmt::Update { table, sets, set_cols, sel } => {
                let t = self.table(table)?;
                set_cols.clear();
                sel.cols.clear();
                for (c, e) in sets.iter() {
                    let ci = match t.col_index(c) {
                        Some(ci) => ci as u32,
                        None if is_rowid_name(c) => ROWID,
                        None => return Err(format!("no such column: {}", c)),
                    };
                    set_cols.push(ci);
                    sel.cols.push(RCol { ex: e.clone(), star: None, alias: Some(String::new()), span: String::new() });
                }
                self.compile_select(sel, None, &mut ids)
            }
            Stmt::Insert(ins) => {
                let t = self.table(&ins.table)?;
                ins.table = ins.table.to_ascii_lowercase();
                ins.cols.clear();
                match &ins.col_names {
                    Some(names) => {
                        for n in names {
                            let ci = match t.col_index(n) {
                                Some(ci) => ci as u32,
                                None if is_rowid_name(n) => ROWID,
                                None => return Err(format!("table {} has no column named {}", t.name, n)),
                            };
                            ins.cols.push(ci);
                        }
                    }
                    None => ins.cols = (0..t.cols.len() as u32).collect(),
                }
                let empty = Scope { srcs: &[], parent: None };
                for r in ins.rows.iter_mut() {
                    if !r.is_empty() && r.len() != ins.cols.len() {
                        if ins.col_names.is_none() {
                            return Err(format!("table {} has {} columns but {} values were supplied", t.name, t.cols.len(), r.len()));
                        }
                        return Err(format!("{} values for {} columns", r.len(), ins.cols.len()));
                    }
                    for e in r.iter_mut() {
                        self.resolve(e, &empty, &mut ids)?;
                        if has_agg(e) {
                            return Err(String::from("misuse of aggregate function"));
                        }
                    }
                }
                if let Some(u) = &mut ins.upsert {
                    u.target = None;
                    if !u.target_names.is_empty() {
                        let mut cols = Vec::new();
                        for n in &u.target_names {
                            match t.col_index(n) {
                                Some(c) => cols.push(c),
                                None => return Err(format!("no such column: {}", n)),
                            }
                        }
                        if cols.len() == 1 && t.alias == Some(cols[0]) {
                            u.target = Some(Target::Rowid);
                        } else {
                            let mut sc = cols.clone();
                            sc.sort_unstable();
                            sc.dedup();
                            for (xi, idx) in t.indexes.iter().enumerate() {
                                let mut ic = idx.cols.clone();
                                ic.sort_unstable();
                                if idx.unique && ic == sc {
                                    u.target = Some(Target::Index(xi));
                                    break;
                                }
                            }
                            if u.target.is_none() {
                                return Err(String::from("ON CONFLICT clause does not match any PRIMARY KEY or UNIQUE constraint"));
                            }
                        }
                    } else if u.sets.is_some() {
                        return Err(String::from("sqlmini: DO UPDATE requires a conflict target"));
                    }
                    let cols: Vec<(String, Aff)> = t.cols.iter().map(|c| (c.name.clone(), c.aff)).collect();
                    let srcs = [
                        SrcInfo { name: ins.alias.clone().unwrap_or_else(|| t.name.clone()), cols: cols.clone(), alias: t.alias, table: true },
                        SrcInfo { name: String::from("excluded"), cols, alias: t.alias, table: true },
                    ];
                    let sc = Scope { srcs: &srcs, parent: None };
                    u.sets_c.clear();
                    if let Some(sets) = &u.sets {
                        for (c, e) in sets {
                            let ci = match t.col_index(c) {
                                Some(ci) => ci as u32,
                                None => return Err(format!("no such column: {}", c)),
                            };
                            let mut e = e.clone();
                            self.resolve(&mut e, &sc, &mut ids)?;
                            u.sets_c.push((ci, e));
                        }
                    }
                    if let Some(w) = &mut u.where_ {
                        self.resolve(w, &sc, &mut ids)?;
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

// ════════════════════════════════════════════════════════════════════════
// Execution
// ════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy)]
struct Env<'a> {
    slots: &'a [Option<(i64, &'a [SqlVal])>],
    aggs: &'a [SqlVal],
    parent: Option<&'a Env<'a>>,
}

struct InSet {
    set: BTreeSet<KeyV>,
    has_null: bool,
}

enum SubRes {
    Set(InSet),
    Val(SqlVal),
}

struct Ctx<'p> {
    params: &'p [SqlVal],
    cache: RefCell<BTreeMap<u32, Rc<SubRes>>>,
}

impl<'p> Ctx<'p> {
    fn new(params: &'p [SqlVal]) -> Ctx<'p> {
        Ctx { params, cache: RefCell::new(BTreeMap::new()) }
    }
    fn cached(&self, id: u32) -> Option<Rc<SubRes>> {
        self.cache.borrow().get(&id).cloned()
    }
    fn store(&self, id: u32, r: Rc<SubRes>) {
        self.cache.borrow_mut().insert(id, r);
    }
}

enum SrcData<'a> {
    T(&'a Table),
    R(&'a [Vec<SqlVal>]),
}

type Emit<'f> = dyn FnMut(&Env) -> Result<bool, String> + 'f;

#[derive(Clone)]
enum AccSt {
    Count(i64),
    Sum { i: i64, r: f64, approx: bool, any: bool, ovf: bool },
    Total(f64),
    Avg(f64, i64),
    Min(Option<SqlVal>),
    Max(Option<SqlVal>),
}

#[derive(Clone)]
struct Acc {
    st: AccSt,
    seen: Option<BTreeSet<KeyV>>,
}

impl Acc {
    fn new(d: &AggDef) -> Acc {
        let st = match d.f {
            AggF::Count => AccSt::Count(0),
            AggF::Sum => AccSt::Sum { i: 0, r: 0.0, approx: false, any: false, ovf: false },
            AggF::Total => AccSt::Total(0.0),
            AggF::Avg => AccSt::Avg(0.0, 0),
            AggF::Min => AccSt::Min(None),
            AggF::Max => AccSt::Max(None),
        };
        Acc { st, seen: if d.distinct { Some(BTreeSet::new()) } else { None } }
    }

    fn step(&mut self, v: Option<SqlVal>) {
        let v = match v {
            None => {
                if let AccSt::Count(n) = &mut self.st {
                    *n += 1;
                }
                return;
            }
            Some(v) => v,
        };
        if v == SqlVal::Null {
            return;
        }
        if let Some(seen) = &mut self.seen {
            if !seen.insert(KeyV::of(&v)) {
                return;
            }
        }
        match &mut self.st {
            AccSt::Count(n) => *n += 1,
            AccSt::Sum { i, r, approx, any, ovf } => {
                *any = true;
                let nv = match &v {
                    SqlVal::Text(s) => match parse_num_full(s) {
                        Some(x) => x,
                        None => SqlVal::Real(text_to_real(s)),
                    },
                    o => o.clone(),
                };
                match nv {
                    SqlVal::Int(x) if !*approx => match i.checked_add(x) {
                        Some(s) => *i = s,
                        None => {
                            *ovf = true;
                            *approx = true;
                            *r = *i as f64 + x as f64;
                        }
                    },
                    other => {
                        if !*approx {
                            *approx = true;
                            *r = *i as f64;
                        }
                        *r += num_f64(&other);
                    }
                }
            }
            AccSt::Total(t) => *t += num_f64(&to_num(v)),
            AccSt::Avg(s, n) => {
                *s += num_f64(&to_num(v));
                *n += 1;
            }
            AccSt::Min(m) => {
                if m.as_ref().map_or(true, |c| cmp_vals(&v, c) == Ordering::Less) {
                    *m = Some(v);
                }
            }
            AccSt::Max(m) => {
                if m.as_ref().map_or(true, |c| cmp_vals(&v, c) == Ordering::Greater) {
                    *m = Some(v);
                }
            }
        }
    }

    fn finish(&self) -> Result<SqlVal, String> {
        let v = self.finish_raw()?;
        // NaN results (e.g. inf + -inf) are NULL in SQLite
        Ok(match v {
            SqlVal::Real(r) if r.is_nan() => SqlVal::Null,
            o => o,
        })
    }

    fn finish_raw(&self) -> Result<SqlVal, String> {
        Ok(match &self.st {
            AccSt::Count(n) => SqlVal::Int(*n),
            AccSt::Sum { i, r, approx, any, ovf } => {
                if !*any {
                    SqlVal::Null
                } else if *ovf {
                    return Err(String::from("integer overflow"));
                } else if *approx {
                    SqlVal::Real(*r)
                } else {
                    SqlVal::Int(*i)
                }
            }
            AccSt::Total(t) => SqlVal::Real(*t),
            AccSt::Avg(s, n) => {
                if *n == 0 {
                    SqlVal::Null
                } else {
                    SqlVal::Real(*s / *n as f64)
                }
            }
            AccSt::Min(m) | AccSt::Max(m) => m.clone().unwrap_or(SqlVal::Null),
        })
    }
}

fn row_to_rid(v: &SqlVal) -> Option<i64> {
    match v {
        SqlVal::Int(i) => Some(*i),
        SqlVal::Real(r) => {
            if *r >= -9223372036854775808.0 && *r < 9223372036854775808.0 {
                let i = *r as i64;
                if i as f64 == *r {
                    return Some(i);
                }
            }
            None
        }
        _ => None,
    }
}

fn arith(op: BOp, a: SqlVal, b: SqlVal) -> SqlVal {
    if a == SqlVal::Null || b == SqlVal::Null {
        return SqlVal::Null;
    }
    let a = to_num(a);
    let b = to_num(b);
    if let (SqlVal::Int(x), SqlVal::Int(y)) = (&a, &b) {
        let (x, y) = (*x, *y);
        let r = match op {
            BOp::Add => x.checked_add(y),
            BOp::Sub => x.checked_sub(y),
            BOp::Mul => x.checked_mul(y),
            BOp::Div => {
                if y == 0 {
                    return SqlVal::Null;
                }
                x.checked_div(y)
            }
            BOp::Rem => {
                if y == 0 {
                    return SqlVal::Null;
                }
                let y = if y == -1 { 1 } else { y };
                x.checked_rem(y)
            }
            _ => None,
        };
        if let Some(r) = r {
            return SqlVal::Int(r);
        }
    }
    let x = num_f64(&a);
    let y = num_f64(&b);
    let r = match op {
        BOp::Add => x + y,
        BOp::Sub => x - y,
        BOp::Mul => x * y,
        BOp::Div => {
            if y == 0.0 {
                return SqlVal::Null;
            }
            x / y
        }
        BOp::Rem => {
            let ix = x as i64;
            let iy = y as i64;
            if iy == 0 {
                return SqlVal::Null;
            }
            let iy = if iy == -1 { 1 } else { iy };
            (ix.wrapping_rem(iy)) as f64
        }
        _ => return SqlVal::Null,
    };
    if r.is_nan() {
        SqlVal::Null
    } else {
        SqlVal::Real(r)
    }
}

/// Comparison with affinity; returns None when NULL is involved (non-IS ops).
fn compare(op: BOp, l: SqlVal, la: Aff, r: SqlVal, ra: Aff) -> SqlVal {
    let x = cmp_affinity(la, ra);
    let is = matches!(op, BOp::Is | BOp::IsNot);
    if l == SqlVal::Null || r == SqlVal::Null {
        if is {
            let eq = l == SqlVal::Null && r == SqlVal::Null;
            return SqlVal::Int(((op == BOp::Is) == eq) as i64);
        }
        return SqlVal::Null;
    }
    let l = apply_cmp_aff(x, l);
    let r = apply_cmp_aff(x, r);
    let o = cmp_vals(&l, &r);
    let b = match op {
        BOp::Eq | BOp::Is => o == Ordering::Equal,
        BOp::Ne | BOp::IsNot => o != Ordering::Equal,
        BOp::Lt => o == Ordering::Less,
        BOp::Le => o != Ordering::Greater,
        BOp::Gt => o == Ordering::Greater,
        BOp::Ge => o != Ordering::Less,
        _ => false,
    };
    SqlVal::Int(b as i64)
}

fn cmp_keyrows(a: &[SqlVal], b: &[SqlVal], order: &[(Ex, bool)]) -> Ordering {
    for (i, (_, desc)) in order.iter().enumerate() {
        let o = cmp_vals(a.get(i).unwrap_or(&SqlVal::Null), b.get(i).unwrap_or(&SqlVal::Null));
        let o = if *desc { o.reverse() } else { o };
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}

impl Db {
    fn eval(&self, ctx: &Ctx, e: &Ex, env: &Env) -> Result<SqlVal, String> {
        Ok(match e {
            Ex::Lit(v) => v.clone(),
            Ex::Param(i) => ctx.params.get(*i).map_or(SqlVal::Null, norm_param),
            Ex::Col { up, src, col, .. } => {
                let mut en = env;
                for _ in 0..*up {
                    en = match en.parent {
                        Some(p) => p,
                        None => return Ok(SqlVal::Null),
                    };
                }
                match en.slots.get(*src as usize) {
                    Some(Some((rid, vals))) => {
                        if *col == ROWID {
                            SqlVal::Int(*rid)
                        } else {
                            vals.get(*col as usize).cloned().unwrap_or(SqlVal::Null)
                        }
                    }
                    _ => SqlVal::Null,
                }
            }
            Ex::Agg(i) => env.aggs.get(*i as usize).cloned().unwrap_or(SqlVal::Null),
            Ex::Un(op, a) => {
                let v = self.eval(ctx, a, env)?;
                match op {
                    UOp::Pos => v,
                    UOp::Not => match truthy(&v) {
                        None => SqlVal::Null,
                        Some(b) => SqlVal::Int((!b) as i64),
                    },
                    UOp::Neg => match to_num(v) {
                        SqlVal::Int(i) => match i.checked_neg() {
                            Some(n) => SqlVal::Int(n),
                            None => SqlVal::Real(-(i as f64)),
                        },
                        SqlVal::Real(r) => SqlVal::Real(-r),
                        _ => SqlVal::Null,
                    },
                }
            }
            Ex::Bin(op, a, b) => match op {
                BOp::And => {
                    let x = truthy(&self.eval(ctx, a, env)?);
                    if x == Some(false) {
                        return Ok(SqlVal::Int(0));
                    }
                    let y = truthy(&self.eval(ctx, b, env)?);
                    match (x, y) {
                        (_, Some(false)) => SqlVal::Int(0),
                        (Some(true), Some(true)) => SqlVal::Int(1),
                        _ => SqlVal::Null,
                    }
                }
                BOp::Or => {
                    let x = truthy(&self.eval(ctx, a, env)?);
                    if x == Some(true) {
                        return Ok(SqlVal::Int(1));
                    }
                    let y = truthy(&self.eval(ctx, b, env)?);
                    match (x, y) {
                        (_, Some(true)) => SqlVal::Int(1),
                        (Some(false), Some(false)) => SqlVal::Int(0),
                        _ => SqlVal::Null,
                    }
                }
                BOp::Eq | BOp::Ne | BOp::Lt | BOp::Le | BOp::Gt | BOp::Ge | BOp::Is | BOp::IsNot => {
                    let l = self.eval(ctx, a, env)?;
                    let r = self.eval(ctx, b, env)?;
                    compare(*op, l, expr_aff(a), r, expr_aff(b))
                }
                BOp::Concat => {
                    let l = self.eval(ctx, a, env)?;
                    let r = self.eval(ctx, b, env)?;
                    if l == SqlVal::Null || r == SqlVal::Null {
                        SqlVal::Null
                    } else {
                        let mut s = val_to_text(&l);
                        s.push_str(&val_to_text(&r));
                        SqlVal::Text(s)
                    }
                }
                _ => {
                    let l = self.eval(ctx, a, env)?;
                    let r = self.eval(ctx, b, env)?;
                    arith(*op, l, r)
                }
            },
            Ex::Between(x, lo, hi, not) => {
                let v = self.eval(ctx, x, env)?;
                let l = self.eval(ctx, lo, env)?;
                let h = self.eval(ctx, hi, env)?;
                let a = truthy(&compare(BOp::Ge, v.clone(), expr_aff(x), l, expr_aff(lo)));
                let b = truthy(&compare(BOp::Le, v, expr_aff(x), h, expr_aff(hi)));
                let r = match (a, b) {
                    (Some(false), _) | (_, Some(false)) => Some(false),
                    (Some(true), Some(true)) => Some(true),
                    _ => None,
                };
                match r {
                    None => SqlVal::Null,
                    Some(b) => SqlVal::Int((b != *not) as i64),
                }
            }
            Ex::InList(x, list, not) => {
                let v = self.eval(ctx, x, env)?;
                if list.is_empty() {
                    return Ok(SqlVal::Int(*not as i64));
                }
                if v == SqlVal::Null {
                    return Ok(SqlVal::Null);
                }
                let mut found = false;
                let mut saw_null = false;
                for it in list {
                    let iv = self.eval(ctx, it, env)?;
                    if iv == SqlVal::Null {
                        saw_null = true;
                        continue;
                    }
                    if compare(BOp::Eq, v.clone(), expr_aff(x), iv, expr_aff(it)) == SqlVal::Int(1) {
                        found = true;
                        break;
                    }
                }
                if found {
                    SqlVal::Int((!*not) as i64)
                } else if saw_null {
                    SqlVal::Null
                } else {
                    SqlVal::Int(*not as i64)
                }
            }
            Ex::Sub(s) => match s.kind {
                SubKind::Scalar => {
                    if !s.corr {
                        if let Some(r) = ctx.cached(s.id) {
                            if let SubRes::Val(v) = &*r {
                                return Ok(v.clone());
                            }
                        }
                    }
                    let rows = self.exec_select(ctx, &s.sel, Some(env), 1)?;
                    let v = rows.into_iter().next().and_then(|r| r.into_iter().next()).unwrap_or(SqlVal::Null);
                    if !s.corr {
                        ctx.store(s.id, Rc::new(SubRes::Val(v.clone())));
                    }
                    v
                }
                SubKind::Exists => {
                    let rows = self.exec_select(ctx, &s.sel, Some(env), 1)?;
                    SqlVal::Int(((!rows.is_empty()) != s.not) as i64)
                }
                SubKind::In => {
                    let lv = match &s.lhs {
                        Some(l) => self.eval(ctx, l, env)?,
                        None => SqlVal::Null,
                    };
                    let set = self.in_set(ctx, s, env)?;
                    let (set, has_null) = match &*set {
                        SubRes::Set(st) => (&st.set, st.has_null),
                        _ => return Err(String::from("internal: bad IN cache")),
                    };
                    if set.is_empty() && !has_null {
                        return Ok(SqlVal::Int(s.not as i64));
                    }
                    if lv == SqlVal::Null {
                        return Ok(SqlVal::Null);
                    }
                    let k = KeyV::of(&apply_cmp_aff(s.aff, lv));
                    if set.contains(&k) {
                        SqlVal::Int((!s.not) as i64)
                    } else if has_null {
                        SqlVal::Null
                    } else {
                        SqlVal::Int(s.not as i64)
                    }
                }
            },
            Ex::Func(n, args, _, _) => {
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(self.eval(ctx, a, env)?);
                }
                scalar_fn(n, vals)?
            }
            Ex::Name(_, c) => return Err(format!("no such column: {}", c)),
        })
    }

    fn in_set(&self, ctx: &Ctx, s: &SubQ, env: &Env) -> Result<Rc<SubRes>, String> {
        if !s.corr {
            if let Some(r) = ctx.cached(s.id) {
                return Ok(r);
            }
        }
        let rows = self.exec_select(ctx, &s.sel, Some(env), usize::MAX)?;
        let mut set = BTreeSet::new();
        let mut has_null = false;
        for r in rows {
            match r.into_iter().next() {
                Some(SqlVal::Null) | None => has_null = true,
                Some(v) => {
                    set.insert(KeyV::of(&apply_cmp_aff(s.aff, v)));
                }
            }
        }
        let r = Rc::new(SubRes::Set(InSet { set, has_null }));
        if !s.corr {
            ctx.store(s.id, r.clone());
        }
        Ok(r)
    }

    #[allow(clippy::too_many_arguments)]
    fn visit<'a>(
        &self,
        ctx: &Ctx,
        sel: &Select,
        data: &[SrcData<'a>],
        lvl: usize,
        slots: &mut Vec<Option<(i64, &'a [SqlVal])>>,
        outer: Option<&Env>,
        emit: &mut Emit,
        rid: i64,
        vals: &'a [SqlVal],
    ) -> Result<bool, String> {
        slots.truncate(lvl);
        slots.push(Some((rid, vals)));
        {
            let env = Env { slots: &slots[..], aggs: &[], parent: outer };
            for c in &sel.levels[lvl].conds {
                if truthy(&self.eval(ctx, c, &env)?) != Some(true) {
                    return Ok(true);
                }
            }
            if lvl + 1 == data.len() {
                return emit(&env);
            }
        }
        self.scan(ctx, sel, data, lvl + 1, slots, outer, emit)
    }

    fn scan<'a>(
        &self,
        ctx: &Ctx,
        sel: &Select,
        data: &[SrcData<'a>],
        lvl: usize,
        slots: &mut Vec<Option<(i64, &'a [SqlVal])>>,
        outer: Option<&Env>,
        emit: &mut Emit,
    ) -> Result<bool, String> {
        let level = match sel.levels.get(lvl) {
            Some(l) => l,
            None => return Err(String::from("internal: missing level")),
        };
        match data.get(lvl) {
            None => Err(String::from("internal: missing source")),
            Some(SrcData::R(rows)) => {
                let rows: &'a [Vec<SqlVal>] = *rows;
                for (i, r) in rows.iter().enumerate() {
                    if !self.visit(ctx, sel, data, lvl, slots, outer, emit, i as i64 + 1, &r[..])? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Some(SrcData::T(t)) => {
                let t: &'a Table = *t;
                match &level.access {
                    Access::Scan => {
                        let it: Box<dyn Iterator<Item = (&'a i64, &'a Vec<SqlVal>)> + 'a> =
                            if level.reverse { Box::new(t.rows.iter().rev()) } else { Box::new(t.rows.iter()) };
                        for (rid, v) in it {
                            if !self.visit(ctx, sel, data, lvl, slots, outer, emit, *rid, &v[..])? {
                                return Ok(false);
                            }
                        }
                        Ok(true)
                    }
                    Access::Rowid(e, x) => {
                        let v = {
                            let env = Env { slots: &slots[..lvl.min(slots.len())], aggs: &[], parent: outer };
                            apply_cmp_aff(*x, self.eval(ctx, e, &env)?)
                        };
                        if let Some(rid) = row_to_rid(&v) {
                            if let Some(vals) = t.rows.get(&rid) {
                                return self.visit(ctx, sel, data, lvl, slots, outer, emit, rid, &vals[..]);
                            }
                        }
                        Ok(true)
                    }
                    Access::Index(xi, probes) => {
                        let mut lo = Vec::with_capacity(probes.len() + 1);
                        {
                            let env = Env { slots: &slots[..lvl.min(slots.len())], aggs: &[], parent: outer };
                            for (e, x) in probes {
                                let v = apply_cmp_aff(*x, self.eval(ctx, e, &env)?);
                                if v == SqlVal::Null {
                                    return Ok(true);
                                }
                                lo.push(KeyV::of(&v));
                            }
                        }
                        let idx = t.indexes.get(*xi).ok_or_else(|| String::from("internal: bad index"))?;
                        self.scan_index(ctx, sel, data, lvl, slots, outer, emit, t, idx, lo, level.reverse, level.need_row)
                    }
                    Access::RowidIn(e) | Access::IndexIn(_, e) => {
                        let s = match e {
                            Ex::Sub(s) => s,
                            _ => return Err(String::from("internal: bad IN access")),
                        };
                        let res = {
                            let env = Env { slots: &[], aggs: &[], parent: outer };
                            self.in_set(ctx, s, &env)?
                        };
                        let set = match &*res {
                            SubRes::Set(st) => &st.set,
                            _ => return Err(String::from("internal: bad IN cache")),
                        };
                        for k in set.iter() {
                            match &level.access {
                                Access::RowidIn(_) => {
                                    if let Some(rid) = row_to_rid(&k.val()) {
                                        if let Some(vals) = t.rows.get(&rid) {
                                            if !self.visit(ctx, sel, data, lvl, slots, outer, emit, rid, &vals[..])? {
                                                return Ok(false);
                                            }
                                        }
                                    }
                                }
                                Access::IndexIn(xi, _) => {
                                    if *k == KeyV::Null {
                                        continue;
                                    }
                                    let idx = t.indexes.get(*xi).ok_or_else(|| String::from("internal: bad index"))?;
                                    if !self.scan_index(ctx, sel, data, lvl, slots, outer, emit, t, idx, vec![k.clone()], false, true)? {
                                        return Ok(false);
                                    }
                                }
                                _ => {}
                            }
                        }
                        Ok(true)
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn scan_index<'a>(
        &self,
        ctx: &Ctx,
        sel: &Select,
        data: &[SrcData<'a>],
        lvl: usize,
        slots: &mut Vec<Option<(i64, &'a [SqlVal])>>,
        outer: Option<&Env>,
        emit: &mut Emit,
        t: &'a Table,
        idx: &'a Index,
        lo: Vec<KeyV>,
        reverse: bool,
        need_row: bool,
    ) -> Result<bool, String> {
        let (lo, hi) = IKey::bounds(lo, idx.cols.len());
        let r = idx.set.range(lo..=hi);
        let it: Box<dyn Iterator<Item = &'a IKey> + 'a> = if reverse { Box::new(r.rev()) } else { Box::new(r) };
        for k in it {
            let rid = k.rid;
            let vals: &'a [SqlVal] = if need_row {
                match t.rows.get(&rid) {
                    Some(v) => &v[..],
                    None => continue,
                }
            } else {
                &[]
            };
            if !self.visit(ctx, sel, data, lvl, slots, outer, emit, rid, vals)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Runs the FROM/WHERE part of `sel`, calling `emit` for each joined row.
    fn run_scan(&self, ctx: &Ctx, sel: &Select, outer: Option<&Env>, emit: &mut Emit) -> Result<(), String> {
        {
            let base = Env { slots: &[], aggs: &[], parent: outer };
            for c in &sel.pre {
                if truthy(&self.eval(ctx, c, &base)?) != Some(true) {
                    return Ok(());
                }
            }
            if sel.from.is_empty() {
                emit(&base)?;
                return Ok(());
            }
        }
        let mut mats: Vec<Vec<Vec<SqlVal>>> = Vec::with_capacity(sel.from.len());
        for f in &sel.from {
            match &f.sub {
                Some(s) => mats.push(self.exec_select(ctx, s, outer, usize::MAX)?),
                None => mats.push(Vec::new()),
            }
        }
        let mut data: Vec<SrcData> = Vec::with_capacity(sel.from.len());
        for (i, f) in sel.from.iter().enumerate() {
            if f.sub.is_some() {
                data.push(SrcData::R(mats.get(i).map_or(&[][..], |v| &v[..])));
            } else {
                let t = self.tables.get(f.name.as_deref().unwrap_or("")).ok_or_else(|| String::from("no such table"))?;
                data.push(SrcData::T(t));
            }
        }
        let mut slots = Vec::with_capacity(sel.from.len());
        self.scan(ctx, sel, &data, 0, &mut slots, outer, emit)?;
        Ok(())
    }

    fn exec_select(&self, ctx: &Ctx, sel: &Select, outer: Option<&Env>, cap: usize) -> Result<Vec<Vec<SqlVal>>, String> {
        let (limit, offset) = {
            let base = Env { slots: &[], aggs: &[], parent: outer };
            let limit = match &sel.limit {
                Some(e) => {
                    let v = must_int(self.eval(ctx, e, &base)?)?;
                    if v < 0 {
                        None
                    } else {
                        Some(v as usize)
                    }
                }
                None => None,
            };
            let offset = match &sel.offset {
                Some(e) => must_int(self.eval(ctx, e, &base)?)?.max(0) as usize,
                None => 0,
            };
            (limit, offset)
        };
        let take = match limit {
            Some(l) => l.min(cap),
            None => cap,
        };
        if take == 0 {
            return Ok(Vec::new());
        }
        let mut out: Vec<Vec<SqlVal>> = Vec::new();
        let mut keyed: Vec<(Vec<SqlVal>, Vec<SqlVal>)> = Vec::new();
        let mut seen: BTreeSet<Vec<KeyV>> = BTreeSet::new();
        if !sel.is_agg {
            let mut skip = offset;
            self.run_scan(ctx, sel, outer, &mut |env: &Env| {
                let mut row = Vec::with_capacity(sel.cols.len());
                for c in &sel.cols {
                    row.push(self.eval(ctx, &c.ex, env)?);
                }
                if sel.distinct && !seen.insert(row.iter().map(KeyV::of).collect()) {
                    return Ok(true);
                }
                if sel.sort {
                    let mut k = Vec::with_capacity(sel.order.len());
                    for (o, _) in &sel.order {
                        k.push(self.eval(ctx, o, env)?);
                    }
                    keyed.push((k, row));
                    return Ok(true);
                }
                if skip > 0 {
                    skip -= 1;
                    return Ok(true);
                }
                out.push(row);
                Ok(out.len() < take)
            })?;
            if sel.sort {
                keyed.sort_by(|a, b| cmp_keyrows(&a.0, &b.0, &sel.order));
                out = keyed.into_iter().skip(offset).take(take).map(|x| x.1).collect();
            }
            return Ok(out);
        }
        // ── aggregate query ──
        struct Grp {
            /// last row of the group: owned copies (FROM-subqueries) ...
            rep: Option<Vec<Option<(i64, Vec<SqlVal>)>>>,
            /// ... or just rowids when every source is a table
            rids: Vec<Option<i64>>,
            acc: Vec<Acc>,
        }
        let all_tables = sel.from.iter().all(|f| f.sub.is_none());
        let new_grp = || Grp { rep: None, rids: Vec::new(), acc: sel.aggs.iter().map(Acc::new).collect() };
        let mut groups: Vec<Grp> = Vec::new();
        let mut gmap: BTreeMap<Vec<KeyV>, usize> = BTreeMap::new();
        if sel.group.is_empty() {
            groups.push(new_grp());
        }
        if let Some(n) = self.fast_count(ctx, sel, outer)? {
            if let Some(g) = groups.get_mut(0) {
                for a in g.acc.iter_mut() {
                    a.st = AccSt::Count(n);
                }
            }
        } else {
        self.run_scan(ctx, sel, outer, &mut |env: &Env| {
            let gi = if sel.group.is_empty() {
                0
            } else {
                let mut k = Vec::with_capacity(sel.group.len());
                for g in &sel.group {
                    k.push(KeyV::of(&self.eval(ctx, g, env)?));
                }
                match gmap.get(&k) {
                    Some(&i) => i,
                    None => {
                        groups.push(new_grp());
                        gmap.insert(k, groups.len() - 1);
                        groups.len() - 1
                    }
                }
            };
            for (ai, d) in sel.aggs.iter().enumerate() {
                let v = match &d.arg {
                    Some(e) => Some(self.eval(ctx, e, env)?),
                    None => None,
                };
                if let Some(a) = groups.get_mut(gi).and_then(|g| g.acc.get_mut(ai)) {
                    a.step(v);
                }
            }
            let g = match groups.get_mut(gi) {
                Some(g) => g,
                None => return Err(String::from("internal: bad group")),
            };
            if sel.needs_rep {
                if all_tables {
                    g.rids.clear();
                    g.rids.extend(env.slots.iter().map(|s| s.map(|(r, _)| r)));
                } else {
                    g.rep = Some(env.slots.iter().map(|s| s.map(|(r, v)| (r, v.to_vec()))).collect());
                }
            }
            Ok(true)
        })?;
        }
        let order_ix: Vec<usize> = if sel.group.is_empty() { vec![0] } else { gmap.values().copied().collect() };
        let nsrc = sel.from.len();
        for gi in order_ix {
            let g = match groups.get(gi) {
                Some(g) => g,
                None => continue,
            };
            let mut aggvals = Vec::with_capacity(g.acc.len());
            for a in &g.acc {
                aggvals.push(a.finish()?);
            }
            let slots: Vec<Option<(i64, &[SqlVal])>> = match &g.rep {
                Some(r) => r.iter().map(|s| s.as_ref().map(|(rid, v)| (*rid, &v[..]))).collect(),
                None if !g.rids.is_empty() => {
                    let mut v = Vec::with_capacity(nsrc);
                    for (i, r) in g.rids.iter().enumerate() {
                        let t = sel.from.get(i).and_then(|f| self.tables.get(f.name.as_deref().unwrap_or("")));
                        v.push(match (r, t) {
                            (Some(rid), Some(t)) => t.rows.get(rid).map(|row| (*rid, &row[..])),
                            _ => None,
                        });
                    }
                    v
                }
                None => vec![None; nsrc],
            };
            let env = Env { slots: &slots, aggs: &aggvals, parent: outer };
            if let Some(h) = &sel.having {
                if truthy(&self.eval(ctx, h, &env)?) != Some(true) {
                    continue;
                }
            }
            let mut row = Vec::with_capacity(sel.cols.len());
            for c in &sel.cols {
                row.push(self.eval(ctx, &c.ex, &env)?);
            }
            if sel.distinct && !seen.insert(row.iter().map(KeyV::of).collect()) {
                continue;
            }
            let mut k = Vec::with_capacity(sel.order.len());
            for (o, _) in &sel.order {
                k.push(self.eval(ctx, o, &env)?);
            }
            keyed.push((k, row));
        }
        if !sel.order.is_empty() {
            keyed.sort_by(|a, b| cmp_keyrows(&a.0, &b.0, &sel.order));
        }
        Ok(keyed.into_iter().skip(offset).take(take).map(|x| x.1).collect())
    }

    /// `SELECT COUNT(*) FROM t [WHERE <fully index-answered>]`: count rowid
    /// map / index entries directly (like SQLite's OP_Count).
    fn fast_count(&self, ctx: &Ctx, sel: &Select, outer: Option<&Env>) -> Result<Option<i64>, String> {
        if !sel.group.is_empty()
            || sel.needs_rep
            || sel.levels.len() != 1
            || sel.from.len() != 1
            || sel.from[0].sub.is_some()
            || !sel.levels[0].conds.is_empty()
            || !sel.aggs.iter().all(|a| a.f == AggF::Count && a.arg.is_none() && !a.distinct)
        {
            return Ok(None);
        }
        let base = Env { slots: &[], aggs: &[], parent: outer };
        for c in &sel.pre {
            if truthy(&self.eval(ctx, c, &base)?) != Some(true) {
                return Ok(Some(0));
            }
        }
        let t = match self.tables.get(sel.from[0].name.as_deref().unwrap_or("")) {
            Some(t) => t,
            None => return Ok(None),
        };
        Ok(match &sel.levels[0].access {
            Access::Scan => Some(t.rows.len() as i64),
            Access::Rowid(e, x) => {
                let v = apply_cmp_aff(*x, self.eval(ctx, e, &base)?);
                Some(row_to_rid(&v).map_or(0, |r| t.rows.contains_key(&r) as i64))
            }
            Access::Index(xi, probes) => {
                let idx = match t.indexes.get(*xi) {
                    Some(i) => i,
                    None => return Ok(None),
                };
                let mut lo = Vec::with_capacity(probes.len());
                for (e, x) in probes {
                    let v = apply_cmp_aff(*x, self.eval(ctx, e, &base)?);
                    if v == SqlVal::Null {
                        return Ok(Some(0));
                    }
                    lo.push(KeyV::of(&v));
                }
                let (lo, hi) = IKey::bounds(lo, idx.cols.len());
                Some(idx.set.range(lo..=hi).count() as i64)
            }
            _ => None,
        })
    }

    /// Rowids (and evaluated result columns) matched by a DELETE/UPDATE plan.
    fn collect(&self, ctx: &Ctx, sel: &Select) -> Result<Vec<(i64, Vec<SqlVal>)>, String> {
        let mut out = Vec::new();
        self.run_scan(ctx, sel, None, &mut |env: &Env| {
            let rid = match env.slots.first() {
                Some(Some((r, _))) => *r,
                _ => return Ok(true),
            };
            let mut vals = Vec::with_capacity(sel.cols.len());
            for c in &sel.cols {
                vals.push(self.eval(ctx, &c.ex, env)?);
            }
            out.push((rid, vals));
            Ok(true)
        })?;
        Ok(out)
    }
}

fn scalar_fn(n: &str, mut v: Vec<SqlVal>) -> Result<SqlVal, String> {
    let first = v.first().cloned().unwrap_or(SqlVal::Null);
    Ok(match n {
        "coalesce" | "ifnull" => v.into_iter().find(|x| *x != SqlVal::Null).unwrap_or(SqlVal::Null),
        "nullif" => {
            let b = v.get(1).cloned().unwrap_or(SqlVal::Null);
            if first != SqlVal::Null && b != SqlVal::Null && cmp_vals(&first, &b) == Ordering::Equal {
                SqlVal::Null
            } else {
                first
            }
        }
        "length" => match &first {
            SqlVal::Null => SqlVal::Null,
            SqlVal::Text(s) => SqlVal::Int(s.chars().count() as i64),
            o => SqlVal::Int(val_to_text(o).len() as i64),
        },
        "abs" => match first {
            SqlVal::Text(s) => {
                let r = text_to_real(&s);
                SqlVal::Real(if r < 0.0 { -r } else { r })
            }
            SqlVal::Int(i) => match i.checked_abs() {
                Some(a) => SqlVal::Int(a),
                None => return Err(String::from("integer overflow")),
            },
            SqlVal::Real(r) => SqlVal::Real(if r < 0.0 { -r } else { r }),
            _ => SqlVal::Null,
        },
        "lower" => match first {
            SqlVal::Null => SqlVal::Null,
            o => SqlVal::Text(val_to_text(&o).to_ascii_lowercase()),
        },
        "upper" => match first {
            SqlVal::Null => SqlVal::Null,
            o => SqlVal::Text(val_to_text(&o).to_ascii_uppercase()),
        },
        "typeof" => SqlVal::Text(String::from(match first {
            SqlVal::Null => "null",
            SqlVal::Int(_) => "integer",
            SqlVal::Real(_) => "real",
            SqlVal::Text(_) => "text",
        })),
        "min" | "max" => {
            if v.iter().any(|x| *x == SqlVal::Null) {
                return Ok(SqlVal::Null);
            }
            let want = if n == "min" { Ordering::Less } else { Ordering::Greater };
            let mut best = v.remove(0);
            for x in v {
                if cmp_vals(&x, &best) == want {
                    best = x;
                }
            }
            best
        }
        _ => return Err(format!("no such function: {}", n)),
    })
}

// ════════════════════════════════════════════════════════════════════════
// Db: mutation, transactions, durability
// ════════════════════════════════════════════════════════════════════════

enum Undo {
    Ins { t: String, rid: i64 },
    Del { t: String, rid: i64, vals: Vec<SqlVal> },
    Seq { t: String, old: i64 },
    Create { t: String },
    Index { t: String },
    Drop { t: Table },
}

pub struct Db {
    tables: BTreeMap<String, Table>,
    cache: BTreeMap<String, Arc<Stmt>>,
    in_tx: bool,
    undo: Vec<Undo>,
    log: Vec<u8>,
    tx_log: Vec<u8>,
    tx_log_n: u32,
    replaying: bool,
}

struct Outcome {
    changes: usize,
    mutated: bool,
}

const PLAN_CACHE_MAX: usize = 1024;
const SNAP_MAGIC: &[u8; 8] = b"SQLMINI1";

impl Default for Db {
    fn default() -> Self {
        Db::new()
    }
}

impl Db {
    pub fn new() -> Db {
        Db {
            tables: BTreeMap::new(),
            cache: BTreeMap::new(),
            in_tx: false,
            undo: Vec::new(),
            log: Vec::new(),
            tx_log: Vec::new(),
            tx_log_n: 0,
            replaying: false,
        }
    }

    /// True between BEGIN and COMMIT/ROLLBACK.
    pub fn in_transaction(&self) -> bool {
        self.in_tx
    }

    /// Number of rows in a table (None if it does not exist).
    pub fn table_len(&self, name: &str) -> Option<usize> {
        self.tables.get(&name.to_ascii_lowercase()).map(|t| t.rows.len())
    }

    fn prepare(&mut self, sql: &str) -> Result<Arc<Stmt>, String> {
        if let Some(s) = self.cache.get(sql) {
            return Ok(s.clone());
        }
        let mut st = parse(sql)?;
        self.compile(&mut st)?;
        let st = Arc::new(st);
        if self.cache.len() >= PLAN_CACHE_MAX {
            self.cache.clear();
        }
        self.cache.insert(sql.to_string(), st.clone());
        Ok(st)
    }

    /// Runs one statement; returns rows affected (sqlite3_changes).
    pub fn exec(&mut self, sql: &str, params: &[SqlVal]) -> Result<usize, String> {
        let st = self.prepare(sql)?;
        let o = self.exec_stmt(&st, params)?;
        if o.mutated && !self.replaying {
            self.record(sql, params);
        }
        Ok(o.changes)
    }

    /// Runs a SELECT; returns (column names, rows).
    pub fn query(&mut self, sql: &str, params: &[SqlVal]) -> Result<(Vec<String>, Vec<Vec<SqlVal>>), String> {
        let st = self.prepare(sql)?;
        match &*st {
            Stmt::Select(sel) => {
                let ctx = Ctx::new(params);
                let rows = self.exec_select(&ctx, sel, None, usize::MAX)?;
                Ok((sel.names.clone(), rows))
            }
            _ => {
                self.exec(sql, params)?;
                Ok((Vec::new(), Vec::new()))
            }
        }
    }

    fn exec_stmt(&mut self, st: &Stmt, params: &[SqlVal]) -> Result<Outcome, String> {
        let none = Outcome { changes: 0, mutated: false };
        match st {
            Stmt::Begin => {
                if self.in_tx {
                    return Err(String::from("cannot start a transaction within a transaction"));
                }
                self.in_tx = true;
                self.undo.clear();
                self.tx_log.clear();
                self.tx_log_n = 0;
                Ok(none)
            }
            Stmt::Commit => {
                if !self.in_tx {
                    return Err(String::from("cannot commit - no transaction is active"));
                }
                self.in_tx = false;
                self.undo.clear();
                if self.tx_log_n > 0 {
                    let body = core::mem::take(&mut self.tx_log);
                    let n = self.tx_log_n;
                    self.push_unit(n, &body);
                }
                self.tx_log_n = 0;
                Ok(none)
            }
            Stmt::Rollback => {
                if !self.in_tx {
                    return Err(String::from("cannot rollback - no transaction is active"));
                }
                self.rollback_to(0);
                self.in_tx = false;
                self.tx_log.clear();
                self.tx_log_n = 0;
                Ok(none)
            }
            Stmt::Select(sel) => {
                let ctx = Ctx::new(params);
                self.exec_select(&ctx, sel, None, usize::MAX)?;
                Ok(none)
            }
            _ => {
                let mark = self.undo.len();
                let r = self.mutate(st, params);
                if r.is_err() {
                    self.rollback_to(mark);
                }
                if !self.in_tx {
                    self.undo.clear();
                }
                r
            }
        }
    }

    fn rollback_to(&mut self, mark: usize) {
        while self.undo.len() > mark {
            let u = match self.undo.pop() {
                Some(u) => u,
                None => break,
            };
            match u {
                Undo::Ins { t, rid } => {
                    if let Some(tb) = self.tables.get_mut(&t) {
                        tb.raw_delete(rid);
                    }
                }
                Undo::Del { t, rid, vals } => {
                    if let Some(tb) = self.tables.get_mut(&t) {
                        tb.raw_insert(rid, vals);
                    }
                }
                Undo::Seq { t, old } => {
                    if let Some(tb) = self.tables.get_mut(&t) {
                        tb.seq = old;
                    }
                }
                Undo::Create { t } => {
                    self.tables.remove(&t);
                    self.cache.clear();
                }
                Undo::Index { t } => {
                    if let Some(tb) = self.tables.get_mut(&t) {
                        tb.indexes.pop();
                    }
                    self.cache.clear();
                }
                Undo::Drop { t } => {
                    self.tables.insert(t.name.to_ascii_lowercase(), t);
                    self.cache.clear();
                }
            }
        }
    }

    fn mutate(&mut self, st: &Stmt, params: &[SqlVal]) -> Result<Outcome, String> {
        match st {
            Stmt::CreateTable(ct) => self.create_table(ct),
            Stmt::CreateIndex { name, table, cols, unique, ine, sql } => self.create_index(name, table, cols, *unique, *ine, Some(sql.clone())),
            Stmt::DropTable { name, if_exists } => {
                let key = name.to_ascii_lowercase();
                match self.tables.remove(&key) {
                    Some(t) => {
                        self.undo.push(Undo::Drop { t });
                        self.cache.clear();
                        Ok(Outcome { changes: 0, mutated: true })
                    }
                    None if *if_exists => Ok(Outcome { changes: 0, mutated: false }),
                    None => Err(format!("no such table: {}", name)),
                }
            }
            Stmt::Insert(ins) => self.do_insert(ins, params),
            Stmt::Delete { table, sel } => {
                let key = table.to_ascii_lowercase();
                let rows = {
                    let ctx = Ctx::new(params);
                    self.collect(&ctx, sel)?
                };
                let t = self.tables.get_mut(&key).ok_or_else(|| format!("no such table: {}", table))?;
                let mut n = 0;
                for (rid, _) in rows {
                    if let Some(vals) = t.raw_delete(rid) {
                        self.undo.push(Undo::Del { t: key.clone(), rid, vals });
                        n += 1;
                    }
                }
                Ok(Outcome { changes: n, mutated: n > 0 })
            }
            Stmt::Update { table, set_cols, sel, .. } => {
                let key = table.to_ascii_lowercase();
                let rows = {
                    let ctx = Ctx::new(params);
                    self.collect(&ctx, sel)?
                };
                let alias = self.tables.get(&key).and_then(|t| t.alias);
                let mut n = 0;
                for (rid, newv) in rows {
                    let old = match self.tables.get(&key).and_then(|t| t.rows.get(&rid)) {
                        Some(v) => v.clone(),
                        None => continue,
                    };
                    let mut vals = old;
                    let mut new_rid = rid;
                    for (c, v) in set_cols.iter().zip(newv) {
                        if *c == ROWID || alias.map_or(false, |a| a as u32 == *c) {
                            new_rid = match apply_col_aff(Aff::Integer, v) {
                                SqlVal::Int(i) => i,
                                _ => return Err(String::from("datatype mismatch")),
                            };
                        } else if let Some(slot) = vals.get_mut(*c as usize) {
                            *slot = v;
                        }
                    }
                    self.update_row(&key, rid, new_rid, vals)?;
                    n += 1;
                }
                Ok(Outcome { changes: n, mutated: n > 0 })
            }
            _ => Ok(Outcome { changes: 0, mutated: false }),
        }
    }

    fn create_table(&mut self, ct: &CreateTable) -> Result<Outcome, String> {
        let key = ct.name.to_ascii_lowercase();
        if self.tables.contains_key(&key) {
            if ct.ine {
                return Ok(Outcome { changes: 0, mutated: false });
            }
            return Err(format!("table {} already exists", ct.name));
        }
        let mut cols = Vec::new();
        for c in &ct.cols {
            if cols.iter().any(|x: &Col| eqi(&x.name, &c.name)) {
                return Err(format!("duplicate column name: {}", c.name));
            }
            cols.push(Col { name: c.name.clone(), aff: decl_affinity(&c.decl), notnull: c.notnull, default: c.default.clone() });
        }
        let col_of = |n: &str| -> Result<usize, String> { cols.iter().position(|c| eqi(&c.name, n)).ok_or_else(|| format!("no such column: {}", n)) };
        let mut pk: Vec<usize> = Vec::new();
        let mut pk_desc = false;
        let mut autoinc = false;
        for (i, c) in ct.cols.iter().enumerate() {
            if c.pk {
                pk.push(i);
                pk_desc = c.pk_desc;
                autoinc = c.autoinc;
            }
        }
        for n in &ct.pk {
            pk.push(col_of(n)?);
        }
        let mut alias = None;
        if pk.len() == 1 && !pk_desc {
            let c = &ct.cols[pk[0]];
            if c.decl.trim().eq_ignore_ascii_case("INTEGER") {
                alias = Some(pk[0]);
            }
        }
        if autoinc && alias.is_none() {
            return Err(String::from("AUTOINCREMENT is only allowed on an INTEGER PRIMARY KEY"));
        }
        let mut indexes = Vec::new();
        let mut n_auto = 0;
        let mut add_unique = |cols_ix: Vec<usize>, indexes: &mut Vec<Index>| {
            if indexes.iter().any(|x: &Index| x.unique && x.cols == cols_ix) {
                return;
            }
            n_auto += 1;
            indexes.push(Index { name: format!("sqlite_autoindex_{}_{}", ct.name, n_auto), cols: cols_ix, unique: true, set: BTreeSet::new(), sql: None });
        };
        if !pk.is_empty() && alias.is_none() {
            add_unique(pk.clone(), &mut indexes);
        }
        for (i, c) in ct.cols.iter().enumerate() {
            if c.unique && alias != Some(i) {
                add_unique(vec![i], &mut indexes);
            }
        }
        for u in &ct.uniques {
            let mut v = Vec::new();
            for n in u {
                v.push(col_of(n)?);
            }
            if v.len() == 1 && alias == Some(v[0]) {
                continue;
            }
            add_unique(v, &mut indexes);
        }
        let t = Table { name: ct.name.clone(), sql: ct.sql.clone(), cols, alias, autoinc, seq: 0, rows: BTreeMap::new(), indexes };
        self.tables.insert(key.clone(), t);
        self.undo.push(Undo::Create { t: key });
        self.cache.clear();
        Ok(Outcome { changes: 0, mutated: true })
    }

    fn create_index(&mut self, name: &str, table: &str, cols: &[String], unique: bool, ine: bool, sql: Option<String>) -> Result<Outcome, String> {
        if self.tables.values().any(|t| t.indexes.iter().any(|i| eqi(&i.name, name))) {
            if ine {
                return Ok(Outcome { changes: 0, mutated: false });
            }
            return Err(format!("index {} already exists", name));
        }
        let key = table.to_ascii_lowercase();
        let t = self.tables.get_mut(&key).ok_or_else(|| format!("no such table: main.{}", table))?;
        let mut ix = Vec::new();
        for c in cols {
            ix.push(t.col_index(c).ok_or_else(|| format!("no such column: {}", c))?);
        }
        let mut idx = Index { name: name.to_string(), cols: ix, unique, set: BTreeSet::new(), sql };
        for (rid, vals) in t.rows.iter() {
            idx.set.insert(Table::key_for(&idx, vals, *rid));
        }
        t.indexes.push(idx);
        if unique {
            let x = t.indexes.len() - 1;
            let dup = t.rows.iter().any(|(rid, vals)| t.find_conflict(x, vals, Some(*rid)).is_some());
            if dup {
                let msg = t.unique_msg(Some(x));
                t.indexes.pop();
                return Err(msg);
            }
        }
        self.undo.push(Undo::Index { t: key });
        self.cache.clear();
        Ok(Outcome { changes: 0, mutated: true })
    }

    fn do_insert(&mut self, ins: &Insert, params: &[SqlVal]) -> Result<Outcome, String> {
        let key = ins.table.clone();
        let mut changes = 0usize;
        let ctx = Ctx::new(params);
        for row in &ins.rows {
            // build the candidate row
            let (vals, explicit_rid) = {
                let t = self.tables.get(&key).ok_or_else(|| format!("no such table: {}", ins.table))?;
                let mut vals: Vec<SqlVal> = t.cols.iter().map(|c| c.default.clone().unwrap_or(SqlVal::Null)).collect();
                let mut explicit_rid = SqlVal::Null;
                let env = Env { slots: &[], aggs: &[], parent: None };
                for (pos, e) in row.iter().enumerate() {
                    let v = self.eval(&ctx, e, &env)?;
                    match ins.cols.get(pos) {
                        Some(&c) if c == ROWID => explicit_rid = v,
                        Some(&c) => {
                            if let Some(s) = vals.get_mut(c as usize) {
                                *s = v;
                            }
                        }
                        None => return Err(String::from("too many values")),
                    }
                }
                if let Some(a) = t.alias {
                    let v = core::mem::replace(&mut vals[a], SqlVal::Null);
                    if v != SqlVal::Null {
                        explicit_rid = v;
                    }
                }
                (vals, explicit_rid)
            };
            changes += self.insert_one(&ctx, ins, &key, vals, explicit_rid)?;
        }
        Ok(Outcome { changes, mutated: changes > 0 })
    }

    fn insert_one(&mut self, ctx: &Ctx, ins: &Insert, key: &str, mut vals: Vec<SqlVal>, explicit_rid: SqlVal) -> Result<usize, String> {
        let mode = ins.mode;
        let (rid, victims) = {
            let t = self.tables.get(key).ok_or_else(|| String::from("no such table"))?;
            for (i, c) in t.cols.iter().enumerate() {
                if Some(i) == t.alias {
                    continue;
                }
                let v = core::mem::replace(&mut vals[i], SqlVal::Null);
                vals[i] = apply_col_aff(c.aff, v);
            }
            let rid = match explicit_rid {
                SqlVal::Null => t.new_rowid()?,
                v => match apply_col_aff(Aff::Integer, v) {
                    SqlVal::Int(i) => i,
                    _ => return Err(String::from("datatype mismatch")),
                },
            };
            if let Some(a) = t.alias {
                vals[a] = SqlVal::Int(rid);
            }
            for (i, c) in t.cols.iter().enumerate() {
                if Some(i) == t.alias || !c.notnull || vals[i] != SqlVal::Null {
                    continue;
                }
                match mode {
                    Conf::Ignore => return Ok(0),
                    Conf::Replace if c.default.as_ref().map_or(false, |d| *d != SqlVal::Null) => {
                        vals[i] = apply_col_aff(c.aff, c.default.clone().unwrap_or(SqlVal::Null));
                    }
                    _ => return Err(format!("NOT NULL constraint failed: {}.{}", t.name, c.name)),
                }
            }
            // upsert target first
            if let Some(u) = &ins.upsert {
                let conflict = match &u.target {
                    Some(Target::Rowid) => {
                        if t.rows.contains_key(&rid) {
                            Some(rid)
                        } else {
                            None
                        }
                    }
                    Some(Target::Index(x)) => t.find_conflict(*x, &vals, None),
                    None => {
                        let mut c = if t.rows.contains_key(&rid) { Some(rid) } else { None };
                        if c.is_none() {
                            for x in 0..t.indexes.len() {
                                if t.indexes[x].unique {
                                    if let Some(r) = t.find_conflict(x, &vals, None) {
                                        c = Some(r);
                                        break;
                                    }
                                }
                            }
                        }
                        c
                    }
                };
                if let Some(old) = conflict {
                    if u.sets.is_none() {
                        return Ok(0);
                    }
                    let oldvals = match t.rows.get(&old) {
                        Some(v) => v.clone(),
                        None => return Err(String::from("internal: missing conflict row")),
                    };
                    let slots = [Some((old, &oldvals[..])), Some((rid, &vals[..]))];
                    let env = Env { slots: &slots, aggs: &[], parent: None };
                    if let Some(w) = &u.where_ {
                        if truthy(&self.eval(ctx, w, &env)?) != Some(true) {
                            return Ok(0);
                        }
                    }
                    let mut newv = oldvals.clone();
                    let mut new_rid = old;
                    for (c, e) in &u.sets_c {
                        let v = self.eval(ctx, e, &env)?;
                        if Some(*c as usize) == t.alias {
                            new_rid = match apply_col_aff(Aff::Integer, v) {
                                SqlVal::Int(i) => i,
                                _ => return Err(String::from("datatype mismatch")),
                            };
                        } else if let Some(s) = newv.get_mut(*c as usize) {
                            *s = v;
                        }
                    }
                    let k = key.to_string();
                    self.update_row(&k, old, new_rid, newv)?;
                    return Ok(1);
                }
            }
            let mut victims: Vec<i64> = Vec::new();
            let mut on_conflict = |r: i64, x: Option<usize>| -> Result<bool, String> {
                match mode {
                    Conf::Ignore => Ok(false),
                    Conf::Replace => {
                        if !victims.contains(&r) {
                            victims.push(r);
                        }
                        Ok(true)
                    }
                    Conf::Abort => Err(t.unique_msg(x)),
                }
            };
            if t.rows.contains_key(&rid) && !on_conflict(rid, None)? {
                return Ok(0);
            }
            for x in 0..t.indexes.len() {
                if !t.indexes[x].unique {
                    continue;
                }
                if let Some(r) = t.find_conflict(x, &vals, None) {
                    if !on_conflict(r, Some(x))? {
                        return Ok(0);
                    }
                }
            }
            (rid, victims)
        };
        let k = key.to_string();
        let t = self.tables.get_mut(key).ok_or_else(|| String::from("no such table"))?;
        for v in victims {
            if let Some(old) = t.raw_delete(v) {
                self.undo.push(Undo::Del { t: k.clone(), rid: v, vals: old });
            }
        }
        if t.autoinc && rid > t.seq {
            self.undo.push(Undo::Seq { t: k.clone(), old: t.seq });
            t.seq = rid;
        }
        t.raw_insert(rid, vals);
        self.undo.push(Undo::Ins { t: k, rid });
        Ok(1)
    }

    /// Replace row `rid` with `vals` (possibly moving it to `new_rid`),
    /// enforcing affinity, NOT NULL and uniqueness (ABORT on violation).
    fn update_row(&mut self, key: &str, rid: i64, new_rid: i64, mut vals: Vec<SqlVal>) -> Result<(), String> {
        {
            let t = self.tables.get(key).ok_or_else(|| String::from("no such table"))?;
            for (i, c) in t.cols.iter().enumerate() {
                if Some(i) == t.alias {
                    continue;
                }
                let v = core::mem::replace(&mut vals[i], SqlVal::Null);
                vals[i] = apply_col_aff(c.aff, v);
            }
            if let Some(a) = t.alias {
                vals[a] = SqlVal::Int(new_rid);
            }
            for (i, c) in t.cols.iter().enumerate() {
                if Some(i) != t.alias && c.notnull && vals[i] == SqlVal::Null {
                    return Err(format!("NOT NULL constraint failed: {}.{}", t.name, c.name));
                }
            }
            if new_rid != rid && t.rows.contains_key(&new_rid) {
                return Err(t.unique_msg(None));
            }
            for x in 0..t.indexes.len() {
                if t.indexes[x].unique && t.find_conflict(x, &vals, Some(rid)).is_some() {
                    return Err(t.unique_msg(Some(x)));
                }
            }
        }
        let k = key.to_string();
        let t = self.tables.get_mut(key).ok_or_else(|| String::from("no such table"))?;
        if let Some(old) = t.raw_delete(rid) {
            self.undo.push(Undo::Del { t: k.clone(), rid, vals: old });
        }
        if t.autoinc && new_rid > t.seq {
            self.undo.push(Undo::Seq { t: k.clone(), old: t.seq });
            t.seq = new_rid;
        }
        t.raw_insert(new_rid, vals);
        self.undo.push(Undo::Ins { t: k, rid: new_rid });
        Ok(())
    }

    // ── durable log ──────────────────────────────────────────────────────

    fn record(&mut self, sql: &str, params: &[SqlVal]) {
        let mut rec = Vec::new();
        put_str(&mut rec, sql);
        put_u32(&mut rec, params.len() as u32);
        for p in params {
            put_val(&mut rec, p);
        }
        if self.in_tx {
            self.tx_log.extend_from_slice(&rec);
            self.tx_log_n += 1;
        } else {
            self.push_unit(1, &rec);
        }
    }

    fn push_unit(&mut self, n: u32, body: &[u8]) {
        self.log.push(b'U');
        put_u32(&mut self.log, n);
        put_u32(&mut self.log, body.len() as u32);
        self.log.extend_from_slice(body);
        put_u32(&mut self.log, fnv32(body));
    }

    /// Every committed change since the last call (see module docs for the
    /// format).  Statements of an open transaction are not included until
    /// COMMIT; rolled-back ones never are.
    pub fn take_log(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.log)
    }

    /// Re-applies a log produced by `take_log` (normally on a Db restored from
    /// the snapshot taken before that log started).  Each unit (one committed
    /// transaction or one autocommit statement) is applied atomically.  A
    /// truncated or corrupt final unit (torn write) is ignored; replay does not
    /// add to this Db's own log.
    pub fn replay(&mut self, log: &[u8]) -> Result<(), String> {
        if self.in_tx {
            return Err(String::from("cannot replay inside a transaction"));
        }
        let mut r = Reader { b: log, p: 0 };
        while r.p < log.len() {
            let unit = (|| -> Result<(u32, &[u8]), String> {
                if r.u8()? != b'U' {
                    return Err(String::new());
                }
                let n = r.u32()?;
                let len = r.u32()? as usize;
                let body = r.bytes(len)?;
                let sum = r.u32()?;
                if fnv32(body) != sum {
                    return Err(String::new());
                }
                Ok((n, body))
            })();
            let (n, body) = match unit {
                Ok(u) => u,
                Err(_) => return Ok(()), // torn tail
            };
            self.replaying = true;
            let res = self.replay_unit(n, body);
            self.replaying = false;
            res?;
        }
        Ok(())
    }

    fn replay_unit(&mut self, n: u32, body: &[u8]) -> Result<(), String> {
        let mut br = Reader { b: body, p: 0 };
        let mut stmts = Vec::new();
        for _ in 0..n {
            let sql = br.string()?;
            let np = br.u32()? as usize;
            let mut ps = Vec::with_capacity(np.min(4096));
            for _ in 0..np {
                ps.push(br.val()?);
            }
            stmts.push((sql, ps));
        }
        self.exec("BEGIN", &[])?;
        for (sql, ps) in &stmts {
            if let Err(e) = self.exec(sql, ps) {
                let _ = self.exec("ROLLBACK", &[]);
                return Err(format!("replay failed on `{}`: {}", sql, e));
            }
        }
        self.exec("COMMIT", &[])?;
        Ok(())
    }

    // ── snapshot ─────────────────────────────────────────────────────────

    /// Whole-database image.  Taken while a transaction is open it includes
    /// the uncommitted changes, so snapshot outside transactions.
    pub fn snapshot(&self) -> Vec<u8> {
        let mut o = Vec::new();
        o.extend_from_slice(SNAP_MAGIC);
        put_u32(&mut o, self.tables.len() as u32);
        for t in self.tables.values() {
            put_str(&mut o, &t.sql);
            let extra: Vec<&Index> = t.indexes.iter().filter(|i| i.sql.is_some()).collect();
            put_u32(&mut o, extra.len() as u32);
            for i in extra {
                put_str(&mut o, i.sql.as_deref().unwrap_or(""));
            }
            put_i64(&mut o, t.seq);
            put_u32(&mut o, t.cols.len() as u32);
            put_u64(&mut o, t.rows.len() as u64);
            for (rid, vals) in &t.rows {
                put_i64(&mut o, *rid);
                for v in vals {
                    put_val(&mut o, v);
                }
            }
        }
        let sum = fnv64(&o);
        put_u64(&mut o, sum);
        o
    }

    pub fn restore(bytes: &[u8]) -> Result<Db, String> {
        if bytes.len() < SNAP_MAGIC.len() + 12 || &bytes[..8] != SNAP_MAGIC {
            return Err(String::from("snapshot: bad magic"));
        }
        let body = &bytes[..bytes.len() - 8];
        let mut tr = Reader { b: &bytes[bytes.len() - 8..], p: 0 };
        if tr.u64()? != fnv64(body) {
            return Err(String::from("snapshot: checksum mismatch"));
        }
        let mut r = Reader { b: body, p: 8 };
        let mut db = Db::new();
        db.replaying = true;
        let nt = r.u32()?;
        for _ in 0..nt {
            let sql = r.string()?;
            db.exec(&sql, &[]).map_err(|e| format!("snapshot: {}", e))?;
            let name = match parse(&sql)? {
                Stmt::CreateTable(ct) => ct.name.to_ascii_lowercase(),
                _ => return Err(String::from("snapshot: bad table sql")),
            };
            let nx = r.u32()?;
            for _ in 0..nx {
                let isql = r.string()?;
                db.exec(&isql, &[]).map_err(|e| format!("snapshot: {}", e))?;
            }
            let seq = r.i64()?;
            let ncols = r.u32()? as usize;
            let nrows = r.u64()?;
            let t = db.tables.get_mut(&name).ok_or_else(|| String::from("snapshot: table missing"))?;
            if t.cols.len() != ncols {
                return Err(String::from("snapshot: column count mismatch"));
            }
            t.seq = seq;
            for _ in 0..nrows {
                let rid = r.i64()?;
                let mut vals = Vec::with_capacity(ncols);
                for _ in 0..ncols {
                    vals.push(r.val()?);
                }
                t.raw_insert(rid, vals);
            }
        }
        if r.p != body.len() {
            return Err(String::from("snapshot: trailing bytes"));
        }
        db.replaying = false;
        db.undo.clear();
        db.log.clear();
        Ok(db)
    }
}

// ── byte encoding ─────────────────────────────────────────────────────────

fn put_u32(o: &mut Vec<u8>, v: u32) {
    o.extend_from_slice(&v.to_le_bytes());
}
fn put_u64(o: &mut Vec<u8>, v: u64) {
    o.extend_from_slice(&v.to_le_bytes());
}
fn put_i64(o: &mut Vec<u8>, v: i64) {
    o.extend_from_slice(&v.to_le_bytes());
}
fn put_str(o: &mut Vec<u8>, s: &str) {
    put_u32(o, s.len() as u32);
    o.extend_from_slice(s.as_bytes());
}
fn put_val(o: &mut Vec<u8>, v: &SqlVal) {
    match v {
        SqlVal::Null => o.push(0),
        SqlVal::Int(i) => {
            o.push(1);
            put_i64(o, *i);
        }
        SqlVal::Real(r) => {
            o.push(2);
            put_u64(o, r.to_bits());
        }
        SqlVal::Text(s) => {
            o.push(3);
            put_str(o, s);
        }
    }
}

fn fnv32(b: &[u8]) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for &x in b {
        h ^= x as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

fn fnv64(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &x in b {
        h ^= x as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

struct Reader<'b> {
    b: &'b [u8],
    p: usize,
}

impl<'b> Reader<'b> {
    fn bytes(&mut self, n: usize) -> Result<&'b [u8], String> {
        let end = self.p.checked_add(n).ok_or_else(|| String::from("truncated"))?;
        let s = self.b.get(self.p..end).ok_or_else(|| String::from("truncated"))?;
        self.p = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.bytes(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        let s = self.bytes(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u64(&mut self) -> Result<u64, String> {
        let s = self.bytes(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Ok(u64::from_le_bytes(a))
    }
    fn i64(&mut self) -> Result<i64, String> {
        Ok(self.u64()? as i64)
    }
    fn string(&mut self) -> Result<String, String> {
        let n = self.u32()? as usize;
        let s = self.bytes(n)?;
        String::from_utf8(s.to_vec()).map_err(|_| String::from("bad utf-8"))
    }
    fn val(&mut self) -> Result<SqlVal, String> {
        Ok(match self.u8()? {
            0 => SqlVal::Null,
            1 => SqlVal::Int(self.i64()?),
            2 => SqlVal::Real(f64::from_bits(self.u64()?)),
            3 => SqlVal::Text(self.string()?),
            _ => return Err(String::from("bad value tag")),
        })
    }
}
