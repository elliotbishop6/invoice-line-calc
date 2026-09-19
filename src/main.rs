// Thin CLI over the lineitems library: read a headered CSV invoice from a
// file or stdin, print each line item priced out, and print a grand total.
// All the actual math and parsing lives in the library; this just wires it
// to text.

use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::process::ExitCode;

use lineitems::{csv, Money};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();

    let input = match args.get(1) {
        Some(path) => match fs::read_to_string(path) {
            Ok(s) => s,
            Err(err) => {
                eprintln!("lineitems: cannot open {path}: {err}");
                return ExitCode::FAILURE;
            }
        },
        None => {
            let mut buf = String::new();
            if let Err(err) = io::stdin().read_to_string(&mut buf) {
                eprintln!("lineitems: read error: {err}");
                return ExitCode::FAILURE;
            }
            buf
        }
    };

    let items = match csv::parse(&input) {
        Ok(items) => items,
        Err(err) => {
            eprintln!("lineitems: {err}");
            return ExitCode::FAILURE;
        }
    };

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut grand_total = Money::from_cents(0);
    let mut had_error = false;

    for item in &items {
        match item.totals() {
            Ok(totals) => match grand_total.checked_add(totals.total) {
                Some(sum) => {
                    grand_total = sum;
                    let _ = writeln!(
                        out,
                        "{:<28} qty {:>10}  subtotal {:>10}  discount {:>10}  tax {:>10}  total {:>10}",
                        item.description,
                        item.quantity.to_string(),
                        totals.subtotal.to_string(),
                        totals.discount.to_string(),
                        totals.tax.to_string(),
                        totals.total.to_string(),
                    );
                }
                None => {
                    eprintln!("lineitems: {}: running total overflowed", item.description);
                    had_error = true;
                }
            },
            Err(err) => {
                eprintln!("lineitems: {}: {err}", item.description);
                had_error = true;
            }
        }
    }

    let _ = writeln!(out, "{:<28} {:>68}", "grand total", grand_total.to_string());

    if had_error {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
