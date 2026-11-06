//! The parsed TOML document as the Node loader sees it (`smol-toml` with `integersAsBigInt`): strings, integers,
//! booleans, arrays and tables, and the two kinds the loader only ever refuses, floats and dates, which carry no value.
//! Table members are kept in JavaScript property order: integer-like keys ascending, then the rest as written.

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Str(String),
    /// An integer that fits 64 bits.
    Int(i64),
    /// An integer beyond 64 bits (`true` when negative): a `bigint` the loader finds out of every range it checks.
    BigInt(bool),
    /// A float; the loader refuses it where it expects an integer, string, boolean or table, and only `[cache] max_size_gb` reads its value.
    Float(f64),
    /// A date or time, an object that is not a plain table.
    Date,
    Bool(bool),
    Array(Vec<Item>),
    Table(Table),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table(Vec<(String, Item)>);

/// `Object.keys` puts array indexes (canonical numbers below 2^32 - 1) first, ascending.
fn array_index(key: &str) -> Option<u32> {
    let bytes = key.as_bytes();
    if bytes.is_empty() || bytes.len() > 10 || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if bytes.len() > 1 && bytes[0] == b'0' {
        return None;
    }
    key.parse::<u64>()
        .ok()
        .filter(|n| *n < 0xffff_ffff)
        .map(|n| n as u32)
}

impl Table {
    pub const fn empty() -> Table {
        Table(Vec::new())
    }

    pub fn get(&self, key: &str) -> Option<&Item> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(k, _)| k.as_str())
    }

    pub fn entries(&self) -> impl Iterator<Item = (&str, &Item)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// This table with `over` laid on top: tables merge key by key, recursively; any other value in `over` (a scalar or an
    /// array) replaces the one here, and a key only one side has is kept.
    pub fn merged_with(&self, over: &Table) -> Table {
        let mut members: Vec<(String, Item)> = self.0.clone();
        for (key, item) in over.entries() {
            match members.iter_mut().find(|(k, _)| k == key) {
                Some((_, slot)) => {
                    *slot = match (&*slot, item) {
                        (Item::Table(base), Item::Table(top)) => Item::Table(base.merged_with(top)),
                        _ => item.clone(),
                    };
                }
                None => members.push((key.to_string(), item.clone())),
            }
        }
        Table::from_members(members)
    }

    pub fn from_members(mut members: Vec<(String, Item)>) -> Table {
        // A stable sort keeps the written order within the non-index keys.
        members.sort_by_key(|(k, _)| array_index(k).map_or((1u8, 0u32), |i| (0, i)));
        Table(members)
    }
}
