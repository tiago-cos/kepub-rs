#![allow(clippy::doc_markdown)]

//! Translates a Kobo location string (which koboSpan, and a character
//! offset into it) into an EPUB location string.
//!
//! Usage:
//!   cargo run --release --example kobo_to_epub -- <kepub.epub> <location>
//!
//! A Kobo location looks like OEBPS/chapter1.xhtml#kobo.4.2:12 — the
//! content document's path *inside* the archive, the koboSpan's id, and a
//! character offset into that specific span's own text, not the paragraph
//! as a whole. A span of N characters accepts offsets 0 through N.

use std::env;
use std::fs::File;
use std::process;

use kepub_rs::kobo_to_epub_location;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: cargo run --release --example kobo_to_epub -- <kepub.epub> <location>");
        eprintln!("  e.g. kobo_to_epub book.kepub.epub 'OEBPS/chapter-001.xhtml#kobo.4.2:12'");
        process::exit(1);
    }

    let kepub_path = &args[1];
    let location = &args[2];

    let input = open(kepub_path);

    println!("Kepub:         {kepub_path}");
    println!("Kobo location: {location}");
    println!();

    match kobo_to_epub_location(input, location) {
        Ok(epub) => println!("EPUB location: {epub}"),
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
