//! A fold tier in the member's own SQLite (`design/SHARED.md`, "Folds"; `design/DESIGN.md`, Stack, Durable Objects:
//! "folds (SQLite and FTS5)").
//!
//! - Tables. Each table of a tier's `Schema` is a SQLite table `<t>_<name>` (`t` is `c` for Core, `m` for Metadata)
//!   with its key columns as real columns under a composite primary key, `written`, and `v`, the row's columns in
//!   postcard. A table with no key holds one row. Beside them, per tier: the meta row (`<t>__fold`), one `snap_undo`
//!   table per schema table (`<t>_<name>__undo`), and `<t>__digest`, the tier's digest after each recent entry. The
//!   DDL comes from the schema (`ddl`), so a schema change is a new table set, never an `ALTER`.
//! - Writes. `apply` writes through a `TierSql` handle, which buffers them. `seal` turns the buffer into statements
//!   for the input's one transaction (`host::SqlStore`, `Write::Apply`), so a tier's rows land with the `raft_state`
//!   row that says they applied, or not at all. Reads see the buffer and every sealed batch until the transaction
//!   ran (`flushed`).
//! - Storage loss. `Store`'s reads cannot fail, so a read SQLite refuses ("Network connection lost" when the object
//!   has moved) marks the tier `lost` and answers no row, logged with its cause. The host checks `lost` before any
//!   input's writes or outputs, or any reply, leave, and resets the object as it does on a failed write
//!   (`host::Host::fail`). It never panics: a panic rebuilds the wasm instance, which resets every member in the
//!   isolate (F47).
//! - Search. The metadata tier keeps two FTS5 tables over issue and pull request titles and bodies, and comment bodies
//!   (`m_issue_fts`, `m_comment_fts`). `seal` writes them in the same transaction, from the rows it writes, so search
//!   has no lag behind the fold. They are derived: not in `fold_digest`, not in a snapshot.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use graftyard_fold::{DIGESTS, Key, META, Schema, Store, Stored, TierStore, Val, undo_table};
use graftyard_types::{Index, Tier};
use worker::{SqlStorage, SqlStorageValue};

use crate::family::Stmt;

type Rows = BTreeMap<(String, Key), Option<Stored>>;

/// How a sealed row is indexed for search: the statements that bring the FTS5 tables in line with it.
pub type Fts = fn(table: &str, key: &Key, row: Option<&Stored>) -> Vec<Stmt>;

struct Inner {
    sql: SqlStorage,
    tier: Tier,
    /// The tables' name prefix: `""` for the member's fold, `SCRATCH` for a load's tables (`sql_name_in`).
    prefix: &'static str,
    /// Key arity of every table this tier writes, by fold name.
    arity: HashMap<String, usize>,
    open: Rows,
    /// Whether `apply` writes this tier: false while a metadata tier loads (`crate::snap::load_meta`).
    loaded: bool,
    /// Batches sealed into statements whose transaction has not run, oldest first.
    sealed: Vec<Rows>,
    fts: Option<Fts>,
    /// A read SQLite refused, with its cause: everything read since may be missing rows. Reads borrow `Inner`
    /// shared, so this has its own cell.
    lost: RefCell<Option<String>>,
    /// Test installs: the next stored read runs against a table that does not exist, so SQLite refuses it.
    #[cfg(feature = "test-faults")]
    fail_read: std::cell::Cell<bool>,
    /// Test installs (L0-16): every read of this tier, stored or buffered, while it is held unloaded.
    #[cfg(feature = "test-faults")]
    reads: std::cell::Cell<u64>,
    #[cfg(feature = "test-faults")]
    held_unloaded: bool,
}

/// A handle on one tier's tables. Clones share the tables and the write buffer, so the fold, the snapshot writer, and
/// the host see one tier.
#[derive(Clone)]
pub struct TierSql(Rc<RefCell<Inner>>);

impl std::fmt::Debug for TierSql {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TierSql({:?})", self.0.borrow().tier)
    }
}

fn prefix(t: Tier) -> &'static str {
    match t {
        Tier::Core => "c",
        Tier::Metadata => "m",
    }
}

/// The SQL name of a fold table: `<t>_<name>`, with `~` (the undo suffix) as `__`.
pub fn sql_name(t: Tier, table: &str) -> String {
    sql_name_in("", t, table)
}

/// The prefix of the tables a snapshot install or a fold audit loads into (`crate::load`): `i` before the member's
/// own names, so a load never touches the fold the member serves from until the install swaps them.
pub const SCRATCH: &str = "i";

/// A fold table's name under a prefix.
pub fn sql_name_in(pre: &str, t: Tier, table: &str) -> String {
    format!("{pre}{}_{}", prefix(t), table.replace('~', "__"))
}

/// The metadata tier's FTS5 tables under a prefix: issues, then comments.
fn fts_names(pre: &str) -> (String, String) {
    (format!("{pre}m_issue_fts"), format!("{pre}m_comment_fts"))
}

/// Statements are `&'static str`; a tier has a fixed set of tables, so each distinct statement is leaked once.
fn leak(s: String) -> &'static str {
    thread_local!(static SEEN: RefCell<HashMap<String, &'static str>> = RefCell::new(HashMap::new()));
    SEEN.with(|m| *m.borrow_mut().entry(s.clone()).or_insert_with(|| Box::leak(s.into_boxed_str())))
}

fn cols(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("k{i}")).collect()
}

/// The tables of a tier and of its bookkeeping, with their key arity.
fn tables(schema: &Schema) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for t in schema.tables {
        out.push((t.name.to_owned(), t.key.len()));
        out.push((undo_table(t.name), t.key.len()));
    }
    out.push((META.to_owned(), 0));
    out.push((DIGESTS.to_owned(), 1));
    out
}

/// The DDL of a tier: one table per schema table and per undo table, the meta row, and the digest ring; and for the
/// metadata tier its FTS5 tables.
pub fn ddl(schema: &Schema, pre: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (name, n) in tables(schema) {
        out.push(create(&sql_name_in(pre, schema.tier, &name), n));
    }
    if schema.tier == Tier::Metadata {
        let (issue, comment) = fts_names(pre);
        out.push(format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS {issue} USING fts5(title, body, content='', contentless_delete=1)"
        ));
        out.push(format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS {comment} USING fts5(body, content='', contentless_delete=1)"
        ));
    }
    out
}

/// Drops a load's scratch tables of a tier, one statement each.
pub fn drop_scratch(schema: &Schema) -> Vec<String> {
    let mut out: Vec<String> = tables(schema)
        .into_iter()
        .map(|(name, _)| format!("DROP TABLE IF EXISTS \"{}\"", sql_name_in(SCRATCH, schema.tier, &name)))
        .collect();
    if schema.tier == Tier::Metadata {
        let (issue, comment) = fts_names(SCRATCH);
        out.push(format!("DROP TABLE IF EXISTS {issue}"));
        out.push(format!("DROP TABLE IF EXISTS {comment}"));
    }
    out
}

/// The statements that make a load's scratch tables the member's fold: each of the tier's tables, and the metadata
/// tier's FTS5 tables, dropped and replaced by its scratch twin. They run inside the install's transaction
/// (`host::SqlStore`, `Write::Install`).
pub fn swap_in(schema: &Schema) -> Vec<&'static str> {
    let mut pairs: Vec<(String, String)> = tables(schema)
        .into_iter()
        .map(|(name, _)| (sql_name(schema.tier, &name), sql_name_in(SCRATCH, schema.tier, &name)))
        .collect();
    if schema.tier == Tier::Metadata {
        let (live, scratch) = (fts_names(""), fts_names(SCRATCH));
        pairs.push((live.0, scratch.0));
        pairs.push((live.1, scratch.1));
    }
    let mut out = Vec::new();
    for (live, scratch) in pairs {
        out.push(leak(format!("DROP TABLE IF EXISTS \"{live}\"")));
        out.push(leak(format!("ALTER TABLE \"{scratch}\" RENAME TO \"{live}\"")));
    }
    out
}

fn create(t: &str, n: usize) -> String {
    if n == 0 {
        return format!(
            "CREATE TABLE IF NOT EXISTS \"{t}\" (one INTEGER PRIMARY KEY CHECK (one = 0), written INTEGER NOT NULL, v BLOB NOT NULL)"
        );
    }
    let k = cols(n).join(", ");
    format!("CREATE TABLE IF NOT EXISTS \"{t}\" ({k}, written INTEGER NOT NULL, v BLOB NOT NULL, PRIMARY KEY ({k}))")
}

/// `CREATE TABLE IF NOT EXISTS` keeps a table whose key the schema has since changed: M2-WP2 gave `check_run`,
/// `check_suite`, and `status` new keys after the BF-20 members were seeded, and the snapshot alarm's first scan of
/// `m_check_run` failed with `no such column: k1`, panicked, and reset every member in the isolate
/// (`tests/m2/results/m2-wp1.md`, "2026-11-04"). An empty table of the old shape is made again in the new one; a table
/// that holds rows of the old shape refuses the restore, since its rows need a refold, not a rename.
pub fn reshape(sql: &SqlStorage, schema: &Schema) -> worker::Result<Vec<String>> {
    let mut made = Vec::new();
    for (name, n) in tables(schema) {
        let t = sql_name(schema.tier, &name);
        // Each cursor is read to its end before the next statement: an open one holds the table, and `DROP TABLE`
        // under it fails with SQLITE_LOCKED.
        let have: Vec<String> = sql
            .exec(&format!("SELECT name FROM pragma_table_info('{t}') ORDER BY cid"), None)?
            .raw()
            .map(|r| crate::host::text(&r?[0]).map(str::to_owned))
            .collect::<worker::Result<_>>()?;
        if have == columns(n) {
            continue;
        }
        // A table this release adds, or a member's first restore: nothing to keep. Reading it would fail with "no
        // such table" and refuse the restore of every member of the family (M2-WP5's first deploy did that).
        if have.is_empty() {
            sql.exec(&create(&t, n), None)?;
            made.push(t);
            continue;
        }
        let rows: Vec<_> = sql.exec(&format!("SELECT 1 FROM \"{t}\" LIMIT 1"), None)?.raw().collect();
        if !rows.is_empty() {
            return Err(worker::Error::RustError(format!("fold table {t} holds rows keyed {have:?}, not {n} columns")));
        }
        sql.exec(&format!("DROP TABLE \"{t}\""), None)?;
        sql.exec(&create(&t, n), None)?;
        made.push(t);
    }
    Ok(made)
}

/// The columns `create` gives a table with an `n`-column key, in order.
fn columns(n: usize) -> Vec<String> {
    let key = if n == 0 { vec!["one".into()] } else { cols(n) };
    key.into_iter().chain(["written".into(), "v".into()]).collect()
}

pub fn to_sql(v: &Val) -> SqlStorageValue {
    match v {
        Val::Null => SqlStorageValue::Null,
        Val::Int(i) => SqlStorageValue::Integer(*i),
        Val::Text(s) => SqlStorageValue::String(s.to_string()),
        Val::Blob(b) => SqlStorageValue::Blob(b.to_vec()),
    }
}

fn from_sql(v: SqlStorageValue) -> Val {
    match v {
        SqlStorageValue::Null => Val::Null,
        SqlStorageValue::Boolean(b) => Val::Int(i64::from(b)),
        SqlStorageValue::Integer(i) => Val::Int(i),
        SqlStorageValue::Float(f) => Val::Int(f as i64),
        SqlStorageValue::String(s) => Val::Text(s.into()),
        SqlStorageValue::Blob(b) => Val::Blob(b.into()),
    }
}

/// A stored row's index and columns. `Err` names what did not decode, such as a row written under another layout of
/// the columns; the caller treats it as a refused read (`Inner::refused`), never a panic.
fn row_of(written: SqlStorageValue, v: SqlStorageValue) -> Result<Stored, String> {
    let written = match written {
        SqlStorageValue::Integer(i) => Index(i as u64),
        SqlStorageValue::Float(f) => Index(f as u64),
        other => return Err(format!("index is {other:?}")),
    };
    let cols = match v {
        SqlStorageValue::Blob(b) => postcard::from_bytes(&b).map_err(|e| format!("columns: {e}"))?,
        other => return Err(format!("columns are {other:?}")),
    };
    Ok(Stored { written, cols })
}

impl TierSql {
    /// A handle on `schema`'s tables in `sql`, which `ddl` has created.
    pub fn new(sql: SqlStorage, schema: &Schema, fts: Option<Fts>) -> TierSql {
        Self::new_in(sql, schema, fts, "")
    }

    /// A handle on the tables under `pre`.
    pub fn new_in(sql: SqlStorage, schema: &Schema, fts: Option<Fts>, pre: &'static str) -> TierSql {
        let arity = tables(schema).into_iter().collect();
        TierSql(Rc::new(RefCell::new(Inner {
            sql,
            tier: schema.tier,
            prefix: pre,
            arity,
            open: BTreeMap::new(),
            loaded: true,
            sealed: Vec::new(),
            fts,
            lost: RefCell::new(None),
            #[cfg(feature = "test-faults")]
            fail_read: std::cell::Cell::new(false),
            #[cfg(feature = "test-faults")]
            reads: std::cell::Cell::new(0),
            #[cfg(feature = "test-faults")]
            held_unloaded: false,
        })))
    }

    /// The cause of a read SQLite refused, if one was: the member must not use what it read since.
    pub fn lost(&self) -> Option<String> {
        self.0.borrow().lost.borrow().clone()
    }

    /// Test installs: the next read of a stored row fails in SQLite.
    #[cfg(feature = "test-faults")]
    pub fn fail_next_read(&self) {
        self.0.borrow().fail_read.set(true);
    }

    /// Test installs (L0-16): hold the tier unloaded (`loaded()` false, and `set_loaded` ignored so the alarm's load
    /// does not bring it back) and count every read of it from now, or release it. A released tier stays unloaded:
    /// `apply` skipped it while held, so the alarm's metadata load replays what it missed before it serves again.
    /// Answers the reads counted.
    #[cfg(feature = "test-faults")]
    pub fn hold_unloaded(&self, hold: bool) -> u64 {
        let mut i = self.0.borrow_mut();
        i.held_unloaded = hold;
        i.loaded = false;
        if hold {
            i.reads.set(0);
        }
        i.reads.get()
    }

    #[cfg(feature = "test-faults")]
    pub fn reads_while_held(&self) -> Option<u64> {
        let i = self.0.borrow();
        i.held_unloaded.then(|| i.reads.get())
    }

    /// The buffered writes as statements for the input's transaction. They stay readable until `flushed`.
    pub fn seal(&self, stmts: &mut Vec<Stmt>) {
        let mut inner = self.0.borrow_mut();
        let open = std::mem::take(&mut inner.open);
        if open.is_empty() {
            return;
        }
        for ((table, key), row) in &open {
            stmts.push(inner.write_stmt(table, key, row.as_ref()));
            if let Some(f) = inner.fts {
                stmts.extend(f(table, key, row.as_ref()));
            }
        }
        inner.sealed.push(open);
    }

    /// Every sealed batch's transaction has run.
    pub fn flushed(&self) {
        self.0.borrow_mut().sealed.clear();
    }

    pub fn set_loaded(&self, loaded: bool) {
        let mut i = self.0.borrow_mut();
        #[cfg(feature = "test-faults")]
        if i.held_unloaded {
            return;
        }
        i.loaded = loaded;
    }
}

impl Inner {
    /// A refused read: logged with its cause and kept for the host (`TierSql::lost`). The caller answers no rows.
    fn refused(&self, what: &str, table: &str, e: impl std::fmt::Display) {
        let why = format!("{what} of {}: {e}", self.name(table));
        worker::console_error!("fold storage lost: {why}");
        self.lost.borrow_mut().get_or_insert(why);
    }

    /// The query a stored read runs: on test installs, once after `fail_next_read`, one SQLite refuses.
    fn query(&self, q: String) -> String {
        #[cfg(feature = "test-faults")]
        if self.held_unloaded {
            self.reads.set(self.reads.get() + 1);
        }
        #[cfg(feature = "test-faults")]
        if self.fail_read.replace(false) {
            return "SELECT written, v FROM graftyard_no_such_table".into();
        }
        q
    }

    fn arity(&self, table: &str) -> usize {
        *self.arity.get(table).unwrap_or_else(|| panic!("table {table} is not in the {:?} tier", self.tier))
    }

    fn name(&self, table: &str) -> String {
        sql_name_in(self.prefix, self.tier, table)
    }

    fn write_stmt(&self, table: &str, key: &Key, row: Option<&Stored>) -> Stmt {
        let n = self.arity(table);
        let t = self.name(table);
        let mut args: Vec<SqlStorageValue> = key.iter().map(to_sql).collect();
        match row {
            Some(r) => {
                let q = if n == 0 {
                    format!("INSERT OR REPLACE INTO \"{t}\" (one, written, v) VALUES (0, ?, ?)")
                } else {
                    let k = cols(n).join(", ");
                    let marks = vec!["?"; n].join(", ");
                    format!("INSERT OR REPLACE INTO \"{t}\" ({k}, written, v) VALUES ({marks}, ?, ?)")
                };
                args.push(SqlStorageValue::Integer(r.written.0 as i64));
                args.push(SqlStorageValue::Blob(postcard::to_allocvec(&r.cols).expect("a row encodes")));
                (leak(q), args)
            }
            None => (leak(format!("DELETE FROM \"{t}\"{}", where_eq(n))), args),
        }
    }

    /// The newest buffered or sealed value of a row: `Some(None)` for a buffered delete, `None` for no buffered write.
    fn pending(&self, table: &str, key: &[Val]) -> Option<Option<Stored>> {
        let k = (table.to_owned(), key.to_vec());
        if let Some(v) = self.open.get(&k) {
            return Some(v.clone());
        }
        self.sealed.iter().rev().find_map(|b| b.get(&k).cloned())
    }

    fn sql_get(&self, table: &str, key: &[Val]) -> Option<Stored> {
        let n = self.arity(table);
        assert_eq!(key.len(), n, "a {table} key has {n} columns");
        let q = format!("SELECT written, v FROM \"{}\"{}", self.name(table), where_eq(n));
        let cursor = match self.sql.exec(&self.query(q), key.iter().map(to_sql).collect::<Vec<_>>()) {
            Ok(c) => c,
            Err(e) => {
                self.refused("a fold read", table, e);
                return None;
            }
        };
        // Read to the end: a cursor stopped after its row holds the table, and a fold audit's closing `DROP TABLE` of
        // its scratch tables then fails with SQLITE_LOCKED (as the log cursor did, d6e350d).
        let mut r = match cursor.raw().collect::<worker::Result<Vec<_>>>() {
            Ok(mut rows) => rows.pop()?,
            Err(e) => {
                self.refused("a fold row", table, e);
                return None;
            }
        };
        let v = r.pop().expect("v");
        let w = r.pop().expect("written");
        match row_of(w, v) {
            Ok(s) => Some(s),
            Err(e) => {
                self.refused("a fold row", table, e);
                None
            }
        }
    }

    fn sql_scan(&self, table: &str, from: &[Val], limit: usize) -> Vec<(Key, Stored)> {
        let n = self.arity(table);
        let t = self.name(table);
        if n == 0 {
            if !from.is_empty() {
                return Vec::new();
            }
            return self.sql_get(table, &[]).map(|r| (Vec::new(), r)).into_iter().take(limit).collect();
        }
        let k = cols(n);
        let m = from.len().min(n);
        let cond = match m {
            0 => String::new(),
            _ => format!(" WHERE ({}) >= ({})", k[..m].join(", "), vec!["?"; m].join(", ")),
        };
        let q = format!("SELECT {}, written, v FROM \"{t}\"{cond} ORDER BY {} LIMIT ?", k.join(", "), k.join(", "));
        let mut args: Vec<SqlStorageValue> = from[..m].iter().map(to_sql).collect();
        args.push(SqlStorageValue::Integer(limit as i64));
        let mut out = Vec::new();
        let rows = match self.sql.exec(&self.query(q), args) {
            Ok(r) => r,
            Err(e) => {
                self.refused("a fold scan", table, e);
                return out;
            }
        };
        for r in rows.raw() {
            let mut r = match r {
                Ok(r) => r,
                Err(e) => {
                    self.refused("a fold row", table, e);
                    return out;
                }
            };
            let v = r.pop().expect("v");
            let w = r.pop().expect("written");
            let key: Key = r.into_iter().map(from_sql).collect();
            // A `from` longer than the key's prefix compared on: drop rows below it.
            if from.len() > m || key.as_slice() >= from {
                match row_of(w, v) {
                    Ok(s) => out.push((key, s)),
                    Err(e) => {
                        self.refused("a fold row", table, e);
                        return out;
                    }
                }
            }
        }
        out
    }
}

fn where_eq(n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    format!(" WHERE {}", cols(n).iter().map(|c| format!("{c} IS ?")).collect::<Vec<_>>().join(" AND "))
}

impl Store for TierSql {
    fn get(&self, table: &str, key: &[Val]) -> Option<Stored> {
        let inner = self.0.borrow();
        match inner.pending(table, key) {
            Some(v) => v,
            None => inner.sql_get(table, key),
        }
    }

    fn put(&mut self, table: &str, key: Key, row: Stored) {
        let mut inner = self.0.borrow_mut();
        assert_eq!(key.len(), inner.arity(table), "a {table} key's columns");
        assert!(!key.contains(&Val::Null), "a fold key holds no NULL");
        inner.open.insert((table.to_owned(), key), Some(row));
    }

    fn delete(&mut self, table: &str, key: &[Val]) {
        let mut inner = self.0.borrow_mut();
        inner.open.insert((table.to_owned(), key.to_vec()), None);
    }

    /// The table's rows with every buffered write on top: enough stored rows that buffered deletes cannot leave fewer
    /// than `limit`, merged in key order.
    fn scan(&self, table: &str, from: &[Val], limit: usize) -> Vec<(Key, Stored)> {
        let inner = self.0.borrow();
        let lo = (table.to_owned(), from.to_vec());
        // Each batch's rows from `from`, newest batch last so it wins, up to `limit` of its own visible rows: a later
        // row of that batch sorts after `limit` visible ones (`graftyard_fold::Overlay::scan`).
        let mut mine: BTreeMap<Key, Option<Stored>> = BTreeMap::new();
        for b in inner.sealed.iter().chain(std::iter::once(&inner.open)) {
            let mut visible = 0;
            for ((_, k), v) in b.range(lo.clone()..).take_while(|((t, _), _)| t == table) {
                if visible == limit {
                    break;
                }
                visible += usize::from(v.is_some());
                mine.insert(k.clone(), v.clone());
            }
        }
        let deletes = mine.values().filter(|v| v.is_none()).count();
        let mut merged: BTreeMap<Key, Stored> = inner.sql_scan(table, from, limit + deletes).into_iter().collect();
        for (k, v) in mine {
            match v {
                Some(v) => merged.insert(k, v),
                None => merged.remove(&k),
            };
        }
        merged.into_iter().take(limit).collect()
    }

    /// One `DELETE` for the stored rows, run at once: only `end_snapshot` clears, and only undo tables, which no
    /// transaction of the member's writes touches but `apply`'s copies, buffered here and dropped too.
    fn clear(&mut self, table: &str) {
        let mut inner = self.0.borrow_mut();
        inner.open.retain(|(t, _), _| t != table);
        for b in &mut inner.sealed {
            b.retain(|(t, _), _| t != table);
        }
        let q = format!("DELETE FROM \"{}\"", inner.name(table));
        if let Err(e) = inner.sql.exec(&q, None) {
            inner.refused("clearing", table, e);
        }
    }
}

impl TierStore for TierSql {
    fn clone_box(&self) -> Box<dyn TierStore> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn loaded(&self) -> bool {
        self.0.borrow().loaded
    }
}

/// The handle behind a tier of a fold built over `TierSql`.
pub fn handle(t: &dyn TierStore) -> &TierSql {
    t.as_any().downcast_ref::<TierSql>().expect("a site member's tier is a TierSql")
}

/// The SQL tables of a repository chain's two tiers, created if missing, and a handle on each.
pub fn repo_tiers(sql: &SqlStorage) -> (TierSql, TierSql) {
    for schema in [graftyard_git::tiers::CORE, graftyard_git::tiers::METADATA] {
        for q in ddl(&schema, "") {
            sql.exec(&q, None).expect("fold tier schema");
        }
    }
    let core = TierSql::new(sql.clone(), &graftyard_git::tiers::CORE, None);
    let meta = TierSql::new(sql.clone(), &graftyard_git::tiers::METADATA, Some(search));
    (core, meta)
}

/// A load's scratch tiers (`SCRATCH`), created if missing; with `fts`, the metadata tier's search rows go to its
/// scratch FTS5 tables, as an install needs and an audit does not.
pub fn scratch_tiers(sql: &SqlStorage, fts: bool) -> worker::Result<(TierSql, TierSql)> {
    for schema in [graftyard_git::tiers::CORE, graftyard_git::tiers::METADATA] {
        for q in ddl(&schema, SCRATCH) {
            sql.exec(&q, None)?;
        }
    }
    let core = TierSql::new_in(sql.clone(), &graftyard_git::tiers::CORE, None, SCRATCH);
    let meta = TierSql::new_in(sql.clone(), &graftyard_git::tiers::METADATA, fts.then_some(search_scratch), SCRATCH);
    Ok((core, meta))
}

fn search(table: &str, key: &Key, row: Option<&Stored>) -> Vec<Stmt> {
    search_in("", table, key, row)
}

fn search_scratch(table: &str, key: &Key, row: Option<&Stored>) -> Vec<Stmt> {
    search_in(SCRATCH, table, key, row)
}

/// The FTS5 rows for a metadata row: an issue's or pull request's title and body under its number, a comment's body
/// under its id. A contentless table deletes by rowid, so each write replaces the rowid's entry.
fn search_in(pre: &str, table: &str, key: &Key, row: Option<&Stored>) -> Vec<Stmt> {
    let (issue, comment) = fts_names(pre);
    let (fts, id) = match (table, key.first()) {
        ("issue", Some(Val::Int(n))) => (issue, *n),
        ("comment", Some(Val::Int(n))) => (comment, *n),
        _ => return Vec::new(),
    };
    let rec = |r: &Stored| match r.cols.first() {
        Some(Val::Blob(b)) => Some(b.clone()),
        _ => None,
    };
    let mut out: Vec<Stmt> =
        vec![(leak(format!("DELETE FROM {fts} WHERE rowid = ?")), vec![SqlStorageValue::Integer(id)])];
    let Some(b) = row.and_then(rec) else { return out };
    if table == "issue" {
        let r: graftyard_git::tiers::IssueRow = postcard::from_bytes(&b).expect("an issue row");
        out.push((
            leak(format!("INSERT INTO {fts} (rowid, title, body) VALUES (?, ?, ?)")),
            vec![SqlStorageValue::Integer(id), r.title.to_string().into(), r.body.to_string().into()],
        ));
    } else {
        let r: graftyard_git::tiers::CommentRow = postcard::from_bytes(&b).expect("a comment row");
        out.push((
            leak(format!("INSERT INTO {fts} (rowid, body) VALUES (?, ?)")),
            vec![SqlStorageValue::Integer(id), r.body.to_string().into()],
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use graftyard_fold::{Schema, TableDef};
    use graftyard_types::Tier;

    use worker::SqlStorageValue;

    use super::{columns, create, row_of, tables};

    /// The metadata tables `2ba89d5` created on the BF-20 members, before M2-WP2 rekeyed three of them.
    const SEEDED: Schema = Schema {
        tier: Tier::Metadata,
        tables: &[
            TableDef { name: "check_run", key: &["run"] },
            TableDef { name: "check_suite", key: &["suite"] },
            TableDef { name: "status", key: &["sha", "context", "id"] },
            TableDef { name: "comment", key: &["id"] },
        ],
    };

    /// `CREATE TABLE IF NOT EXISTS` with this release's DDL keeps a table created under an older key, so restore
    /// compares each table's columns with the shape `create` makes: the rekeyed tables differ, an unchanged one does
    /// not.
    #[test]
    fn a_table_created_under_an_older_key_is_told_apart_from_this_releases_shape() {
        let now = graftyard_git::tiers::METADATA;
        let arity = |s: &Schema, name: &str| tables(s).into_iter().find(|(t, _)| t == name).map(|(_, n)| n);
        let mut differ = Vec::new();
        for t in SEEDED.tables {
            let (old, new) = (arity(&SEEDED, t.name).unwrap(), arity(&now, t.name).unwrap());
            if columns(old) != columns(new) {
                differ.push(t.name);
            }
        }
        assert_eq!(differ, ["check_run", "check_suite", "status"]);
        assert_eq!(columns(2), ["k0", "k1", "written", "v"]);
        assert_eq!(columns(0), ["one", "written", "v"]);
        assert!(
            create("m_check_run", 2)
                .contains("(k0, k1, written INTEGER NOT NULL, v BLOB NOT NULL, PRIMARY KEY (k0, k1))")
        );
    }
    /// A stored row that does not decode is an error for the caller to treat as lost storage, never a panic.
    #[test]
    fn a_row_that_does_not_decode_is_an_error() {
        let cols = postcard::to_allocvec(&vec![graftyard_fold::Val::Int(7)]).expect("encodes");
        let ok = row_of(SqlStorageValue::Integer(3), SqlStorageValue::Blob(cols.clone())).expect("decodes");
        assert_eq!(ok.written.0, 3);
        assert!(row_of(SqlStorageValue::String("x".into()), SqlStorageValue::Blob(cols)).is_err());
        assert!(row_of(SqlStorageValue::Integer(3), SqlStorageValue::Integer(4)).is_err());
        assert!(row_of(SqlStorageValue::Integer(3), SqlStorageValue::Blob(vec![0xff; 3])).is_err());
    }
}
