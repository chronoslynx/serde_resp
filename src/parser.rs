use nom::{
    branch::alt,
    bytes::streaming::{tag, take_until},
    character::streaming::crlf,
    combinator::map_res,
    sequence::{preceded, tuple},
    IResult,
};
use std::str;

#[derive(Debug)]
pub enum Type<'a> {
    SimpleStr(&'a [u8]),
    Error(&'a str),
    Integer(i64),
    BulkString(&'a [u8]),
    Array(Vec<Type<'a>>),
}

fn until_crlf(input: &[u8]) -> IResult<&[u8], &[u8]> {
    let (remaining, (line, _)) = tuple((take_until("\r\n"), crlf))(input)?;
    Ok((remaining, line))
}

pub fn simple_str<'a>(input: &'a [u8]) -> IResult<&[u8], Type<'a>> {
    let (remaining, line) = preceded(tag("+"), until_crlf)(input)?;
    Ok((remaining, Type::SimpleStr(line)))
}

pub fn error(input: &[u8]) -> IResult<&[u8], Type> {
    let (remaining, err) =
        map_res(preceded(tag("-"), until_crlf), |e: &[u8]| str::from_utf8(e))(input)?;
    Ok((remaining, Type::Error(err)))
}

// TODO errors?
pub fn parse(input: &[u8]) -> IResult<&[u8], Type> {
    alt((simple_str, error))(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_str_ok() -> Result<(), String> {
        let (_, parsed) = simple_str(b"+OK\r\n").map_err(|e| e.to_string())?;
        match parsed {
            Type::SimpleStr(b"OK") => Ok(()),
            _ => Err(format!("expected SimpleStr('OK'), not {:?}", parsed)),
        }
    }

    #[test]
    fn parse_error_ok() -> Result<(), String> {
        let (_, parsed) = error(b"-Error oh no\r\n").map_err(|e| e.to_string())?;
        match parsed {
            Type::Error("Error oh no") => Ok(()),
            _ => Err(format!("expected Error('Error oh no'), not {:?}", parsed)),
        }
    }

    #[test]
    fn parse_error_not_str() -> Result<(), String> {
        match error(b"+Error oh no\r\n").map_err(|e| e.to_string()) {
            Err(_) => Ok(()),
            Ok((_, parsed)) => Err(format!("expected an error, not {:?}", parsed)),
        }
    }

    #[test]
    fn parse_parses_simple_strs() -> Result<(), String> {
        match parse(b"+Simplest of strings\r\n").map_err(|e| e.to_string())? {
            (_, Type::SimpleStr(b"Simplest of strings")) => Ok(()),
            (_, parsed) => Err(format!(
                "expected SimpleStr('Simplest of strings'), not {:?}",
                parsed,
            )),
        }
    }
    #[test]
    fn parse_parses_errors() -> Result<(), String> {
        match parse(b"-Oops\r\n").map_err(|e| e.to_string())? {
            (_, Type::Error("Oops")) => Ok(()),
            (_, parsed) => Err(format!("expected Error('Oops'), not {:?}", parsed,)),
        }
    }
}
