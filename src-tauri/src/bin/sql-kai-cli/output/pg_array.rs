use std::iter::Peekable;
use std::str::Chars;

use serde_json::Value;
use sql_kai_lib::db::Type;

pub(super) fn decode(text: &str, element: &Type) -> Option<Value> {
    let body = if text.starts_with('[') {
        let (mut bounds, body) = text.split_once('=')?;
        while let Some(rest) = bounds.strip_prefix('[') {
            let (bound, rest) = rest.split_once(']')?;
            let (lower, upper) = bound.split_once(':')?;
            lower.parse::<i32>().ok()?;
            upper.parse::<i32>().ok()?;
            bounds = rest;
        }
        if !bounds.is_empty() {
            return None;
        }
        body
    } else {
        text
    };
    // box is the only built-in element type whose typdelim is not a comma.
    let delimiter = if *element == Type::BOX { ';' } else { ',' };
    let mut chars = body.chars().peekable();
    let value = array(&mut chars, element, delimiter, 0)?;
    chars.next().is_none().then_some(value)
}

fn array(
    chars: &mut Peekable<Chars<'_>>,
    element: &Type,
    delimiter: char,
    depth: u8,
) -> Option<Value> {
    if depth >= 6 || chars.next()? != '{' {
        return None;
    }
    let mut values = Vec::new();
    if chars.peek() == Some(&'}') {
        chars.next();
        return Some(Value::Array(values));
    }
    loop {
        let value = if chars.peek() == Some(&'{') {
            array(chars, element, delimiter, depth + 1)?
        } else {
            let quoted = chars.peek() == Some(&'"');
            if quoted {
                chars.next();
            }
            let mut text = String::new();
            let mut escaped = false;
            loop {
                let ch = *chars.peek()?;
                if quoted && ch == '"' {
                    chars.next();
                    break;
                }
                if !quoted && (ch == delimiter || ch == '}') {
                    break;
                }
                chars.next();
                match ch {
                    '\\' => {
                        escaped = true;
                        text.push(chars.next()?);
                    }
                    '{' | '"' if !quoted => return None,
                    _ => text.push(ch),
                }
            }
            if !quoted && text.is_empty() {
                return None;
            }
            if !quoted && !escaped && text == "NULL" {
                Value::Null
            } else {
                super::typed_value(&text, element)
            }
        };
        values.push(value);
        match chars.next()? {
            '}' => return Some(Value::Array(values)),
            ch if ch == delimiter => {}
            _ => return None,
        }
    }
}
