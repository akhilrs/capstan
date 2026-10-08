//! The parsed TOML document as the Node loader sees it (`smol-toml` with `integersAsBigInt`): strings, integers,
//! booleans, arrays and tables. Floats and dates are not represented: a document that holds one is left to Node (see `convert`).
//! Table members are kept in JavaScript property order: integer-like keys ascending, then the rest as written.

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Str(String),
    Int(i64),
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

    pub fn from_members(mut members: Vec<(String, Item)>) -> Table {
        // A stable sort keeps the written order within the non-index keys.
        members.sort_by_key(|(k, _)| array_index(k).map_or((1u8, 0u32), |i| (0, i)));
        Table(members)
    }
}

/// A document the Rust parser reads but that this port will not interpret.
#[derive(Debug)]
pub struct Unsupported;

/// Converts the parser's value. A key that JavaScript treats specially (`__proto__`), and any float or date, is `Unsupported`,
/// so the caller defers: a valid configuration holds neither, and how `smol-toml` reads the many spellings of them (leap
/// seconds, offsets, `nan`, `1.`) is not worth reproducing for a document that Node will refuse anyway.
pub fn convert(value: toml::Value) -> Result<Item, Unsupported> {
    Ok(match value {
        toml::Value::String(s) => Item::Str(s),
        toml::Value::Integer(n) => Item::Int(n),
        toml::Value::Boolean(b) => Item::Bool(b),
        toml::Value::Array(items) => {
            Item::Array(items.into_iter().map(convert).collect::<Result<_, _>>()?)
        }
        toml::Value::Table(table) => {
            let mut members = Vec::with_capacity(table.len());
            for (key, value) in table {
                if key == "__proto__" {
                    return Err(Unsupported);
                }
                members.push((key, convert(value)?));
            }
            Item::Table(Table::from_members(members))
        }
        toml::Value::Float(_) | toml::Value::Datetime(_) => return Err(Unsupported),
    })
}
