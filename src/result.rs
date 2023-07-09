pub type Result<T> = std::result::Result<T, Error>;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("{0}")]
    NixError(#[from] nix::Error),

    #[error("{0}")]
    IoError(#[from] std::io::Error),
}
