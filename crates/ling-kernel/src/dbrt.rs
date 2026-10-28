//! `db_open` / `db_exec` / `db_query` for compiled Ling: the host's SQLite
//! builtins on the in-kernel engine (sqlmini.rs). Each isolate has its own
//! connection; connections to the same file share one database (a node and
//! its wallet both read the chain), and while one of them has a transaction
//! open the others wait for it -- SQLite's locking, on green threads.
//!
//! The database lives in memory. Durability: every committed change goes to
//! the engine's log, which is written out as a numbered journal file
//! (`<db>.j<n>`) at most every few seconds; every so often the whole database
//! is written as a snapshot (`<db>`, through `<db>.tmp`) and the journals are
//! removed. Opening loads the snapshot and replays the journals in order. A
//! power cut loses at most the last few seconds -- a node re-syncs those.
//!
//! Values in: numbers as REAL, bools as 0/1, strings (and anything else, as
//! its display) as TEXT -- the host's binding. Out: rows are records (`Row`)
//! whose fields are the result columns; NULL reads as "".

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::lingrt::{is_num, is_str, list_from, mk_str, num, str_of, to_f64, FALSE, TRUE, UNIT};
use crate::sqlmini::{Db, SqlVal};

/// Write the log out at most this often (ms).
const FLUSH_MS: u64 = 5_000;
/// Journals before a snapshot replaces them.
const JOURNALS_PER_SNAPSHOT: u32 = 48;

struct Slot {
    db: Db,
    path: String,
    seq: u32,
    dirty: bool,
    last_flush: u64,
    /// The isolate whose transaction is open.
    txn: Option<u64>,
}

/// Open databases by path.
static mut DBS: BTreeMap<String, Slot> = BTreeMap::new();
/// Each isolate's connection: the path it opened.
static mut CONNS: BTreeMap<u64, String> = BTreeMap::new();
static mut INIT: bool = false;

fn dbs() -> &'static mut BTreeMap<String, Slot> {
    unsafe { &mut *(&raw mut DBS) }
}

fn conns() -> &'static mut BTreeMap<u64, String> {
    unsafe { &mut *(&raw mut CONNS) }
}

/// Drop isolate `iso`'s connection; the database closes (written out) when
/// no connection is left. An open transaction of its is rolled back.
fn disconnect(iso: u64) {
    let Some(path) = conns().remove(&iso) else { return };
    if let Some(s) = dbs().get_mut(&path) {
        if s.txn == Some(iso) {
            let _ = s.db.exec("ROLLBACK", &[]);
            s.txn = None;
        }
    }
    if !conns().values().any(|p| *p == path) {
        if let Some(mut s) = dbs().remove(&path) {
            flush(&mut s, true);
        }
    }
}

/// The database of `iso`'s connection, once no other isolate's transaction
/// is open on it (waiting meanwhile).
fn slot_for(iso: u64) -> Option<&'static mut Slot> {
    loop {
        let path = conns().get(&iso)?.clone();
        let s = dbs().get_mut(&path)?;
        match s.txn {
            Some(owner) if owner != iso && crate::coro::alive(owner) => crate::coro::yield_now(),
            Some(owner) if owner != iso => {
                // its isolate ended mid-transaction
                let _ = s.db.exec("ROLLBACK", &[]);
                s.txn = None;
            },
            _ => return dbs().get_mut(&path),
        }
    }
}

fn init() {
    unsafe {
        if INIT {
            return;
        }
        INIT = true;
    }
    crate::coro::on_kill(cleanup);
}

/// Isolates that ended: close their connections.
fn cleanup() {
    let gone: Vec<u64> = conns().keys().copied().filter(|&id| id != 0 && !crate::coro::alive(id)).collect();
    for id in gone {
        disconnect(id);
    }
}

fn journal_path(base: &str, n: u32) -> String {
    alloc::format!("{base}.j{n}")
}

fn open(path: &str) -> Slot {
    let mut db = match crate::vfs::read(path) {
        Some(bytes) if !bytes.is_empty() => match Db::restore(&bytes) {
            Ok(d) => d,
            Err(e) => {
                crate::console_write(alloc::format!("db: {path}: unreadable snapshot ({e}), starting empty\n").as_bytes());
                Db::new()
            },
        },
        _ => Db::new(),
    };
    let mut seq = 0u32;
    loop {
        let j = journal_path(path, seq);
        let Some(bytes) = crate::vfs::read(&j) else { break };
        if let Err(e) = db.replay(&bytes) {
            crate::console_write(alloc::format!("db: {j}: {e} (later changes dropped)\n").as_bytes());
            break;
        }
        seq += 1;
    }
    // the replayed changes are in the database now, not news for the log
    let _ = db.take_log();
    Slot { db, path: String::from(path), seq, dirty: false, last_flush: crate::arch::timer::now_ms(), txn: None }
}

/// Write what changed: a journal, or (every so often, or `force`) a snapshot.
fn flush(s: &mut Slot, force: bool) {
    if !s.dirty {
        return;
    }
    let now = crate::arch::timer::now_ms();
    if !force && now.saturating_sub(s.last_flush) < FLUSH_MS {
        return;
    }
    s.last_flush = now;
    s.dirty = false;
    // a snapshot mid-transaction would hold uncommitted rows: journal only then
    if (force || s.seq >= JOURNALS_PER_SNAPSHOT) && !s.db.in_transaction() {
        let _ = s.db.take_log();
        let snap = s.db.snapshot();
        let tmp = alloc::format!("{}.tmp", s.path);
        if crate::vfs::write(&tmp, &snap) {
            crate::vfs::delete(&s.path);
            if crate::vfs::rename(&tmp, &s.path) {
                for n in 0..s.seq {
                    crate::vfs::delete(&journal_path(&s.path, n));
                }
                s.seq = 0;
                return;
            }
        }
        crate::console_write(b"db: could not write the snapshot; keeping the journals\n");
        return;
    }
    let log = s.db.take_log();
    if log.is_empty() {
        return;
    }
    if crate::vfs::write(&journal_path(&s.path, s.seq), &log) {
        s.seq += 1;
    } else {
        crate::console_write(b"db: could not write a journal\n");
    }
}

fn to_sql(v: u64) -> SqlVal {
    if is_num(v) {
        SqlVal::Real(to_f64(v))
    } else if v == TRUE {
        SqlVal::Int(1)
    } else if v == FALSE {
        SqlVal::Int(0)
    } else if is_str(v) {
        SqlVal::Text(String::from(str_of(v)))
    } else {
        SqlVal::Text(crate::lingrt::to_string(v))
    }
}

fn from_sql(v: &SqlVal) -> u64 {
    match v {
        SqlVal::Null => mk_str(""),
        SqlVal::Int(n) => num(*n as f64),
        SqlVal::Real(x) => num(*x),
        SqlVal::Text(s) => mk_str(s),
    }
}

fn report(what: &str, sql: &str, e: &str) {
    crate::console_write(alloc::format!("{what}: {e}\n  sql: {sql}\n").as_bytes());
}

pub fn dispatch(name: &[u8], args: &[u64]) -> Option<u64> {
    if !name.starts_with(b"db_") {
        return None;
    }
    init();
    let iso = crate::coro::current_isolate();
    let arg = |i: usize| args.get(i).copied().unwrap_or(UNIT);
    Some(match name {
        b"db_open" => {
            let path = crate::vfs::resolve(match str_of(arg(0)) {
                "" => "app.db",
                p => p,
            });
            disconnect(iso);
            if !dbs().contains_key(&path) {
                let slot = open(&path);
                dbs().insert(path.clone(), slot);
            }
            conns().insert(iso, path);
            UNIT
        },
        b"db_close" => {
            disconnect(iso);
            UNIT
        },
        b"db_exec" => {
            let sql = String::from(str_of(arg(0)));
            let params: Vec<SqlVal> = args.iter().skip(1).map(|&v| to_sql(v)).collect();
            let Some(s) = slot_for(iso) else {
                report("db_exec", &sql, "call db_open first");
                return Some(num(0.0));
            };
            let r = s.db.exec(&sql, &params);
            s.txn = if s.db.in_transaction() { Some(iso) } else { None };
            match r {
                Ok(n) => {
                    s.dirty = true;
                    flush(s, false);
                    num(n as f64)
                },
                Err(e) => {
                    report("db_exec", &sql, &e);
                    num(0.0)
                },
            }
        },
        b"db_query" => {
            let sql = String::from(str_of(arg(0)));
            let params: Vec<SqlVal> = args.iter().skip(1).map(|&v| to_sql(v)).collect();
            let Some(s) = slot_for(iso) else {
                report("db_query", &sql, "call db_open first");
                return Some(list_from(Vec::new()));
            };
            match s.db.query(&sql, &params) {
                Ok((cols, rows)) => {
                    let names: Vec<&str> = cols.iter().map(|c| c.as_str()).collect();
                    let names = crate::lingrec::names_list(&names);
                    let out: Vec<u64> = rows
                        .iter()
                        .map(|r| {
                            let vals: Vec<u64> = r.iter().map(from_sql).collect();
                            crate::lingrec::mk_record("Row", names, &vals)
                        })
                        .collect();
                    list_from(out)
                },
                Err(e) => {
                    report("db_query", &sql, &e);
                    list_from(Vec::new())
                },
            }
        },
        _ => return None,
    })
}

/// Write every open database out now (shutdown, reboot).
pub fn flush_all() {
    for s in dbs().values_mut() {
        flush(s, true);
    }
}
