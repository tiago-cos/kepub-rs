#![allow(clippy::doc_markdown)]

//! Translates an EPUB location string into a Kobo location string (which
//! koboSpan, and a character offset into it).
//!
//! Usage:
//!   cargo run --release --example epub_to_kobo -- <kepub.epub> <location>
//!
//! The location string carries its own content document path, so there's
//! nothing else to pass alongside it:
//!   cargo run --release --example epub_to_kobo -- book.kepub.epub 'OEBPS/chapter-001.xhtml#0/2/t1:44'

use std::env;
use std::fs::File;
use std::process;

use kepub_rs::epub_to_kobo_location;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: cargo run --release --example epub_to_kobo -- <kepub.epub> <location>");
        eprintln!("  e.g. epub_to_kobo book.kepub.epub 'OEBPS/chapter-001.xhtml#0/2/t1:44'");
        process::exit(1);
    }

    let kepub_path = &args[1];
    let location = &args[2];

    let input = open(kepub_path);

    println!("Kepub:         {kepub_path}");
    println!("EPUB location: {location}");
    println!();

    match epub_to_kobo_location(input, location) {
        Ok(kobo) => println!("Kobo location: {kobo}"),
        Err(e) => {
            eprintln!("Error: translation failed: {e}");
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
