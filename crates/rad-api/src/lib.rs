//! Versioned, I/O-free client contract. Domain validation belongs to the authority.
pub const VERSION: u8 = 1;
pub const CLI_VERSION: u8 = 1;
mod request;
mod strict;
pub use request::*;
mod views;
pub use views::*;
mod notice;
pub use notice::*;
mod error;
pub use error::*;

pub mod response;
mod response_data;
pub use response::{Error, Outcome, Response, Status};
pub use response_data::*;
