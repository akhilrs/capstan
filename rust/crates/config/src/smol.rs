//! The TOML reader of the Node loader, in Rust: a port of `smol-toml` 1.8 (parse.js, struct.js, extract.js, primitive.js,
//! util.js and the date check of date.js) with `integersAsBigInt`. It decides what is valid TOML and where the first fault
//! is, and builds the document the loader resolves, so the Rust loader reads exactly what the Node loader reads. The Node
//! loader words a refusal as `capstan.toml is not valid TOML at line L, column C` from the error's pointer; the same
//! pointer, in UTF-16 code units as JavaScript counts them, gives the same words.
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::value::{Item, Table as ItemTable};

/// The default `maxDepth` of `parse`.
const MAX_DEPTH: i64 = 1000;

/// A refusal: the pointer (a UTF-16 index) the error is raised at.
type Refusal = usize;
type R<T> = Result<T, Refusal>;

/// What the loader makes of a refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

/// The document `smol-toml` makes of `source` (the text after the loader's BOM and CRLF handling), or where it refuses.
pub fn parse(source: &str) -> Result<ItemTable, Position> {
    let units: Vec<u16> = source.encode_utf16().collect();
    let mut parser = Parser::new(&units);
    match parser.document() {
        Ok(root) => match to_item(&root) {
            Item::Table(table) => Ok(table),
            _ => unreachable!("the root is a table"),
        },
        Err(ptr) => Err(match &parser.local {
            Some(local) => position(local, ptr),
            None => position(&units, ptr),
        }),
    }
}

/// `Some(position)` when `smol-toml` refuses `source` with a `TomlError`.
pub fn refusal(source: &str) -> Option<Position> {
    parse(source).err()
}

/// `getLineColFromPtr`: `string.slice(0, ptr).split(/\r\n|\n|\r/g)`: its length, and the length of the last piece plus one.
fn position(units: &[u16], ptr: usize) -> Position {
    let end = ptr.min(units.len());
    let (mut line, mut start, mut at) = (1, 0, 0);
    while at < end {
        match units[at] {
            0x0d if at + 1 < end && units[at + 1] == 0x0a => {
                line += 1;
                at += 2;
                start = at;
            }
            0x0a | 0x0d => {
                line += 1;
                at += 1;
                start = at;
            }
            _ => at += 1,
        }
    }
    Position {
        line,
        column: end - start + 1,
    }
}

// ---------------------------------------------------------------------------------------------------- tables

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Dotted = 0,
    Explicit = 1,
    Array = 2,
    ArrayDotted = 3,
}

struct State {
    t: Kind,
    d: bool,
    i: usize,
    c: Meta,
}

type StateRef = Rc<RefCell<State>>;
type Meta = Rc<RefCell<HashMap<String, StateRef>>>;
/// A table: its members in the order they were made.
type Table = Rc<RefCell<Vec<(String, Value)>>>;

fn get(table: &Table, key: &str) -> Option<Value> {
    table
        .borrow()
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

fn has(table: &Table, key: &str) -> bool {
    table.borrow().iter().any(|(k, _)| k == key)
}

/// `table[key] = value`: a new member goes last, an existing one keeps its place.
fn set(table: &Table, key: String, value: Value) {
    let mut members = table.borrow_mut();
    match members.iter_mut().find(|(k, _)| *k == key) {
        Some((_, slot)) => *slot = value,
        None => members.push((key, value)),
    }
}

fn new_meta() -> Meta {
    Rc::new(RefCell::new(HashMap::new()))
}

fn new_state(t: Kind) -> StateRef {
    Rc::new(RefCell::new(State {
        t,
        d: false,
        i: 0,
        c: new_meta(),
    }))
}

/// A value of the document. Tables and arrays have an identity, which inline tables track.
#[derive(Clone)]
enum Value {
    Table(Table, usize),
    Array(Rc<RefCell<Vec<Value>>>, usize),
    Scalar(Item),
}

impl Value {
    fn id(&self) -> Option<usize> {
        match self {
            Value::Table(_, id) | Value::Array(_, id) => Some(*id),
            Value::Scalar(_) => None,
        }
    }
}

/// What a lookup that cannot fail returns when it does (the bookkeeping never reaches it).
fn nothing() -> Value {
    Value::Scalar(Item::Bool(false))
}

fn to_item(value: &Value) -> Item {
    match value {
        Value::Scalar(item) => item.clone(),
        Value::Array(items, _) => Item::Array(items.borrow().iter().map(to_item).collect()),
        Value::Table(members, _) => Item::Table(ItemTable::from_members(
            members
                .borrow()
                .iter()
                .map(|(k, v)| (k.clone(), to_item(v)))
                .collect(),
        )),
    }
}

struct Parser<'a> {
    s: &'a [u16],
    p: usize,
    d: i64,
    next_id: usize,
    /// The text a refusal's pointer is an index into, when it is not the document: `sliceAndTrimEndOf` checks a comment
    /// inside the slice it cut.
    local: Option<Vec<u16>>,
}

const NONE: i32 = -1;

fn is_js_space(unit: u16) -> bool {
    matches!(
        unit,
        0x09..=0x0d
            | 0x20
            | 0xa0
            | 0x1680
            | 0x2000..=0x200a
            | 0x2028
            | 0x2029
            | 0x202f
            | 0x205f
            | 0x3000
            | 0xfeff
    )
}

impl<'a> Parser<'a> {
    fn new(s: &'a [u16]) -> Self {
        Parser {
            s,
            p: 0,
            d: MAX_DEPTH,
            next_id: 0,
            local: None,
        }
    }

    fn id(&mut self) -> usize {
        self.next_id += 1;
        self.next_id
    }

    fn new_table(&mut self) -> Value {
        let id = self.id();
        Value::Table(Rc::new(RefCell::new(Vec::new())), id)
    }

    /// `charCodeAt`: NaN (here `NONE`) outside the text.
    fn at(&self, index: usize) -> i32 {
        self.s.get(index).map_or(NONE, |u| i32::from(*u))
    }

    fn cur(&self) -> i32 {
        self.at(self.p)
    }

    fn index_of(&self, unit: u16, from: usize) -> Option<usize> {
        (from..self.s.len()).find(|i| self.s[*i] == unit)
    }

    // ------------------------------------------------------------------------------------------------ util.js

    /// `indexOfNewline(str, start)` over `str`: the index of the first `\n`, one less when a `\r` precedes it; `-1` as `None`.
    fn index_of_newline(text: &[u16], start: usize) -> Option<usize> {
        let at = (start..text.len()).find(|i| text[*i] == 0x0a)?;
        // `str.charCodeAt(idx - 1) === 0xd`
        Some(if at > 0 && text[at - 1] == 0x0d {
            at - 1
        } else {
            at
        })
    }

    /// `skipComment`.
    fn skip_comment(&mut self) -> R<()> {
        while self.p < self.s.len() {
            let c = self.cur();
            if c == 0x0a {
                break;
            }
            if c == 0x0d && self.at(self.p + 1) == 0x0a {
                self.p += 1;
                break;
            }
            if (c < 0x20 && c != 0x09) || c == 0x7f {
                return Err(self.p);
            }
            self.p += 1;
        }
        Ok(())
    }

    /// `skipVoid(ctx, banNewLines, banComments)`.
    fn skip_void(&mut self, ban_new_lines: bool, ban_comments: bool) -> R<()> {
        loop {
            let mut c;
            loop {
                c = self.cur();
                let skip = c == 0x20
                    || c == 0x09
                    || (!ban_new_lines
                        && (c == 0x0a || (c == 0x0d && self.at(self.p + 1) == 0x0a)));
                if !skip {
                    break;
                }
                self.p += 1;
            }
            if ban_comments || c != 0x23 {
                return Ok(());
            }
            self.skip_comment()?;
        }
    }

    /// `skipUntil(ctx, sep, end)`.
    fn skip_until(&mut self, sep: i32, end: Option<i32>) -> R<()> {
        let mut ptr = self.p;
        let Some(end) = end else {
            let found = Self::index_of_newline(self.s, ptr);
            self.p = found.unwrap_or(self.s.len());
            return Ok(());
        };
        while self.p < self.s.len() {
            let c = self.cur();
            if c == 0x23 {
                self.skip_comment()?;
            } else if c == end || c == sep {
                return Ok(());
            }
            self.p += 1;
        }
        ptr = ptr.min(self.s.len());
        Err(ptr)
    }

    // ------------------------------------------------------------------------------------------ primitive.js

    /// `parseString(ctx)`: the decoded text.
    fn parse_string(&mut self) -> R<String> {
        let start = self.p;
        let mut c = self.cur();
        self.p += 1;
        let first = c;
        let is_literal = c == 0x27;
        let is_multiline = c == self.cur() && c == self.at(self.p + 1);
        if is_multiline {
            self.p += 2;
            c = self.cur();
            if c == 0x0a {
                self.p += 1;
            } else if c == 0x0d && self.at(self.p + 1) == 0x0a {
                self.p += 2;
            }
        }
        let mut parsed: Vec<u16> = Vec::new();
        let mut slice_start = self.p;
        // 0 decoding, 1 decoding an escape, 2 whitespace escape before a newline, 3 whitespace escape after one.
        let mut state = 0;
        while self.p < self.s.len() {
            c = self.cur();
            if is_multiline && (c == 0x0a || (c == 0x0d && self.at(self.p + 1) == 0x0a)) {
                state = if state != 0 { 3 } else { 0 };
            } else if (c < 0x20 && c != 0x09) || c == 0x7f {
                return Err(self.p);
            } else if (state == 0 || state == 3)
                && c == first
                && (!is_multiline || (self.at(self.p + 1) == first && self.at(self.p + 2) == first))
            {
                if is_multiline {
                    if self.at(self.p + 3) == first {
                        self.p += 1;
                    }
                    if self.at(self.p + 3) == first {
                        self.p += 1;
                    }
                }
                if state == 0 {
                    parsed.extend_from_slice(&self.s[slice_start..self.p]);
                }
                self.p += if is_multiline { 3 } else { 1 };
                return Ok(String::from_utf16_lossy(&parsed));
            } else if state == 0 {
                if !is_literal && c == 0x5c {
                    parsed.extend_from_slice(&self.s[slice_start..self.p]);
                    slice_start = self.p;
                    state = 1;
                }
            } else if state == 1 {
                if c == 0x78 || c == 0x75 || c == 0x55 {
                    let mut value: i32 = 0;
                    let len = if c == 0x78 {
                        2
                    } else if c == 0x75 {
                        4
                    } else {
                        8
                    };
                    for _ in 0..len {
                        let hex = self.at(self.p + 1);
                        let digit = match hex {
                            0x30..=0x39 => hex - 0x30,
                            0x41..=0x46 => hex - 0x41 + 10,
                            0x61..=0x66 => hex - 0x61 + 10,
                            _ => -1,
                        };
                        if digit < 0 {
                            return Err(self.p + 1);
                        }
                        value = (value << 4) | digit;
                        self.p += 1;
                    }
                    if !(0..=0x10ffff).contains(&value) || (0xd800..=0xdfff).contains(&value) {
                        return Err(self.p);
                    }
                    let ch = char::from_u32(value as u32).expect("a scalar value");
                    let mut buffer = [0u16; 2];
                    parsed.extend_from_slice(ch.encode_utf16(&mut buffer));
                    slice_start = self.p + 1;
                    state = 0;
                } else if c == 0x20 || c == 0x09 {
                    state = 2;
                } else {
                    let decoded = match c {
                        0x62 => 0x08,
                        0x74 => 0x09,
                        0x6e => 0x0a,
                        0x66 => 0x0c,
                        0x72 => 0x0d,
                        0x65 => 0x1b,
                        0x22 => 0x22,
                        0x5c => 0x5c,
                        _ => return Err(self.p),
                    };
                    parsed.push(decoded);
                    slice_start = self.p + 1;
                    state = 0;
                }
            } else if c != 0x20 && c != 0x09 {
                if state == 2 {
                    return Err(slice_start);
                }
                state = if !is_literal && c == 0x5c { 1 } else { 0 };
                slice_start = self.p;
            }
            self.p += 1;
        }
        Err(start)
    }

    /// `parseValue(ctx, integersAsBigInt, end)`: numbers and dates, the legacy path of `extractValue`.
    fn parse_value(&mut self, end: Option<i32>) -> R<Item> {
        let ptr = self.p;
        self.skip_until(0x2c, end)?;
        let value = self.slice_and_trim_end(ptr, self.p)?;
        if value.is_empty() {
            return Err(ptr);
        }
        let text = String::from_utf16_lossy(&value);
        match text.as_str() {
            "-inf" | "inf" | "+inf" | "nan" | "+nan" | "-nan" => return Ok(Item::Float),
            "-0" => return Ok(Item::Int(0)),
            _ => {}
        }
        let integer = is_int(&value);
        if integer || is_float(&value) {
            if has_leading_zero(&value) {
                return Err(ptr);
            }
            let digits: String = text.chars().filter(|c| *c != '_').collect();
            if number_is_nan(&digits) {
                return Err(ptr);
            }
            return Ok(if integer {
                integer_item(&digits)
            } else {
                Item::Float
            });
        }
        if crate::smol_date::is_valid_toml_date(&text) {
            Ok(Item::Date)
        } else {
            Err(ptr)
        }
    }

    /// `sliceAndTrimEndOf`.
    fn slice_and_trim_end(&mut self, start: usize, end: usize) -> R<Vec<u16>> {
        let mut value: Vec<u16> = self.s[start..end.min(self.s.len())].to_vec();
        if let Some(comment) = value.iter().position(|u| *u == 0x23).filter(|i| *i > 0) {
            // The call to `skipComment` validates the comment (no control characters).
            let mut inner = Parser::new(&value);
            inner.p = comment;
            inner.d = 0;
            if let Err(ptr) = inner.skip_comment() {
                self.local = Some(value);
                return Err(ptr);
            }
            value.truncate(comment);
        }
        while value.last().is_some_and(|u| is_js_space(*u)) {
            value.pop();
        }
        Ok(value)
    }

    // ------------------------------------------------------------------------------------------- struct.js

    /// `parseKey(ctx, end)`: the key's parts.
    fn parse_key(&mut self, end: u16) -> R<Vec<String>> {
        let start = self.p;
        let mut dot: isize = start as isize - 1;
        let mut parsed = Vec::new();
        let Some(mut end_ptr) = self.index_of(end, start) else {
            return Err(start);
        };
        loop {
            dot += 1;
            self.p = dot as usize;
            let c = self.cur();
            if c != 0x20 && c != 0x09 {
                if c == 0x22 || c == 0x27 {
                    if c == self.at(self.p + 1) && c == self.at(self.p + 2) {
                        return Err(self.p);
                    }
                    let part = self.parse_string()?;
                    dot = self.index_of(0x2e, self.p).map_or(-1, |i| i as isize);
                    let stop = if dot < 0 || dot as usize > end_ptr {
                        end_ptr
                    } else {
                        dot as usize
                    };
                    let str_end: &[u16] = if self.p <= stop {
                        &self.s[self.p..stop]
                    } else {
                        &[]
                    };
                    if let Some(newline) = Self::index_of_newline(str_end, 0) {
                        // The pointer is an index into the slice, as in the original.
                        return Err(newline);
                    }
                    if str_end.iter().any(|u| !is_js_space(*u)) {
                        return Err(self.p);
                    }
                    if end_ptr < self.p {
                        match self.index_of(end, self.p) {
                            Some(found) => end_ptr = found,
                            None => return Err(start),
                        }
                    }
                    parsed.push(part);
                } else {
                    dot = self.index_of(0x2e, self.p).map_or(-1, |i| i as isize);
                    let stop = if dot < 0 || dot as usize > end_ptr {
                        end_ptr
                    } else {
                        dot as usize
                    };
                    let part: &[u16] = if self.p <= stop {
                        &self.s[self.p..stop]
                    } else {
                        &[]
                    };
                    if !is_key_part(part) {
                        return Err(self.p);
                    }
                    let mut trimmed = part.to_vec();
                    while trimmed.last().is_some_and(|u| is_js_space(*u)) {
                        trimmed.pop();
                    }
                    parsed.push(String::from_utf16_lossy(&trimmed));
                }
            }
            // Until there is no more dot.
            if !(dot + 1 != 0 && dot < end_ptr as isize) {
                break;
            }
        }
        self.p = end_ptr + 1;
        self.skip_void(true, true)?;
        Ok(parsed)
    }

    /// `parseInlineTable`: the table, or the pointer it is refused at.
    fn parse_inline_table(&mut self) -> R<Value> {
        let result = self.new_table();
        let Value::Table(table, _) = &result else {
            unreachable!()
        };
        let mut seen: HashSet<usize> = HashSet::new();
        self.p += 1;
        while self.p < self.s.len() {
            self.skip_void(false, false)?;
            if self.cur() == 0x7d {
                self.p += 1;
                return Ok(result);
            }
            let mut t: Table = Rc::clone(table);
            let mut has_own = false;
            let mut k = String::new();
            let p = self.p;
            let key = self.parse_key(0x3d)?;
            for (i, part) in key.iter().enumerate() {
                if i > 0 {
                    let next = if has_own {
                        get(&t, &k).unwrap_or_else(nothing)
                    } else {
                        let created = self.new_table();
                        set(&t, k.clone(), created.clone());
                        created
                    };
                    match next {
                        Value::Table(inner, _) => t = inner,
                        // Assigning into anything else is refused above before it is reached.
                        _ => return Err(p),
                    }
                }
                k = part.clone();
                let existing = get(&t, &k);
                has_own = existing.is_some();
                if let Some(existing) = existing {
                    let redefined = match existing.id() {
                        None => true,
                        Some(id) => seen.contains(&id),
                    };
                    if redefined {
                        return Err(p);
                    }
                }
            }
            if has_own {
                return Err(self.p);
            }
            let value = self.extract_value(Some(0x7d))?;
            if let Some(id) = value.id() {
                seen.insert(id);
            }
            set(&t, k, value);
            self.skip_void(false, false)?;
            let c = self.cur();
            self.p += 1;
            if c == 0x7d {
                return Ok(result);
            }
            if c != 0x2c {
                return Err(self.p - 1);
            }
        }
        Err(self.p)
    }

    /// `parseArray`.
    fn parse_array(&mut self) -> R<Value> {
        let id = self.id();
        let result = Value::Array(Rc::new(RefCell::new(Vec::new())), id);
        self.p += 1;
        while self.p < self.s.len() {
            self.skip_void(false, false)?;
            if self.cur() == 0x5d {
                self.p += 1;
                return Ok(result);
            }
            let value = self.extract_value(Some(0x5d))?;
            if let Value::Array(items, _) = &result {
                items.borrow_mut().push(value);
            }
            self.skip_void(false, false)?;
            let c = self.cur();
            self.p += 1;
            if c == 0x5d {
                return Ok(result);
            }
            if c != 0x2c {
                return Err(self.p - 1);
            }
        }
        Err(self.p)
    }

    // ------------------------------------------------------------------------------------------- extract.js

    fn extract_value(&mut self, end: Option<i32>) -> R<Value> {
        let ptr = self.p;
        let c = self.cur();
        if c == 0x5b || c == 0x7b {
            let depth = self.d;
            self.d -= 1;
            if depth == 0 {
                return Err(ptr);
            }
            let value = if c == 0x5b {
                self.parse_array()?
            } else {
                self.parse_inline_table()?
            };
            self.d += 1;
            return Ok(value);
        }
        if c == 0x22 || c == 0x27 {
            let text = self.parse_string()?;
            return Ok(Value::Scalar(Item::Str(text)));
        }
        if c == 0x74 {
            for expected in [0x72, 0x75, 0x65] {
                self.p += 1;
                if self.cur() != expected {
                    return Err(ptr);
                }
            }
            self.p += 1;
            return Ok(Value::Scalar(Item::Bool(true)));
        }
        if c == 0x66 {
            for expected in [0x61, 0x6c, 0x73, 0x65] {
                self.p += 1;
                if self.cur() != expected {
                    return Err(ptr);
                }
            }
            self.p += 1;
            return Ok(Value::Scalar(Item::Bool(false)));
        }
        Ok(Value::Scalar(self.parse_value(end)?))
    }

    // --------------------------------------------------------------------------------------------- parse.js

    /// `peekTable(key, table, meta, type)`: the key, table and meta of the place the next declaration or value goes, or
    /// `None` for a redefinition.
    fn peek_table(
        &mut self,
        key: &[String],
        table: &Table,
        meta: &Meta,
        kind: Kind,
    ) -> Option<(String, Table, Meta)> {
        let mut t: Table = Rc::clone(table);
        let mut m: Meta = Rc::clone(meta);
        let mut k = String::new();
        let mut has_own = false;
        for (i, part) in key.iter().enumerate() {
            if i > 0 {
                let next = if has_own {
                    get(&t, &k).unwrap_or_else(nothing)
                } else {
                    let created = self.new_table();
                    set(&t, k.clone(), created.clone());
                    created
                };
                let entered = m.borrow().get(&k).cloned()?;
                let child = Rc::clone(&entered.borrow().c);
                m = child;
                let state_kind = entered.borrow().t;
                if kind == Kind::Dotted
                    && (state_kind == Kind::Explicit || state_kind == Kind::Array)
                {
                    return None;
                }
                match next {
                    Value::Table(inner, _) => t = inner,
                    Value::Array(items, _) if state_kind == Kind::Array => {
                        let last = items.borrow().len().checked_sub(1)?;
                        let element = items.borrow().get(last).cloned()?;
                        match element {
                            Value::Table(inner, _) => t = inner,
                            _ => return None,
                        }
                        let element_state = m.borrow().get(&last.to_string()).cloned()?;
                        let element_meta = Rc::clone(&element_state.borrow().c);
                        m = element_meta;
                    }
                    _ => return None,
                }
            }
            k = part.clone();
            has_own = has(&t, &k);
            if has_own {
                if let Some(existing) = m.borrow().get(&k) {
                    let existing = existing.borrow();
                    if existing.t == Kind::Dotted && existing.d {
                        return None;
                    }
                }
            }
            if !has_own {
                let entry_kind = if i < key.len() - 1 && kind == Kind::Array {
                    Kind::ArrayDotted
                } else {
                    kind
                };
                m.borrow_mut().insert(k.clone(), new_state(entry_kind));
            }
        }
        let current = m.borrow().get(&k).cloned()?;
        {
            let existing = current.borrow();
            if existing.t != kind && !(kind == Kind::Explicit && existing.t == Kind::ArrayDotted) {
                return None;
            }
        }
        let mut state_ref = current;
        if kind == Kind::Array {
            if !state_ref.borrow().d {
                state_ref.borrow_mut().d = true;
                let id = self.id();
                set(
                    &t,
                    k.clone(),
                    Value::Array(Rc::new(RefCell::new(Vec::new())), id),
                );
            }
            let fresh = self.new_table();
            if let Some(Value::Array(items, _)) = get(&t, &k) {
                items.borrow_mut().push(fresh.clone());
            }
            if let Value::Table(inner, _) = &fresh {
                t = Rc::clone(inner);
            }
            let index = state_ref.borrow().i;
            state_ref.borrow_mut().i += 1;
            let element = new_state(Kind::Explicit);
            state_ref
                .borrow()
                .c
                .borrow_mut()
                .insert(index.to_string(), Rc::clone(&element));
            state_ref = element;
        }
        if state_ref.borrow().d {
            return None;
        }
        state_ref.borrow_mut().d = true;
        if kind == Kind::Explicit {
            let next = if has_own {
                get(&t, &k).unwrap_or_else(nothing)
            } else {
                let created = self.new_table();
                set(&t, k.clone(), created.clone());
                created
            };
            match next {
                Value::Table(inner, _) => t = inner,
                _ => return None,
            }
        } else if kind == Kind::Dotted && has_own {
            return None;
        }
        let child = Rc::clone(&state_ref.borrow().c);
        Some((k, t, child))
    }

    /// `parse(toml)`: the root table.
    fn document(&mut self) -> R<Value> {
        let res = self.new_table();
        let Value::Table(root, _) = &res else {
            unreachable!()
        };
        let meta = new_meta();
        let mut tbl: Table = Rc::clone(root);
        let mut m: Meta = Rc::clone(&meta);
        self.skip_void(false, false)?;
        while self.p < self.s.len() {
            if self.cur() == 0x5b {
                self.p += 1;
                let is_table_array = self.cur() == 0x5b;
                self.p += usize::from(is_table_array);
                let tmp = self.p;
                let key = self.parse_key(0x5d)?;
                if is_table_array {
                    if self.at(self.p - 1) != 0x5d {
                        return Err(self.p - 1);
                    }
                    self.p += 1;
                }
                let kind = if is_table_array {
                    Kind::Array
                } else {
                    Kind::Explicit
                };
                let Some((_, table, meta_next)) = self.peek_table(&key, root, &meta, kind) else {
                    return Err(tmp);
                };
                m = meta_next;
                tbl = table;
            } else {
                let tmp = self.p;
                let key = self.parse_key(0x3d)?;
                let Some((k, table, _)) = self.peek_table(&key, &tbl, &m, Kind::Dotted) else {
                    return Err(tmp);
                };
                let value = self.extract_value(None)?;
                set(&table, k, value);
            }
            self.skip_void(true, false)?;
            if self.p < self.s.len() {
                let c = self.cur();
                if c != 0x0a && c != 0x0d {
                    return Err(self.p);
                }
            }
            self.skip_void(false, false)?;
        }
        Ok(res)
    }
}

// ------------------------------------------------------------------------------------------------ patterns

fn digit(unit: u16) -> bool {
    (0x30..=0x39).contains(&unit)
}

fn hex_digit(unit: u16) -> bool {
    digit(unit) || (0x41..=0x46).contains(&unit) || (0x61..=0x66).contains(&unit)
}

/// `/^[a-zA-Z0-9-_]+[ \t]*$/`.
fn is_key_part(part: &[u16]) -> bool {
    let word = |u: u16| {
        digit(u)
            || (0x41..=0x5a).contains(&u)
            || (0x61..=0x7a).contains(&u)
            || u == 0x2d
            || u == 0x5f
    };
    let lead = part.iter().take_while(|u| word(**u)).count();
    lead > 0 && part[lead..].iter().all(|u| *u == 0x20 || *u == 0x09)
}

/// `d(_?d)*` over `text`, whole.
fn digits_with_underscores(text: &[u16], valid: fn(u16) -> bool) -> bool {
    let Some((first, rest)) = text.split_first() else {
        return false;
    };
    if !valid(*first) {
        return false;
    }
    let mut at = 0;
    while at < rest.len() {
        if rest[at] == 0x5f {
            if at + 1 < rest.len() && valid(rest[at + 1]) {
                at += 2;
                continue;
            }
            return false;
        }
        if !valid(rest[at]) {
            return false;
        }
        at += 1;
    }
    true
}

/// `INT_REGEX`: `^((0x[0-9a-fA-F](_?[0-9a-fA-F])*)|(([+-]|0[ob])?\d(_?\d)*))$`.
fn is_int(value: &[u16]) -> bool {
    if value.len() > 2
        && value[0] == 0x30
        && value[1] == 0x78
        && digits_with_underscores(&value[2..], hex_digit)
    {
        return true;
    }
    let rest = match value {
        [0x2b | 0x2d, rest @ ..] => rest,
        [0x30, 0x62 | 0x6f, rest @ ..] if digits_with_underscores(rest, digit) => return true,
        other => other,
    };
    digits_with_underscores(rest, digit)
}

/// `FLOAT_REGEX`: `^[+-]?\d(_?\d)*(\.\d(_?\d)*)?([eE][+-]?\d(_?\d)*)?$`.
fn is_float(value: &[u16]) -> bool {
    let rest = match value {
        [0x2b | 0x2d, rest @ ..] => rest,
        other => other,
    };
    let exponent_at = rest.iter().position(|u| *u == 0x65 || *u == 0x45);
    let (mantissa, exponent) = match exponent_at {
        Some(at) => (&rest[..at], Some(&rest[at + 1..])),
        None => (rest, None),
    };
    let mantissa_ok = match mantissa.iter().position(|u| *u == 0x2e) {
        Some(dot) => {
            digits_with_underscores(&mantissa[..dot], digit)
                && digits_with_underscores(&mantissa[dot + 1..], digit)
        }
        None => digits_with_underscores(mantissa, digit),
    };
    if !mantissa_ok {
        return false;
    }
    match exponent {
        None => true,
        Some(exponent) => {
            let exponent = match exponent {
                [0x2b | 0x2d, rest @ ..] => rest,
                other => other,
            };
            digits_with_underscores(exponent, digit)
        }
    }
}

/// `LEADING_ZERO`: `^[+-]?0[0-9_]`.
fn has_leading_zero(value: &[u16]) -> bool {
    let rest = match value {
        [0x2b | 0x2d, rest @ ..] => rest,
        other => other,
    };
    matches!(rest, [0x30, next, ..] if digit(*next) || *next == 0x5f)
}

/// `BigInt(text)` for an integer text (underscores removed): an `Int` when it fits 64 bits, else a `BigInt`.
fn integer_item(text: &str) -> Item {
    let (negative, unsigned) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let (radix, digits) = if let Some(rest) = unsigned.strip_prefix("0x") {
        (16, rest)
    } else if let Some(rest) = unsigned.strip_prefix("0o") {
        (8, rest)
    } else if let Some(rest) = unsigned.strip_prefix("0b") {
        (2, rest)
    } else {
        (10, unsigned)
    };
    let mut magnitude: u128 = 0;
    for digit in digits.chars() {
        let Some(value) = digit.to_digit(radix) else {
            return Item::BigInt(negative);
        };
        match magnitude
            .checked_mul(u128::from(radix))
            .and_then(|m| m.checked_add(u128::from(value)))
        {
            Some(next) => magnitude = next,
            None => return Item::BigInt(negative),
        }
    }
    let signed = if negative {
        0i128.checked_sub_unsigned(magnitude)
    } else {
        i128::try_from(magnitude).ok()
    };
    match signed.and_then(|n| i64::try_from(n).ok()) {
        Some(n) => Item::Int(n),
        None => Item::BigInt(negative),
    }
}

/// Whether `+value` is NaN for text that matched the integer or float pattern (underscores already removed).
fn number_is_nan(value: &str) -> bool {
    let (sign_free, _) = match value.strip_prefix(['+', '-']) {
        Some(rest) => (rest, true),
        None => (value, false),
    };
    if let Some(rest) = sign_free.strip_prefix("0b") {
        return !rest.bytes().all(|b| b == b'0' || b == b'1') || rest.is_empty();
    }
    if let Some(rest) = sign_free.strip_prefix("0o") {
        return !rest.bytes().all(|b| (b'0'..=b'7').contains(&b)) || rest.is_empty();
    }
    // A decimal, float or hexadecimal text that matched the patterns is a number.
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(source: &str) -> Option<(usize, usize)> {
        refusal(source).map(|p| (p.line, p.column))
    }

    #[test]
    fn the_document_has_the_values_and_the_order_javascript_gives() {
        let table = parse(
            "b = 1\n2 = 3\n[t]\nx = 0xff\ny = 99999999999999999999\nz = 1.5\nd = 1979-05-27\n",
        )
        .unwrap();
        let keys: Vec<&str> = table.keys().collect();
        assert_eq!(keys, ["2", "b", "t"]);
        let Some(Item::Table(t)) = table.get("t") else {
            panic!()
        };
        assert_eq!(t.get("x"), Some(&Item::Int(255)));
        assert_eq!(t.get("y"), Some(&Item::BigInt(false)));
        assert_eq!(t.get("z"), Some(&Item::Float));
        assert_eq!(t.get("d"), Some(&Item::Date));
    }

    #[test]
    fn a_document_that_parses_has_no_refusal() {
        assert_eq!(at("a = 1\n[t]\nb = \"x\"\n[[u]]\nc = [1, 2]\n"), None);
        assert_eq!(at(""), None);
    }

    #[test]
    fn positions_follow_javascript_lengths() {
        assert_eq!(at("a = \n"), Some((1, 5)));
        assert_eq!(at("a = 1\na = 2\n"), Some((2, 1)));
        assert_eq!(at("a = 1 b = 2\n"), Some((1, 5)));
        // Two UTF-16 units for the astral character before the error.
        assert_eq!(at("a = \"\u{1f600}\" x\n"), Some((1, 10)));
    }
}
