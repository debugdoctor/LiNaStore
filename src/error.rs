use std::error::Error as StdError;
use std::fmt::{Display, Formatter};

pub type Error = Box<dyn StdError + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
struct SimpleError(String);

impl Display for SimpleError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl StdError for SimpleError {}

pub fn err_msg(msg: impl Into<String>) -> Error {
    Box::new(SimpleError(msg.into()))
}

pub trait Context<T> {
    fn context(self, msg: &'static str) -> Result<T>;
    fn with_context<F>(self, f: F) -> Result<T>
    where
        F: FnOnce() -> String;
}

impl<T, E> Context<T> for std::result::Result<T, E>
where
    E: StdError + Send + Sync + 'static,
{
    fn context(self, msg: &'static str) -> Result<T> {
        self.map_err(|e| err_msg(format!("{msg}: {e}")))
    }

    fn with_context<F>(self, f: F) -> Result<T>
    where
        F: FnOnce() -> String,
    {
        self.map_err(|e| err_msg(format!("{}: {e}", f())))
    }
}

impl<T> Context<T> for Option<T> {
    fn context(self, msg: &'static str) -> Result<T> {
        self.ok_or_else(|| err_msg(msg))
    }

    fn with_context<F>(self, f: F) -> Result<T>
    where
        F: FnOnce() -> String,
    {
        self.ok_or_else(|| err_msg(f()))
    }
}
