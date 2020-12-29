use nom::{
    branch::alt,
    bytes::streaming::{tag, take_until},
    character::streaming::crlf,
    combinator::{map, map_res},
    error::{ErrorKind, FromExternalError, ParseError},
    sequence::{preceded, tuple},
    Err, IResult,
};
use std::fmt;
use std::num;
use std::result::Result as StdResult;
use std::str;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error<I: fmt::Debug> {
    #[error("invalid {value_type}: {reason}")]
    InvalidValue { value_type: String, reason: String },
    #[error(transparent)]
    InvalidInteger {
        #[from]
        source: num::ParseIntError,
    },
    #[error(transparent)]
    InvalidStr {
        #[from]
        source: str::Utf8Error,
    },
    #[error("error {kind:?} at {input:?}")]
    Nom { kind: ErrorKind, input: I },
}

impl<I: fmt::Debug> ParseError<I> for Error<I> {
    fn from_error_kind(input: I, kind: ErrorKind) -> Self {
        Error::Nom { input, kind }
    }

    fn append(_: I, _: ErrorKind, other: Self) -> Self {
        other
    }
}

impl<I: fmt::Debug, E> FromExternalError<I, E> for Error<I> {
    fn from_external_error(input: I, kind: ErrorKind, _e: E) -> Self {
        Error::Nom { kind, input }
    }
}

type Result<I, O> = IResult<I, O, Error<I>>;

// The maximum size of a bulk string is 512MB.
const BULK_STRING_MAX: i64 = 4_096_000_000;
// A bulk string length of -1 indicates a nil value.
const NULL_SENTINEL: i64 = -1;

#[derive(Debug, PartialEq, Clone)]
pub enum Type<'a> {
    SimpleStr(&'a [u8]),
    Error(&'a str),
    Integer(i64),
    Bulk { len: u32, data: &'a [u8] },
    Array(Vec<Type<'a>>),
    Null,
}

fn to_str(input: &[u8]) -> StdResult<&str, Error<&u8>> {
    Ok(str::from_utf8(input)?)
}

fn to_i64(input: &str) -> StdResult<i64, Error<&str>> {
    Ok(input.parse::<i64>()?)
}

fn until_crlf(input: &[u8]) -> Result<&[u8], &[u8]> {
    let (remaining, (line, _)) = tuple((take_until("\r\n"), crlf))(input)?;
    Ok((remaining, line))
}

fn prefixed_line<'a>(prefix: &'a [u8]) -> impl Fn(&[u8]) -> Result<&[u8], &[u8]> + 'a {
    move |input: &[u8]| {
        let t = prefix.clone();
        preceded(tag(t), until_crlf)(input)
    }
}

fn simple_str<'a>(input: &'a [u8]) -> Result<&[u8], Type<'a>> {
    map(prefixed_line(b"+"), |u: &[u8]| Type::SimpleStr(u))(input)
}

fn error(input: &[u8]) -> Result<&[u8], Type> {
    map(map_res(prefixed_line(b"-"), to_str), |s: &str| {
        Type::Error(s)
    })(input)
}

fn integer(input: &[u8]) -> Result<&[u8], Type> {
    map(
        map_res(map_res(prefixed_line(b":"), to_str), to_i64),
        |i: i64| Type::Integer(i),
    )(input)
}

fn bulk(input: &[u8]) -> Result<&[u8], Type> {
    let (remaining, len) = map_res(map_res(prefixed_line(b"$"), to_str), to_i64)(input)?;
    if len == NULL_SENTINEL {
        Ok((remaining, Type::Null))
    } else if len > BULK_STRING_MAX {
        Err(Err::Error(Error::InvalidValue {
            value_type: "bulk".to_string(),
            reason: format!(
                "length of {} is greater than the max of {}",
                len, BULK_STRING_MAX
            ),
        }))
    } else {
        let (remaining, data) = until_crlf(remaining)?;
        Ok((
            remaining,
            Type::Bulk {
                len: len as u32,
                data,
            },
        ))
    }
}

fn array(input: &[u8]) -> Result<&[u8], Type> {
    let (mut remaining, len) = map_res(map_res(prefixed_line(b"*"), to_str), to_i64)(input)?;
    if len == NULL_SENTINEL {
        Ok((remaining, Type::Null))
    } else {
        let mut data = Vec::with_capacity(len as usize);
        for i in 0..len {
            println!("reading element {}", i);
            let (now_remaining, elem) = parse(remaining)?;
            remaining = now_remaining;
            data.push(elem);
        }
        Ok((remaining, Type::Array(data)))
    }
}

pub fn parse(input: &[u8]) -> Result<&[u8], Type> {
    alt((simple_str, error, integer, bulk, array))(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    type TestResult = StdResult<(), String>;

    #[test]
    fn parse_simple_str_ok() -> TestResult {
        let (_, parsed) = simple_str(b"+OK\r\n").map_err(|e| e.to_string())?;
        match parsed {
            Type::SimpleStr(b"OK") => Ok(()),
            _ => Err(format!("expected SimpleStr('OK'), not {:?}", parsed)),
        }
    }

    #[test]
    fn parse_error_ok() -> TestResult {
        let (_, parsed) = error(b"-Error oh no\r\n").map_err(|e| e.to_string())?;
        match parsed {
            Type::Error("Error oh no") => Ok(()),
            _ => Err(format!("expected Error('Error oh no'), not {:?}", parsed)),
        }
    }

    #[test]
    fn parse_error_not_str() -> TestResult {
        match error(b"+Error oh no\r\n").map_err(|e| e.to_string()) {
            Err(_) => Ok(()),
            Ok((_, parsed)) => Err(format!("expected an error, not {:?}", parsed)),
        }
    }

    #[test]
    fn parse_integer_ok() -> TestResult {
        let (_, parsed) = integer(b":-1\r\n").map_err(|e| e.to_string())?;
        match parsed {
            Type::Integer(-1) => Ok(()),
            _ => Err(format!("expected Integer(-1), not {:?}", parsed)),
        }
    }

    #[test]
    fn parse_integer_error_if_decimal() -> TestResult {
        let res = integer(b":1.1\r\n");
        match res {
            Err(_) => Ok(()),
            _ => Err(format!("expected failure, not {:?}", res)),
        }
    }

    #[test]
    fn parse_bulk_ok() -> TestResult {
        let (_, parsed) = bulk(b"$5\r\nhello\r\n").map_err(|e| e.to_string())?;
        match parsed {
            Type::Bulk {
                len: 5,
                data: b"hello",
            } => Ok(()),
            _ => Err(format!("expected Bulk(5, hello), not {:?}", parsed)),
        }
    }

    #[test]
    fn parse_bulk_null() -> TestResult {
        let (_, parsed) = bulk(b"$-1\r\n").map_err(|e| e.to_string())?;
        match parsed {
            Type::Null => Ok(()),
            _ => Err(format!("expected Null, not {:?}", parsed)),
        }
    }

    #[test]
    fn parse_array_ok() -> TestResult {
        let (_, parsed) = array(b"*2\r\n+OK\r\n:12\r\n").map_err(|e| e.to_string())?;
        let expected: Vec<Type> = vec![Type::SimpleStr(b"OK"), Type::Integer(12)];
        match parsed {
            Type::Array(data) => {
                let len = data.len();
                if data.len() != expected.len() {
                    return Err(format!("expected {:?}, got {:?}", expected, data));
                }
                let matching = data
                    .clone()
                    .into_iter()
                    .zip(expected.clone())
                    .filter(|(a, b)| a == b)
                    .count();
                if matching != len {
                    return Err(format!("expected {:?}, got {:?}", expected, data));
                }
                Ok(())
            }
            _ => Err(format!(
                "expected Array(String(OK),Integer(12)), not {:?}",
                parsed
            )),
        }
    }

    #[test]
    fn parse_array_null() -> TestResult {
        let (_, parsed) = array(b"*-1\r\n").map_err(|e| e.to_string())?;
        match parsed {
            Type::Null => Ok(()),
            _ => Err(format!("expected Null, not {:?}", parsed)),
        }
    }

    // TODO: write a macro to make listing examples easy
    #[test]
    fn parse_parses_simple_strs() -> TestResult {
        match parse(b"+Simplest of strings\r\n").map_err(|e| e.to_string())? {
            (_, Type::SimpleStr(b"Simplest of strings")) => Ok(()),
            (_, parsed) => Err(format!(
                "expected SimpleStr('Simplest of strings'), not {:?}",
                parsed,
            )),
        }
    }

    #[test]
    fn parse_parses_errors() -> TestResult {
        match parse(b"-Oops\r\n").map_err(|e| e.to_string())? {
            (_, Type::Error("Oops")) => Ok(()),
            (_, parsed) => Err(format!("expected Error('Oops'), not {:?}", parsed,)),
        }
    }

    #[test]
    fn parse_parses_integers() -> TestResult {
        match parse(b":1\r\n").map_err(|e| e.to_string())? {
            (_, Type::Integer(1)) => Ok(()),
            (_, parsed) => Err(format!("expected Integer(1), not {:?}", parsed,)),
        }
    }

    #[test]
    fn parse_parses_bulk() -> TestResult {
        match parse(b"$-1\r\n").map_err(|e| e.to_string())? {
            (_, Type::Null) => Ok(()),
            (_, parsed) => Err(format!("expected Null, not {:?}", parsed,)),
        }
    }
}
