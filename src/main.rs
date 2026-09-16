// Thin CLI over the lineitems library: read comma-separated line items from
// a file or stdin, print each one priced out, and print a grand total.
// All the actual math lives in the library; this just wires it to text.

use std::env;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::process::ExitCode;

use lineitems::{LineItem, Money, Quantity};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();

    let input: Box<dyn BufRead> = match args.get(1) {
        Some(path) => match File::open(path) {
            Ok(f) => Box::new(BufReader::new(f)),
            Err(err) => {
                eprintln!("lineitems: cannot open {path}: {err}");
                return ExitCode::FAILURE;
            }
        },
        None => Box::new(BufReader::new(io::stdin())),
    };

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut grand_total = Money::from_cents(0);
    let mut had_error = false;

    for (line_no, line) in input.lines().enumerate() {
        let line = match line {
            Ok(l) => l,
            Err(err) => {
                eprintln!("lineitems: read error: {err}");
                had_error = true;
                continue;
            }
        };
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        match parse_line(line).and_then(|item| item.totals().map_err(|e| e.to_string()).map(|t| (item, t))) {
            Ok((item, totals)) => {
                match grand_total.checked_add(totals.total) {
                    Some(sum) => grand_total = sum,
                    None => {
                        eprintln!("lineitems: line {}: running total overflowed", line_no + 1);
                        had_error = true;
                        continue;
                    }
                }
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
            Err(err) => {
                eprintln!("lineitems: line {}: {err}", line_no + 1);
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

/// Parses a "description,quantity,unit_price,discount_bps,tax_bps" line.
fn parse_line(line: &str) -> Result<LineItem, String> {
    let fields: Vec<&str> = line.split(',').map(|f| f.trim()).collect();
    if fields.len() != 5 {
        return Err(format!(
            "expected 5 comma-separated fields (description,quantity,unit_price,discount_bps,tax_bps), got {}",
            fields.len()
        ));
    }

    let description = fields[0].to_string();
    let quantity: Quantity = fields[1].parse().map_err(|e: lineitems::ParseQuantityError| e.to_string())?;
    let unit_price: Money = fields[2].parse().map_err(|e: lineitems::ParseMoneyError| e.to_string())?;
    let discount_bps: u32 = fields[3]
        .parse()
        .map_err(|_| format!("invalid discount_bps {:?} (expected an integer)", fields[3]))?;
    let tax_bps: u32 = fields[4]
        .parse()
        .map_err(|_| format!("invalid tax_bps {:?} (expected an integer)", fields[4]))?;

    Ok(LineItem { description, quantity, unit_price, discount_bps, tax_bps })
}
