# kepub-rs

A Rust library for converting EPUB files into Kobo's KEPUB format, in parallel, and for translating reading positions between a simple structural location format and Kobo's span-based locations.

Conversion and location translation are independent: you can use either without the other.

## Features

- **EPUB → KEPUB conversion**, parallelized across content documents with [`rayon`](https://docs.rs/rayon).
- **Location translation** between a compact structural format and Kobo's `koboSpan`-based positions.
- **Location validation**, checking whether a location string actually resolves to a real character position, either against an unconverted EPUB or a converted kepub.

## Installation

```bash
cargo add kepub-rs
```

Or add it to `Cargo.toml` directly:

```toml
[dependencies]
kepub-rs = "0.1.0"
```

## Usage

### Converting an EPUB to a KEPUB

```rust
use std::fs::File;
use kepub_rs::Converter;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = File::open("book.epub")?;
    let output = File::create("book.kepub.epub")?;

    Converter::default().convert(input, output)?;

    Ok(())
}
```

`Converter` also accepts in-memory buffers (anything implementing `Read + Seek` for input, `Write + Seek` for output), so it works just as well against a `Cursor<Vec<u8>>` if you're not going through the filesystem.

`Converter::default()` uses Kobo's default stylesheet and script and this crate's best-effort element classification and sentence-segmentation rules. Any of these can be overridden with struct update syntax.

```rust
use kepub_rs::{Converter, ElementKind};

fn my_classify(local_name: &str) -> ElementKind {
    match local_name {
        // Treat <aside> like a paragraph boundary, in addition to the defaults.
        "aside" => ElementKind::ParagraphBoundary,
        _ => kepub_rs::default_classify(local_name),
    }
}

let converter = Converter {
    css_contents: "/* my custom stylesheet */",
    classify: my_classify,
    ..Converter::default()
};
```

### Translating between EPUB and Kobo locations

An EPUB location names a single character position inside an EPUB, independently of whether the book has been converted to a kepub yet:

```text
OEBPS/chapter-001.xhtml#0/2/t1:44
```

This reads as: content document `OEBPS/chapter-001.xhtml`, descend to the first element under `<body>` (`0`), then to its third child element (`2`), then take the second text run in that element (`t1`, 0-based), character offset 44.

A Kobo location names the same kind of position in terms of Kobo's own `koboSpan` markup:

```text
OEBPS/chapter-001.xhtml#kobo.4.2:12
```

This reads as: content document `OEBPS/chapter-001.xhtml`, the koboSpan with id `kobo.4.2`, character offset 12 into that span's own text.

```rust
use std::fs::File;
use kepub_rs::{kobo_to_epub_location, epub_to_kobo_location};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Kobo device reports a position as a koboSpan and an offset into it.
    let kobo_location = "OEBPS/chapter-001.xhtml#kobo.4.2:12";

    // Translate it into the crate's own structural format...
    let epub_location = kobo_to_epub_location(File::open("book.kepub.epub")?, kobo_location)?;
    println!("{epub_location}"); // e.g. "OEBPS/chapter-001.xhtml#0/2/t1:44"

    // ...and translate back.
    let round_tripped = epub_to_kobo_location(File::open("book.kepub.epub")?, &epub_location)?;
    assert_eq!(round_tripped, kobo_location);

    Ok(())
}
```

### Validating a location

Both location formats can be checked for validity without translating them.

```rust
use std::fs::File;
use kepub_rs::validate_epub_location;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let location = "OEBPS/chapter-001.xhtml#0/2/t1:44";

    match validate_epub_location(File::open("book.epub")?, location) {
        Ok(()) => println!("still valid"),
        Err(e) => println!("no longer valid: {e}"),
    }

    Ok(())
}
```

`validate_epub_location` works against both an unconverted EPUB and a converted kepub, since it walks the document's logical structure rather than looking up `koboSpan` ids. `validate_kobo_location` is the kepub-only counterpart, for checking a Kobo location string directly.

## License
 
Licensed under the [MIT license](LICENSE).
