#![allow(clippy::doc_markdown)]

//! Checks whether an EPUB location string addresses a real character
//! position — without translating it.
//!
//! Works against **either** an unconverted EPUB or a converted kepub: the
//! same location string should validate against both, since it's counted
//! against the document's structure before koboSpans exist.
//!
//! Usage:
//!   cargo run --release --example validate_epub_location -- <epub_or_kepub.epub> <location>
//!
//! Exits 0 if valid, 1 otherwise (including if the location is well-formed
//! but doesn't resolve — see the printed reason either way).

use std::env;
use std::fs::File;
use std::process;

use kepub_rs::validate_epub_location;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!(
            "Usage: cargo run --release --example validate_epub_location -- <epub_or_kepub.epub> <location>"
        );
        eprintln!("  e.g. validate_epub_location book.epub 'OEBPS/chapter-001.xhtml#0/2/t1:44'");
        process::exit(1);
    }

    let archive_path = &args[1];
    let location = &args[2];

    let input = open(archive_path);

    println!("Archive:       {archive_path}");
    println!("EPUB location: {location}");
    println!();

    match validate_epub_location(input, location) {
        Ok(()) => println!("VALID"),
        Err(e) => {
            println!("INVALID: {e}");
            process::exit(1);
        }
    }
}

/// Opens an existing EPUB or kepub, exiting with a clear message on
/// failure rather than propagating a raw error up through main.
fn open(path: &str) -> File {
    File::open(path).unwrap_or_else(|e| {
        eprintln!("Error: Failed to open '{path}': {e}");
        process::exit(1)
    })
}
