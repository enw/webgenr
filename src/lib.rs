mod document;
pub use self::document::Document;

mod web;
mod util;
pub use self::web::Web;

extern crate pretty_env_logger;

#[macro_use]
extern crate log;
