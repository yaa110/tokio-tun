use std::fmt::Display;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    NixError(nix::Error),
    IoError(std::io::Error),
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NixError(err) => write!(f, "{err}"),
            Self::IoError(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Self::IoError(err)
    }
}

impl From<nix::Error> for Error {
    fn from(err: nix::Error) -> Self {
        Self::NixError(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_from_io_and_display() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "device not found");
        let err: Error = io_err.into();
        assert_eq!(format!("{err}"), "device not found");
    }

    #[test]
    fn test_error_from_nix_and_display() {
        let nix_err = nix::Error::from_raw(libc::EINVAL);
        let err: Error = nix_err.into();
        assert_eq!(format!("{err}"), "EINVAL: Invalid argument");
    }
}
