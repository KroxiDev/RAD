use serde_json::Value;
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    pub operation: String,
    pub arguments: Value,
    pub json: bool,
    pub wait_ms: Option<u64>,
}
#[derive(Debug, Clone, PartialEq)]
pub enum Invocation {
    Help,
    Command(Command),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidArguments;
/// The runtime's per-button action limit, checked here only to fail early.
const MAX_ACTIONS: usize = 20;
pub fn parse(args: &[String]) -> Result<Invocation, InvalidArguments> {
    if args.len() == 1 && args[0] == "--help" {
        return Ok(Invocation::Help);
    }
    let mut positional = Vec::new();
    let mut json = false;
    for arg in args {
        if arg == "--json" {
            if json {
                return Err(InvalidArguments);
            }
            json = true;
        } else {
            positional.push(arg.as_str());
        }
    }
    let mut wait_ms = None;
    let (operation, arguments) = match positional.as_slice() {
        ["status"] => ("status", serde_json::json!({})),
        ["get", id] if entity_id(id) => ("get", serde_json::json!({"id":id})),
        ["list", rest @ ..] => (
            "list",
            serde_json::json!({"cursor":cursor(rest)?,"limit":32}),
        ),
        ["plugin-configuration", id, rest @ ..] if entity_id(id) => (
            "plugin_configuration",
            serde_json::json!({"id":id,"cursor":cursor(rest)?}),
        ),
        ["plugin-catalog", rest @ ..] => (
            "plugin_catalog",
            serde_json::json!({"cursor":cursor(rest)?}),
        ),
        ["schema", kind, rest @ ..] if scalar(kind) => (
            "schema",
            serde_json::json!({"type":kind,"cursor":cursor(rest)?}),
        ),
        ["options", id, field, rest @ ..] if entity_id(id) && scalar(field) => (
            "options",
            options_arguments(serde_json::json!({"id":id,"field":field}), rest)?,
        ),
        ["options", "--type", kind, field, rest @ ..] if scalar(kind) && scalar(field) => (
            "options",
            options_arguments(serde_json::json!({"type":kind,"field":field}), rest)?,
        ),
        ["set", id, field, value] if entity_id(id) && !id.starts_with('f') && scalar(field) => {
            if value.len() > 65536 {
                return Err(InvalidArguments);
            }
            let value: Value =
                rad_wire::parse_json(value.as_bytes()).map_err(|_| InvalidArguments)?;
            (
                "edit",
                serde_json::json!({"changes":[{"kind":"set","id":id,"field":field,"value":value}]}),
            )
        }
        ["move", id, position] if entity_id(id) && id.starts_with('b') => {
            let position = decimal(position)
                .filter(|n| *n < 32)
                .ok_or(InvalidArguments)?;
            (
                "edit",
                serde_json::json!({"changes":[{"kind":"move","id":id,"position":position}]}),
            )
        }
        ["order", id, "--indices", indices] if entity_id(id) && id.starts_with('b') => {
            if indices.len() > 65536 {
                return Err(InvalidArguments);
            }
            let indices: Vec<u64> = serde_json::from_str(indices).map_err(|_| InvalidArguments)?;
            if indices.len() > MAX_ACTIONS {
                return Err(InvalidArguments);
            }
            let mut sorted = indices.clone();
            sorted.sort_unstable();
            if sorted.iter().copied().ne(0..indices.len() as u64) {
                return Err(InvalidArguments);
            }
            (
                "edit",
                serde_json::json!({"changes":[{"kind":"order","id":id,"indices":indices}]}),
            )
        }
        ["run", id, rest @ ..] if entity_id(id) && !id.starts_with('f') => {
            wait_ms = wait_options(rest)?;
            ("run", serde_json::json!({"id":id}))
        }
        ["result", id, rest @ ..] if execution_id(id) => {
            wait_ms = wait_options(rest)?;
            ("result", serde_json::json!({"execution_id":id}))
        }
        _ => return Err(InvalidArguments),
    };
    Ok(Invocation::Command(Command {
        operation: operation.into(),
        arguments,
        json,
        wait_ms,
    }))
}
fn cursor(args: &[&str]) -> Result<Value, InvalidArguments> {
    match args {
        [] => Ok(Value::Null),
        ["--cursor", value] if !value.is_empty() && value.len() <= 128 => {
            Ok(Value::String((*value).into()))
        }
        _ => Err(InvalidArguments),
    }
}
/// `[--cursor TOKEN]` and any number of `--with DEP=VALUE` in any order. The context
/// key is sent only when a draft value is given; the runtime validates its content.
fn options_arguments(mut arguments: Value, args: &[&str]) -> Result<Value, InvalidArguments> {
    let mut cursor = None;
    let mut context = serde_json::Map::new();
    let mut remaining = args;
    while let Some((flag, rest)) = remaining.split_first() {
        let (value, tail) = rest.split_first().ok_or(InvalidArguments)?;
        match *flag {
            "--cursor" if cursor.is_none() && !value.is_empty() && value.len() <= 128 => {
                cursor = Some(*value);
            }
            "--with" => {
                let (dependency, chosen) = value.split_once('=').ok_or(InvalidArguments)?;
                if !scalar(dependency)
                    || chosen.is_empty()
                    || context
                        .insert(dependency.into(), (*chosen).into())
                        .is_some()
                {
                    return Err(InvalidArguments);
                }
            }
            _ => return Err(InvalidArguments),
        }
        remaining = tail;
    }
    arguments["cursor"] = cursor.map_or(Value::Null, Value::from);
    if !context.is_empty() {
        arguments["context"] = Value::Object(context);
    }
    Ok(arguments)
}
fn scalar(value: &str) -> bool {
    !value.is_empty() && value.len() <= 16384 && !value.starts_with("--")
}
fn entity_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes
        .first()
        .is_some_and(|b| matches!(b, b'f' | b'b' | b'a'))
        && bytes.get(1).is_some_and(|b| matches!(b, b'1'..=b'9'))
        && bytes[1..].iter().all(u8::is_ascii_digit)
        && value[1..].parse::<u64>().is_ok()
}

fn decimal(value: &str) -> Option<u64> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    value.parse().ok()
}

fn wait_options(args: &[&str]) -> Result<Option<u64>, InvalidArguments> {
    let mut wait = false;
    let mut timeout = None;
    let mut remaining = args;
    while let Some((arg, rest)) = remaining.split_first() {
        match *arg {
            "--wait" if !wait => {
                wait = true;
                remaining = rest;
            }
            "--wait-timeout-ms" if timeout.is_none() => {
                let (value, tail) = rest.split_first().ok_or(InvalidArguments)?;
                timeout = Some(
                    decimal(value)
                        .filter(|n| (1..=3600000).contains(n))
                        .ok_or(InvalidArguments)?,
                );
                remaining = tail;
            }
            _ => return Err(InvalidArguments),
        }
    }
    if !wait && timeout.is_some() {
        return Err(InvalidArguments);
    }
    Ok(wait.then_some(timeout.unwrap_or(300000)))
}
pub(crate) fn execution_id(value: &str) -> bool {
    let Some((session, counter)) = value.split_once(':') else {
        return false;
    };
    session.len() == 32
        && session
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && session.bytes().any(|b| b != b'0')
        && decimal(counter).is_some_and(|n| n > 0)
}
