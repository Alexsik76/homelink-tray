//! The file format. This is the only module that knows the configuration is TOML.
//!
//! The format is TOML. The format code is kept in this one module and the schema in `raw`
//! is format-neutral (serde), so a different format would touch only this file.
//!
//! Three operations:
//! - [`parse`]: text to the raw schema, with precise error positions;
//! - [`render`]: the raw schema to a fresh, tidy file (used when there is no file yet);
//! - [`edit`]: bring an *existing* file in line with a configuration by editing it by key,
//!   so that everything the user wrote by hand survives (see below).
//!
//! # Editing in place
//!
//! `edit` parses the existing text into a `toml_edit` document, which keeps comments,
//! blank lines, key order and formatting, and applies the new configuration on top:
//!
//! - A table, array or value that is *semantically equal* to the new one is not touched at
//!   all (an inline table and a standard table with the same content are equal), so an
//!   unchanged configuration gives byte-identical text.
//! - A changed value is replaced and keeps its decoration, hence its trailing comment.
//! - Keys that are gone are removed; new keys are appended to their table.
//! - `[[tunnels]]`, `[[routers]]` and `[[pcs]]` entries are matched by `id`, never by
//!   position. An entry that is gone is removed together with its sub-tables and the
//!   comments written above it. A new entry is appended after the last entry of its kind
//!   (or, when the file has none, before the first entry of a later kind).
//!   Entries that change place keep their comments and move as a whole.
//! - A changed `id` is a removal plus an addition (comments of the old entry are lost),
//!   unless the caller lists the rename: then the entry keeps its place, comments and
//!   unchanged keys and only the `id` value changes.

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, TableLike, Value};

use super::raw::RawConfig;

/// Default file name of the configuration.
pub const FILE_NAME: &str = "config.toml";

/// A syntax or structure error reported by the parser.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", self.describe())]
pub struct FormatError {
    /// What is wrong.
    pub message: String,
    /// 1-based line, when known.
    pub line: Option<usize>,
    /// 1-based column, when known.
    pub column: Option<usize>,
}

impl FormatError {
    fn describe(&self) -> String {
        match (self.line, self.column) {
            (Some(line), Some(column)) => {
                format!("line {line}, column {column}: {}", self.message)
            }
            _ => self.message.clone(),
        }
    }

    fn plain(message: String) -> Self {
        Self {
            message,
            line: None,
            column: None,
        }
    }
}

/// Entry renames to honour when editing in place, as `(old id, new id)` pairs.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct EntryRenames {
    pub tunnels: Vec<(String, String)>,
    pub routers: Vec<(String, String)>,
    pub pcs: Vec<(String, String)>,
}

impl EntryRenames {
    fn for_key(&self, key: &str) -> &[(String, String)] {
        match key {
            "tunnels" => &self.tunnels,
            "routers" => &self.routers,
            "pcs" => &self.pcs,
            _ => &[],
        }
    }
}

/// Tells whether two spellings of the string value of `key` mean the same thing (for
/// example `AA-BB-…` and `aa:bb:…` for a MAC address), so that a value the user spelled in
/// their own way is not rewritten when it did not change. Arguments: key, old, new.
pub(super) type Same<'a> = &'a dyn Fn(&str, &str, &str) -> bool;

const BOM: char = '\u{feff}';

/// The keys of the arrays of tables that are matched by `id`, in the order a new file
/// lists them.
const ENTRY_KEYS: [&str; 3] = ["tunnels", "routers", "pcs"];

/// Distance between the positions of tables after [`scale_positions`]; leaves room to put
/// moved and new tables between existing ones.
const GAP: isize = 10_000;
/// Distance between consecutive new entries placed in a gap.
const NEW_ENTRY_STEP: isize = 100;

/// Line and column (1-based) of byte `offset` in `text`.
fn locate(text: &str, offset: usize) -> (usize, usize) {
    let before = text.get(..offset).unwrap_or_default();
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, column)
}

fn error_at(text: &str, message: &str, span: Option<std::ops::Range<usize>>) -> FormatError {
    let (line, column) = match span {
        Some(span) => {
            let (l, c) = locate(text, span.start);
            (Some(l), Some(c))
        }
        None => (None, None),
    };
    FormatError {
        message: message.trim().to_owned(),
        line,
        column,
    }
}

pub(super) fn parse(text: &str) -> Result<RawConfig, FormatError> {
    // Notepad may add a byte order mark.
    let text = text.strip_prefix(BOM).unwrap_or(text);
    toml_edit::de::from_str(text).map_err(|err| error_at(text, err.message(), err.span()))
}

/// A tidy new file: plain values first, then one `[table]` per section separated by blank
/// lines.
pub(super) fn render(raw: &RawConfig) -> Result<String, FormatError> {
    let doc = new_document(raw)?;
    let mut doc = doc;
    let mut set_prefix = |t: &mut Table| t.decor_mut().set_prefix("\n");
    walk_tables(doc.as_table_mut(), &mut set_prefix);
    Ok(doc.to_string())
}

/// The raw configuration as a document made of standard tables (the serializer produces
/// inline tables, which are converted).
fn new_document(raw: &RawConfig) -> Result<DocumentMut, FormatError> {
    let mut doc = toml_edit::ser::to_document(raw)
        .map_err(|err| FormatError::plain(format!("cannot write the configuration: {err}")))?;
    for (_, item) in doc.as_table_mut().iter_mut() {
        to_standard(item);
    }
    Ok(doc)
}

fn to_standard(item: &mut Item) {
    let owned = std::mem::take(item);
    *item = match owned {
        Item::Value(Value::InlineTable(inline)) => Item::Table(standard_table(inline.into_table())),
        Item::Value(Value::Array(array))
            if !array.is_empty() && array.iter().all(Value::is_inline_table) =>
        {
            let mut tables = ArrayOfTables::new();
            for value in array {
                if let Value::InlineTable(inline) = value {
                    tables.push(standard_table(inline.into_table()));
                }
            }
            Item::ArrayOfTables(tables)
        }
        other => other,
    };
}

fn standard_table(mut table: Table) -> Table {
    for (_, item) in table.iter_mut() {
        to_standard(item);
    }
    table
}

/// Parses an existing file and edits it to match `raw` (see the module documentation).
pub(super) fn edit(
    existing: &str,
    raw: &RawConfig,
    renames: &EntryRenames,
    same: Same,
) -> Result<String, FormatError> {
    let (bom, body) = match existing.strip_prefix(BOM) {
        Some(body) => ("\u{feff}", body),
        None => ("", existing),
    };
    let mut doc: DocumentMut = body
        .parse()
        .map_err(|err: toml_edit::TomlError| error_at(body, err.message(), err.span()))?;
    let new_doc = new_document(raw)?;
    scale_positions(doc.as_table_mut());
    merge_root(doc.as_table_mut(), new_doc.as_table(), renames, same);
    place_unpositioned(doc.as_table_mut());
    Ok(format!("{bom}{doc}"))
}

// ---------------------------------------------------------------------------------------
// Traversal helpers
// ---------------------------------------------------------------------------------------

/// Calls `f` for every table below `table` (not `table` itself), depth first.
fn walk_tables(table: &mut Table, f: &mut dyn FnMut(&mut Table)) {
    for (_, item) in table.iter_mut() {
        match item {
            Item::Table(t) => {
                f(t);
                walk_tables(t, f);
            }
            Item::ArrayOfTables(array) => {
                for t in array.iter_mut() {
                    f(t);
                    walk_tables(t, f);
                }
            }
            _ => {}
        }
    }
}

fn scale_positions(root: &mut Table) {
    walk_tables(root, &mut |t| {
        let scaled = t.position().map(|p| p * GAP);
        t.set_position(scaled);
    });
}

/// Largest position of any table in `table`, 0 if there is none.
fn max_position(table: &mut Table) -> isize {
    let mut max = 0;
    walk_tables(table, &mut |t| max = max.max(t.position().unwrap_or(0)));
    max
}

/// Gives every table that still has no position one at the end of the document.
fn place_unpositioned(root: &mut Table) {
    let mut next = max_position(root) + GAP;
    walk_tables(root, &mut |t| {
        if t.position().is_none() {
            t.set_position(Some(next));
            next += 1;
        }
    });
}

/// Marks a table that is new to the file: a blank line before its header.
fn mark_new(item: &mut Item) {
    match item {
        Item::Table(t) => {
            t.decor_mut().set_prefix("\n");
            walk_tables(t, &mut |n| n.decor_mut().set_prefix("\n"));
        }
        Item::ArrayOfTables(array) => {
            for t in array.iter_mut() {
                t.decor_mut().set_prefix("\n");
                walk_tables(t, &mut |n| n.decor_mut().set_prefix("\n"));
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------
// Semantic equality
// ---------------------------------------------------------------------------------------

fn tables_of(item: &Item) -> Option<Vec<&dyn TableLike>> {
    match item {
        Item::ArrayOfTables(array) => Some(array.iter().map(|t| t as &dyn TableLike).collect()),
        Item::Value(Value::Array(array)) if !array.is_empty() => array
            .iter()
            .map(|v| v.as_inline_table().map(|t| t as &dyn TableLike))
            .collect(),
        _ => None,
    }
}

fn items_equal(key: &str, a: &Item, b: &Item, same: Same) -> bool {
    if let (Some(x), Some(y)) = (a.as_table_like(), b.as_table_like()) {
        return table_likes_equal(x, y, same);
    }
    if let (Some(x), Some(y)) = (tables_of(a), tables_of(b)) {
        return x.len() == y.len()
            && x.iter()
                .zip(&y)
                .all(|(p, q)| table_likes_equal(*p, *q, same));
    }
    match (a, b) {
        (Item::Value(x), Item::Value(y)) => values_equal(key, x, y, same),
        (Item::None, Item::None) => true,
        _ => false,
    }
}

fn table_likes_equal(a: &dyn TableLike, b: &dyn TableLike, same: Same) -> bool {
    a.len() == b.len()
        && a.iter()
            .all(|(k, x)| b.get(k).is_some_and(|y| items_equal(k, x, y, same)))
}

fn values_equal(key: &str, a: &Value, b: &Value, same: Same) -> bool {
    match (a, b) {
        (Value::String(x), Value::String(y)) => same(key, x.value(), y.value()),
        (Value::Integer(x), Value::Integer(y)) => x.value() == y.value(),
        (Value::Float(x), Value::Float(y)) => x.value() == y.value(),
        (Value::Boolean(x), Value::Boolean(y)) => x.value() == y.value(),
        (Value::Datetime(x), Value::Datetime(y)) => x.value() == y.value(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y.iter())
                    .all(|(p, q)| values_equal(key, p, q, same))
        }
        (Value::InlineTable(x), Value::InlineTable(y)) => table_likes_equal(x, y, same),
        _ => false,
    }
}

// ---------------------------------------------------------------------------------------
// Merging
// ---------------------------------------------------------------------------------------

fn merge_root(old: &mut Table, new: &Table, renames: &EntryRenames, same: Same) {
    remove_missing_keys(old, new);
    for (key, item) in new.iter() {
        if !ENTRY_KEYS.contains(&key) {
            merge_key(old, key, item, same);
        }
    }
    for (index, key) in ENTRY_KEYS.iter().enumerate() {
        if let Some(item) = new.get(key) {
            let later = ENTRY_KEYS.get(index + 1..).unwrap_or_default();
            merge_entries(old, key, item, renames.for_key(key), later, same);
        }
    }
}

fn remove_missing_keys(old: &mut Table, new: &Table) {
    let gone: Vec<String> = old
        .iter()
        .filter(|(k, _)| !new.contains_key(k))
        .map(|(k, _)| k.to_owned())
        .collect();
    for key in gone {
        old.remove(&key);
    }
}

/// Makes `old[key]` equal to `new_item`, touching as little as possible.
fn merge_key(old: &mut Table, key: &str, new_item: &Item, same: Same) {
    match old.get_mut(key) {
        None => {
            let mut item = new_item.clone();
            mark_new(&mut item);
            old.insert(key, item);
        }
        Some(existing) => {
            if !items_equal(key, existing, new_item, same) {
                merge_item(existing, new_item, same);
            }
        }
    }
}

fn merge_item(old: &mut Item, new: &Item, same: Same) {
    match (&mut *old, new) {
        (Item::Table(old_table), Item::Table(new_table)) => merge_table(old_table, new_table, same),
        (Item::Value(old_value), Item::Value(new_value)) => {
            // The new value, with the old decoration: spacing and trailing comment stay.
            let decor = old_value.decor().clone();
            let mut value = new_value.clone();
            *value.decor_mut() = decor;
            *old_value = value;
        }
        _ => {
            let mut item = new.clone();
            mark_new(&mut item);
            *old = item;
        }
    }
}

fn merge_table(old: &mut Table, new: &Table, same: Same) {
    remove_missing_keys(old, new);
    for (key, item) in new.iter() {
        merge_key(old, key, item, same);
    }
}

fn id_of(table: &Table) -> Option<&str> {
    table.get("id").and_then(Item::as_str)
}

/// A kept entry or a new one, on its way into the array.
struct Entry {
    table: Table,
    /// The position the entry had in the file; `None` for a new entry.
    old_start: Option<isize>,
}

/// Merges one array of tables by `id`.
fn merge_entries(
    root: &mut Table,
    key: &str,
    new_item: &Item,
    renames: &[(String, String)],
    later_keys: &[&str],
    same: Same,
) {
    let Item::ArrayOfTables(new_tables) = new_item else {
        return;
    };
    if let Some(existing) = root.get(key)
        && items_equal(key, existing, new_item, same)
    {
        return;
    }

    // Take the existing entries out; an array that is not an array of tables (an inline
    // array) is dropped and its entries are written fresh.
    let mut pool: Vec<Option<Table>> = match root.remove(key) {
        Some(Item::ArrayOfTables(array)) => array.into_iter().map(Some).collect(),
        _ => Vec::new(),
    };

    let mut entries: Vec<Entry> = Vec::new();
    for new_table in new_tables.iter() {
        let Some(new_id) = id_of(new_table) else {
            continue;
        };
        // A listed rename lets the entry with the old id be matched to this one.
        let renamed_from = renames
            .iter()
            .find(|(_, to)| to == new_id)
            .map(|(from, _)| from.as_str())
            .filter(|from| pool.iter().flatten().any(|t| id_of(t) == Some(from)));
        let wanted = renamed_from.unwrap_or(new_id);
        let found = pool
            .iter_mut()
            .find(|slot| slot.as_ref().is_some_and(|t| id_of(t) == Some(wanted)));
        match found.and_then(Option::take) {
            Some(mut table) => {
                let old_start = table.position();
                if !table_likes_equal(&table, new_table, same) {
                    merge_table(&mut table, new_table, same);
                }
                entries.push(Entry { table, old_start });
            }
            None => {
                let mut table = new_table.clone();
                table.decor_mut().set_prefix("\n");
                walk_tables(&mut table, &mut |t| t.decor_mut().set_prefix("\n"));
                table.set_position(None);
                entries.push(Entry {
                    table,
                    old_start: None,
                });
            }
        }
    }
    if entries.is_empty() {
        return;
    }
    place_entries(root, &mut entries, later_keys);

    let mut array = ArrayOfTables::new();
    for entry in entries {
        array.push(entry.table);
    }
    root.insert(key, Item::ArrayOfTables(array));
}

/// Decides where each entry is printed. Kept entries take the places the kept entries had,
/// in order, so entries that did not move stay exactly where they were and a reordering
/// moves whole entries between the same places. New entries follow the last kept one.
fn place_entries(root: &mut Table, entries: &mut [Entry], later_keys: &[&str]) {
    let mut starts: Vec<isize> = entries.iter().filter_map(|e| e.old_start).collect();
    starts.sort_unstable();

    // Entries that keep their place stay as they are; only children that are new to them get
    // a position (right after their last child).
    let mut in_place_end = None;
    for (index, entry) in entries.iter_mut().enumerate() {
        if starts.get(index).is_some() && entry.old_start == starts.get(index).copied() {
            position_new_children(&mut entry.table);
            in_place_end = in_place_end.max(Some(block_end(&mut entry.table)));
        }
    }

    // New entries come after everything that is already there, one step apart.
    let missing = entries.len() - starts.len();
    let first_new = match starts.last() {
        Some(last) => (last + NEW_ENTRY_STEP).max(in_place_end.map_or(0, |end| end + 1)),
        None => first_free_position(root, later_keys),
    };
    starts.extend((0..missing).map(|k| first_new + NEW_ENTRY_STEP * k as isize));

    for (index, entry) in entries.iter_mut().enumerate() {
        let slot = starts.get(index).copied();
        if let Some(slot) = slot
            && entry.old_start != Some(slot)
        {
            move_block(&mut entry.table, slot);
        }
    }
}

/// The largest position in an entry and its children.
fn block_end(entry: &mut Table) -> isize {
    max_position(entry).max(entry.position().unwrap_or(0))
}

/// Where the first entry of a kind with no entries goes: just before the first entry of a
/// later kind, or at the end of the document.
fn first_free_position(root: &mut Table, later_keys: &[&str]) -> isize {
    for key in later_keys {
        let first = match root.get(key) {
            Some(Item::ArrayOfTables(array)) => array.iter().filter_map(Table::position).min(),
            _ => None,
        };
        if let Some(first) = first {
            return first - GAP / 2;
        }
    }
    max_position(root) + GAP
}

/// Gives the children of an entry that stays in place and have no position one right
/// after the last child that has one, so they print inside the entry.
fn position_new_children(entry: &mut Table) {
    let mut last = entry.position().unwrap_or(0);
    walk_tables(entry, &mut |t| {
        last = last.max(t.position().unwrap_or(last))
    });
    walk_tables(entry, &mut |t| {
        if t.position().is_none() {
            last += 1;
            t.set_position(Some(last));
        }
    });
}

/// Puts an entry and its children at `start`, keeping their relative order; children that
/// have no position yet come last.
fn move_block(entry: &mut Table, start: isize) {
    entry.set_position(Some(start));
    let mut old: Vec<Option<isize>> = Vec::new();
    walk_tables(entry, &mut |t| old.push(t.position()));
    let mut order: Vec<usize> = (0..old.len()).collect();
    order.sort_by_key(|&i| (old.get(i).copied().flatten().unwrap_or(isize::MAX), i));
    let mut rank = vec![0isize; old.len()];
    for (r, &i) in order.iter().enumerate() {
        if let Some(slot) = rank.get_mut(i) {
            *slot = r as isize + 1;
        }
    }
    let mut index = 0usize;
    walk_tables(entry, &mut |t| {
        t.set_position(Some(start + rank.get(index).copied().unwrap_or(1)));
        index += 1;
    });
}
