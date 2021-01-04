use std;
use std::fmt::{self, Display};

use serde::{de, ser};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    Message(String),
    InvalidInteger,
    TooMuchBulk,
    UnsupportedType,
    UnknownLength,
}

impl ser::Error for Error {
    fn custom<T: Display>(msg: T) -> Self {
        Error::Message(msg.to_string())
    }
}

impl de::Error for Error {
    fn custom<T: Display>(msg: T) -> Self {
        Error::Message(msg.to_string())
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Message(msg) => f.write_str(msg),
            Error::InvalidInteger => f.write_str("redis only supports 64-bit signed integers"), /* and so forth */
            Error::TooMuchBulk => f.write_str("bulk strings support at most 512MB"),
            Error::UnsupportedType => f.write_str(
                "RESP only supports null, 64-bit signed integers, strings, and binary data",
            ),
            Error::UnknownLength => {
                f.write_str("RESP sequences must know their length ahead of time")
            }
        }
    }
}

impl std::error::Error for Error {}
