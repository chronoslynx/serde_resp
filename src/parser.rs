//! A nom-based parser for the [REdis Serialization Protocol](https://redis.io/topics/protocol).
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

/// Possible parsing error conditions
#[derive(Debug, Error)]
pub enum Error<I: fmt::Debug> {
    /// A bulk string of larger than 512MB was encountered.
    #[error("invalid bulk: {0}")]
    BulkTooLarge(String),
    /// An invalid string was encountered when parsing a [`Type::Integer`]
    #[error(transparent)]
    InvalidInteger {
        #[from]
        source: num::ParseIntError,
    },
    /// An invalid string was encountered when parsing a [`Type::Error`]
    #[error(transparent)]
    InvalidStr {
        #[from]
        source: str::Utf8Error,
    },
    /// A generic error from nom, our parsing library.
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

/// RESP data types.
#[derive(Debug, PartialEq, Clone)]
pub enum Type<'a> {
    Simple(&'a [u8]),
    Error(&'a str),
    Integer(i64),
    /// Bulk strings can hold up to 512MB of binary data.
    Bulk {
        len: u32,
        data: &'a [u8],
    },
    Array(Vec<Type<'a>>),
    Null,
}

fn to_str(input: &[u8]) -> StdResult<&str, Error<&u8>> {
    Ok(str::from_utf8(input)?)
}

fn to_i64(input: &str) -> StdResult<i64, Error<&str>> {
    Ok(input.parse::<i64>()?)
}

/// Parse
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
    map(prefixed_line(b"+"), |u: &[u8]| Type::Simple(u))(input)
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
        Err(Err::Error(Error::BulkTooLarge(format!(
            "length of {} is greater than the max of {}",
            len, BULK_STRING_MAX
        ))))
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

/// Attempt to parse an RESP [`Type`][Type] from the provided buffer.
///
/// ```rust
/// # use std::error::Error;
/// # use serde_resp::parser;
/// #
/// # fn main() -> Result<(), Box<dyn Error>> {
/// let (_, data) = parser::parse(b"+OK\r\n")?;
/// assert_eq!(data, parser::Type::Simple(b"OK"));
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// If this function requires more data than is available in the input buffer, it will
/// return an error containing [`nom::Err::Incomplete`] which contains the amount
/// necessary to complete parsing. When this occurrs, that data is needed at the end of
/// the provided input buffer, not instead of it:
///
/// ```rust
/// # use std::error::Error;
/// # use serde_resp::parser;
/// # use nom;
/// let result = parser::parse(b"+OK");
/// assert!(result.is_err());
/// assert!(result.err().unwrap().is_incomplete());
///
/// // This won't fix it as the parser is stateless
/// // It will fail will a different error now, as this is fundamentally
/// // not a valid RESP message
/// let result = parser::parse(b"\r\n");
/// assert!(result.is_err());
/// ```
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
            Type::Simple(b"OK") => Ok(()),
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
        let expected: Vec<Type> = vec![Type::Simple(b"OK"), Type::Integer(12)];
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
            (_, Type::Simple(b"Simplest of strings")) => Ok(()),
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

    #[test]
    fn parse_incomplete() -> TestResult {
        let result = parse(b"+OK");
        match result {
            Err(nom::Err::Incomplete(_)) => Ok(()),
            _ => Err(format!("unexpected {:?}", result)),
        }
    }
}
