//! `@pane/json`: `encode(value)` and `decode(text)` return `(value, nil)` or
//! `(nil, { kind, message })`. `null` is a shared, protected userdata sentinel;
//! encoding Lua `nil` also produces JSON null. `array(table)` and `object(table)`
//! mark and return the original table without changing its metatable. Decoded
//! containers retain these markers, including empty arrays. Unmarked empty
//! tables are objects; other tables must have exclusively string keys or dense
//! positive integer keys starting at one. Metamethods are never consulted.
//!
//! Numbers use Luau's IEEE-754 doubles. Integers outside ±(2^53 - 1) are rejected
//! rather than silently rounded, on both paths; fractional JSON numbers may
//! round to the nearest representable double. NaN and infinities are rejected.
//! Documents are limited to 1 MiB of UTF-8 JSON, 64 container levels, and 100,000
//! values. Encoding additionally limits the aggregate unescaped string bytes.

use std::collections::HashSet;
use std::io::{self, Write};

use rune_luau::mlua::{self, AnyUserData, Lua, Table, UserData, Value};
use serde_json::{Map, Number, Value as Json};

const MAX_BYTES: usize = 1024 * 1024;
const MAX_DEPTH: usize = 64;
const MAX_VALUES: usize = 100_000;
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
const MAX_SAFE_INTEGER_U64: u64 = 9_007_199_254_740_991;

struct Null;
impl UserData for Null {}

struct Fault {
    kind: &'static str,
    message: String,
}

impl Fault {
    fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl From<mlua::Error> for Fault {
    fn from(error: mlua::Error) -> Self {
        Self::new("internal", error.to_string())
    }
}

type Checked<T> = Result<T, Fault>;

#[derive(Default)]
struct Budget {
    values: usize,
    bytes: usize,
}

impl Budget {
    fn value(&mut self) -> Checked<()> {
        self.values += 1;
        if self.values > MAX_VALUES {
            return Err(Fault::new("limit", "JSON exceeds the 100,000-value limit"));
        }
        Ok(())
    }

    fn string(&mut self, string: &str) -> Checked<()> {
        self.bytes = self.bytes.saturating_add(string.len());
        if self.bytes > MAX_BYTES {
            return Err(Fault::new("limit", "JSON exceeds the 1 MiB string limit"));
        }
        Ok(())
    }
}

fn container_depth(depth: usize) -> Checked<()> {
    if depth >= MAX_DEPTH {
        return Err(Fault::new(
            "limit",
            "JSON exceeds the 64-container depth limit",
        ));
    }
    Ok(())
}

fn check_number(number: f64) -> Checked<f64> {
    if !number.is_finite() {
        return Err(Fault::new("number", "JSON numbers must be finite"));
    }
    if number.fract() == 0.0 && number.abs() > MAX_SAFE_INTEGER {
        return Err(Fault::new(
            "number",
            "JSON integers must be within ±(2^53 - 1)",
        ));
    }
    Ok(number)
}

fn utf8(string: &mlua::LuaString) -> Checked<String> {
    string
        .to_str()
        .map(|s| s.to_owned())
        .map_err(|_| Fault::new("utf8", "JSON strings and object keys must be valid UTF-8"))
}

struct Encoder<'a> {
    markers: &'a Table,
    active: HashSet<*const std::ffi::c_void>,
    budget: Budget,
}

impl Encoder<'_> {
    fn value(&mut self, value: Value, depth: usize) -> Checked<Json> {
        self.budget.value()?;
        match value {
            Value::Nil => Ok(Json::Null),
            Value::Boolean(value) => Ok(Json::Bool(value)),
            Value::Integer(value) => {
                if value.unsigned_abs() > MAX_SAFE_INTEGER_U64 {
                    return Err(Fault::new(
                        "number",
                        "JSON integers must be within ±(2^53 - 1)",
                    ));
                }
                Ok(Json::Number(Number::from(value)))
            }
            Value::Number(value) => {
                let value = check_number(value)?;
                Number::from_f64(value)
                    .map(Json::Number)
                    .ok_or_else(|| Fault::new("number", "JSON numbers must be finite"))
            }
            Value::String(value) => {
                if value.as_bytes().len() > MAX_BYTES {
                    return Err(Fault::new("limit", "JSON string exceeds 1 MiB"));
                }
                let value = utf8(&value)?;
                self.budget.string(&value)?;
                Ok(Json::String(value))
            }
            Value::UserData(value) if value.is::<Null>() => Ok(Json::Null),
            Value::Table(table) => {
                container_depth(depth)?;
                let pointer = table.to_pointer();
                if !self.active.insert(pointer) {
                    return Err(Fault::new("cycle", "cannot encode a cyclic table"));
                }
                let result = self.table(table, depth + 1);
                self.active.remove(&pointer);
                result
            }
            value => Err(Fault::new(
                "type",
                format!("cannot encode {} as JSON", value.type_name()),
            )),
        }
    }

    fn table(&mut self, table: Table, depth: usize) -> Checked<Json> {
        let marker: Option<bool> = self.markers.raw_get(table.clone())?;
        let mut array = Vec::new();
        let mut object = Map::new();
        for entry in table.pairs::<Value, Value>() {
            let (key, value) = entry?;
            match key {
                Value::String(key) if marker != Some(true) && array.is_empty() => {
                    if key.as_bytes().len() > MAX_BYTES {
                        return Err(Fault::new("limit", "JSON object key exceeds 1 MiB"));
                    }
                    let key = utf8(&key)?;
                    self.budget.string(&key)?;
                    object.insert(key, self.value(value, depth)?);
                }
                Value::Integer(_) | Value::Number(_)
                    if marker != Some(false) && object.is_empty() =>
                {
                    let index = match key {
                        Value::Integer(index) => f64::from(i32::try_from(index).map_err(|_| {
                            Fault::new(
                                "table",
                                "JSON array index exceeds the bounded dense array range",
                            )
                        })?),
                        Value::Number(index) => index,
                        _ => unreachable!(),
                    };
                    if !index.is_finite() || index < 1.0 || index.fract() != 0.0 {
                        return Err(Fault::new(
                            "table",
                            "JSON array keys must be positive integers",
                        ));
                    }
                    // Keep numeric indices as doubles: no lossy or overflowing cast.
                    array.push((index, self.value(value, depth)?));
                }
                _ => {
                    return Err(Fault::new(
                        "table",
                        "expected string object keys or dense integer array keys, not mixed keys",
                    ));
                }
            }
        }
        if marker == Some(true) || !array.is_empty() {
            array.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
            let mut expected = 1.0;
            for (index, _) in &array {
                if *index != expected {
                    return Err(Fault::new(
                        "table",
                        "JSON arrays must be dense and start at index one",
                    ));
                }
                expected += 1.0;
            }
            Ok(Json::Array(
                array.into_iter().map(|(_, value)| value).collect(),
            ))
        } else {
            Ok(Json::Object(object))
        }
    }
}

struct BoundedOutput(Vec<u8>);

impl Write for BoundedOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_BYTES - self.0.len() {
            return Err(io::Error::other("JSON output exceeds 1 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encode(lua: &Lua, markers: &Table, value: Value) -> Checked<Value> {
    let value = Encoder {
        markers,
        active: HashSet::new(),
        budget: Budget::default(),
    }
    .value(value, 0)?;
    let mut output = BoundedOutput(Vec::new());
    serde_json::to_writer(&mut output, &value)
        .map_err(|error| Fault::new("limit", error.to_string()))?;
    Ok(Value::String(lua.create_string(output.0)?))
}

fn to_lua(
    lua: &Lua,
    markers: &Table,
    null: &AnyUserData,
    value: Json,
    depth: usize,
    budget: &mut Budget,
) -> Checked<Value> {
    budget.value()?;
    Ok(match value {
        Json::Null => Value::UserData(null.clone()),
        Json::Bool(value) => Value::Boolean(value),
        Json::Number(value) => {
            // Check integer representations before converting, so 2^53 + 1 cannot
            // round down to an apparently acceptable double.
            if value
                .as_u64()
                .is_some_and(|value| value > MAX_SAFE_INTEGER_U64)
                || value
                    .as_i64()
                    .is_some_and(|value| value.unsigned_abs() > MAX_SAFE_INTEGER_U64)
            {
                return Err(Fault::new(
                    "number",
                    "JSON integers must be within ±(2^53 - 1)",
                ));
            }
            let number = value
                .as_f64()
                .ok_or_else(|| Fault::new("number", "JSON number is not representable"))?;
            Value::Number(check_number(number)?)
        }
        Json::String(value) => Value::String(lua.create_string(value)?),
        Json::Array(values) => {
            container_depth(depth)?;
            let table = lua.create_table()?;
            markers.raw_set(table.clone(), true)?;
            for (index, value) in values.into_iter().enumerate() {
                table.raw_set(
                    index + 1,
                    to_lua(lua, markers, null, value, depth + 1, budget)?,
                )?;
            }
            Value::Table(table)
        }
        Json::Object(values) => {
            container_depth(depth)?;
            let table = lua.create_table()?;
            markers.raw_set(table.clone(), false)?;
            for (key, value) in values {
                table.raw_set(key, to_lua(lua, markers, null, value, depth + 1, budget)?)?;
            }
            Value::Table(table)
        }
    })
}

fn decode(lua: &Lua, markers: &Table, null: &AnyUserData, value: Value) -> Checked<Value> {
    let Value::String(text) = value else {
        return Err(Fault::new("type", "decode expects a JSON string"));
    };
    if text.as_bytes().len() > MAX_BYTES {
        return Err(Fault::new("limit", "JSON input exceeds 1 MiB"));
    }
    let text = utf8(&text)?;
    let value =
        serde_json::from_str(&text).map_err(|error| Fault::new("json", error.to_string()))?;
    to_lua(lua, markers, null, value, 0, &mut Budget::default())
}

fn outcome(lua: &Lua, result: Checked<Value>) -> mlua::Result<(Value, Value)> {
    match result {
        Ok(value) => Ok((value, Value::Nil)),
        Err(error) => {
            let table = lua.create_table()?;
            table.raw_set("kind", error.kind)?;
            table.raw_set("message", error.message)?;
            Ok((Value::Nil, Value::Table(table)))
        }
    }
}

fn module(lua: &Lua) -> mlua::Result<Table> {
    let module = lua.create_table()?;
    let null = lua.create_userdata(Null)?;
    module.raw_set("null", null.clone())?;

    // Private weak keys avoid retaining otherwise dead decoded/marked tables.
    // No marker is placed in user-visible tables, and no user metatable is changed.
    let markers = lua.create_table()?;
    let metatable = lua.create_table()?;
    metatable.raw_set("__mode", "k")?;
    metatable.raw_set("__metatable", "protected JSON markers")?;
    metatable.set_readonly(true);
    markers.set_metatable(Some(metatable))?;

    for (name, is_array) in [("array", true), ("object", false)] {
        let markers = markers.clone();
        module.raw_set(
            name,
            lua.create_function(move |_, table: Table| {
                markers.raw_set(table.clone(), is_array)?;
                Ok(table)
            })?,
        )?;
    }
    let encode_markers = markers.clone();
    module.raw_set(
        "encode",
        lua.create_function(move |lua, value: Value| {
            outcome(lua, encode(lua, &encode_markers, value))
        })?,
    )?;
    module.raw_set(
        "decode",
        lua.create_function(move |lua, value: Value| {
            outcome(lua, decode(lua, &markers, &null, value))
        })?,
    )?;
    module.set_readonly(true);
    Ok(module)
}

pub(super) fn install(host: &rune_luau::Host<'_>) -> mlua::Result<()> {
    host.register_module("@pane/json", module(host.lua())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(source: &str) -> mlua::Result<()> {
        let lua = Lua::new();
        lua.globals().set("json", module(&lua)?)?;
        lua.load(source).exec()
    }

    #[test]
    fn roundtrip_nested_null_and_empty_containers() -> mlua::Result<()> {
        run(r#"
            local value, err = json.decode('{"items":[null,[],{"nested":null}],"empty":{}}')
            assert(err == nil and value.items[1] == json.null)
            assert(value.items[3].nested == json.null)
            assert(type(value.items[2]) == "table")
            assert(json.encode(value.items[2]) == "[]")
            assert(json.encode(value.empty) == "{}")
            local encoded, encodeErr = json.encode(value)
            assert(encodeErr == nil)
            assert(encoded == '{"empty":{},"items":[null,[],{"nested":null}]}')
            collectgarbage("collect")
            assert(json.encode(value.items[2]) == "[]")
        "#)
    }

    #[test]
    fn markers_and_plain_tables() -> mlua::Result<()> {
        run(r#"
            local t = {}
            assert(json.encode(t) == "{}")
            assert(json.array(t) == t and json.encode(t) == "[]")
            assert(json.object(t) == t and json.encode(t) == "{}")
            assert(json.encode({1, 2}) == "[1,2]")
            local mt = {__index = function() error("must not run") end}
            local marked = setmetatable({}, mt)
            assert(json.array(marked) == marked and getmetatable(marked) == mt)
            assert(json.encode(marked) == "[]")
            assert(not pcall(function() json.null = false end))
            assert(not pcall(function() setmetatable(json.null, {}) end))
            assert(type(getmetatable(json.null)) ~= "table")
        "#)
    }

    #[test]
    fn scalar_primitives() -> mlua::Result<()> {
        run(r#"
            for _, text in {"true", "false", "42", "-12.5", '"hello"', "null"} do
                local value, err = json.decode(text)
                assert(err == nil)
                local encoded, encodeErr = json.encode(value)
                assert(encodeErr == nil)
                local again, decodeErr = json.decode(encoded)
                assert(decodeErr == nil and again == value)
            end
            assert(json.decode("null") == json.null)
            assert(json.encode(nil) == "null")
            assert(json.encode(json.null) == "null")
            assert(json.decode('"\\u0000\\u00e9"') == "\0é")
        "#)
    }

    #[test]
    fn malformed_json_and_invalid_types_are_error_values() -> mlua::Result<()> {
        run(r#"
            for _, text in {"", "{", "[1,]", "null trailing", "NaN", '"\\ud800"'} do
                local value, err = json.decode(text)
                assert(value == nil and err.kind == "json" and #err.message > 0)
            end
            for _, value in {false, 12, {}, json.null} do
                local result, err = json.decode(value)
                assert(result == nil and err.kind == "type")
            end
            for _, value in {function() end, coroutine.create(function() end)} do
                local result, err = json.encode(value)
                assert(result == nil and err.kind == "type")
            end
        "#)
    }

    #[test]
    fn cycles_sparse_and_mixed_tables() -> mlua::Result<()> {
        run(r#"
            local cycle = {}; cycle.self = cycle
            local result, err = json.encode(cycle)
            assert(result == nil and err.kind == "cycle")
            local child = {ok = true}
            assert(json.encode({child, child}) == '[{"ok":true},{"ok":true}]')
            for _, value in {
                {[2] = true}, {[0] = true}, {[1.5] = true}, {[true] = true},
                {[1] = true, name = true}, json.array({name = true}), json.object({true}),
            } do
                local result, err = json.encode(value)
                assert(result == nil and err.kind == "table")
            end
        "#)
    }

    #[test]
    fn rejects_invalid_utf8_and_nonfinite_numbers() -> mlua::Result<()> {
        run(r#"
            local bad = string.char(255)
            for _, value in {bad, {[bad] = 1}} do
                local result, err = json.encode(value)
                assert(result == nil and err.kind == "utf8")
            end
            local result, err = json.decode('"' .. bad .. '"')
            assert(result == nil and err.kind == "utf8")
            for _, value in {0/0, math.huge, -math.huge} do
                local result, err = json.encode(value)
                assert(result == nil and err.kind == "number")
            end
            assert(select(2, json.decode("1e400")) ~= nil)
        "#)
    }

    #[test]
    fn safe_integer_policy() -> mlua::Result<()> {
        run(r#"
            for _, text in {"9007199254740991", "-9007199254740991", "0.125"} do
                local value, err = json.decode(text)
                assert(err == nil and value == tonumber(text))
                assert(select(2, json.encode(value)) == nil)
            end
            for _, text in {
                "9007199254740992", "9007199254740993", "-9007199254740993",
                "18446744073709551615", "18446744073709551616", "1e30",
            } do
                local value, err = json.decode(text)
                assert(value == nil and err.kind == "number")
                local encoded, encodeErr = json.encode(tonumber(text))
                assert(encoded == nil and encodeErr.kind == "number")
            end
        "#)
    }

    #[test]
    fn bounds_depth_bytes_and_values() -> mlua::Result<()> {
        run(r#"
            local value = true
            for _ = 1, 64 do value = {value} end
            assert(select(2, json.encode(value)) == nil)
            local result, err = json.encode({value})
            assert(result == nil and err.kind == "limit")
            result, err = json.decode(string.rep("[", 65) .. "0" .. string.rep("]", 65))
            assert(result == nil and err.kind == "limit")
            result, err = json.decode(string.rep(" ", 1024 * 1024 + 1))
            assert(result == nil and err.kind == "limit")
            result, err = json.encode(string.rep("a", 1024 * 1024))
            assert(result == nil and err.kind == "limit")
            result, err = json.encode(string.rep("\0", 200000))
            assert(result == nil and err.kind == "limit")
            result, err = json.encode(table.create(100000, true))
            assert(result == nil and err.kind == "limit")
            result, err = json.decode("[" .. string.rep("0,", 99999) .. "0]")
            assert(result == nil and err.kind == "limit")
        "#)
    }
}
