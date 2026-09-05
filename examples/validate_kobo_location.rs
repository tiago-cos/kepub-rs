#![allow(clippy::doc_markdown)]

//! Checks whether a Kobo location string (which koboSpan, and a character
//! offset into it) addresses a real position in a kepub — without
//! translating it.
//!
//! Requires a **converted** kepub: an unconverted EPUB has no koboSpans,
//! so validation will always fail against one.
//!
//! Usage:
//!   cargo run --release --example validate_kobo_location -- <kepub.epub> <location>
//!
//! A Kobo location looks like OEBPS/chapter1.xhtml#kobo.4.2:12 — the
//! content document's path *inside* the archive, the koboSpan's id, and a
//! character offset into that specific span's own text. A span of N
//! characters accepts offsets 0 through N.
//!
//! Exits 0 if valid, 1 otherwise (including if the location is well-formed
//! but doesn't resolve — see the printed reason either way).

use std::env;
use std::fs::File;
use std::process;

use kepub_rs::validate_kobo_location;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!(
            "Usage: cargo run --release --example validate_kobo_location -- <kepub.epub> <location>"
        );
        eprintln!(
            "  e.g. validate_kobo_location book.kepub.epub 'OEBPS/chapter-001.xhtml#kobo.4.2:12'"
        );
        process::exit(1);
    }

    let kepub_path = &args[1];
    let location = &args[2];

    let input = open(kepub_path);

    println!("Kepub:         {kepub_path}");
    println!("Kobo location: {location}");
    println!();

    match validate_kobo_location(input, location) {
        Ok(()) => println!("VALID"),
        Err(e) => {
            println!("INVALID: {e}");
            process::exit(1);
        }
    }
}

/// Opens an existing kepub, exiting with a clear message on failure rather
/// than propagating a raw error up through main.
fn open(path: &str) -> File {
    File::open(path).unwrap_or_else(|e| {
        eprintln!("Error: Failed to open '{path}': {e}");
        process::exit(1)
    })
}
