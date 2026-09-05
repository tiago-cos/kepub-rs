//! `kepub-rs`: converts EPUBs to Kobo's KEPUB format, and translates
//! reading positions between a simple structural location format and
//! Kobo's span-based locations.
//!
//! The crate has two independent halves, and neither needs the other:
//!
//! - **Conversion** — [`Converter`] reads an EPUB and writes a KEPUB,
//!   transforming spine content documents in parallel.
//! - **Location translation** — [`kobo_to_epub_location`] and
//!   [`epub_to_kobo_location`] convert between an EPUB location string
//!   (e.g. `OEBPS/chapter-001.xhtml#0/2/t1:44`) and a Kobo one (e.g.
//!   `OEBPS/chapter-001.xhtml#kobo.4.2:12`), with
//!   [`validate_epub_location`] and [`validate_kobo_location`] checking a
//!   location resolves without translating it. This is recomputed on
//!   demand from a finished kepub's own structure, so conversion persists
//!   no mapping data.

mod archive;
mod convert;
mod dom;
mod error;
mod location;

pub use convert::{Converter, ElementKind, default_classify, default_segment};
pub use error::KepubError;
pub use location::{
    epub_to_kobo_location, kobo_to_epub_location, validate_epub_location, validate_kobo_location,
};
