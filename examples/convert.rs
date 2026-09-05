//! Converts an EPUB to a KEPUB.
//!
//! Usage:
//!   cargo run --release --example convert -- <input.epub> <output.kepub.epub>

use std::env;
use std::fs::File;
use std::process;
use std::time::Instant;

use kepub_rs::Converter;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!(
            "Usage: cargo run --release --example convert -- <input.epub> <output.kepub.epub>"
        );
        process::exit(1);
    }
    let input_path = &args[1];
    let output_path = &args[2];

    println!("Starting conversion...");
    println!("Input:  {input_path}");
    println!("Output: {output_path}");

    let start_time = Instant::now();

    let input_file = File::open(input_path).unwrap_or_else(|e| {
        eprintln!("Error: Failed to open input file '{input_path}': {e}");
        process::exit(1)
    });
    let mut output_file = File::create(output_path).unwrap_or_else(|e| {
        eprintln!("Error: Failed to create output file '{output_path}': {e}");
        process::exit(1)
    });

    let converter = Converter::default();
    match converter.convert(input_file, &mut output_file) {
        Ok(()) => {
            let duration = start_time.elapsed();
            println!("\nConversion successful in {duration:.2?}!");
        }
        Err(e) => {
            eprintln!("\nConversion failed: {e}");
            process::exit(1);
        }
    }
}
