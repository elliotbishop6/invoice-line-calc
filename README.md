# lineitems

A small Rust library for pricing invoice line items, plus a thin CLI that
reads a list of them and prints totals.

## The problem

Given a quantity, a unit price, a discount, and a tax rate, most people
reach for `f64` and multiply things together. That works until you hit a
line like 2.5 hours at $33.33/hr with an 8.25% discount, and the total is
off by a cent from what the invoice PDF shows, and now someone in
accounting is filing a bug. Floating point can't represent most decimal
fractions exactly, and rounding errors compound across a multi-line
invoice.

This library uses fixed-point integers instead: money is stored as cents,
quantities as thousandths of a unit. Discount is applied to the subtotal,
then tax is applied to what's left, with each step rounding independently
to the nearest cent (exact halves round away from zero, so 1.5 cents
becomes 2 and -1.5 cents becomes -2). That matches how most invoicing
systems round in practice and keeps a negative quantity (a return or
credit memo) behaving symmetrically with a positive one.

## Library usage

```rust
use lineitems::{LineItem, Money, Quantity};

let item = LineItem {
    description: "Consulting, on-site".to_string(),
    quantity: "2.5".parse::<Quantity>().unwrap(),   // 2.5 hours
    unit_price: "150.00".parse::<Money>().unwrap(), // $150.00/hr
    discount_bps: 1000,                             // 10% off
    tax_bps: 825,                                   // 8.25% sales tax
};

let totals = item.totals().unwrap();
assert_eq!(totals.subtotal.to_string(), "375.00");
assert_eq!(totals.discount.to_string(), "37.50");
assert_eq!(totals.tax.to_string(), "27.84"); // 8.25% of (375.00 - 37.50)
assert_eq!(totals.total.to_string(), "365.34");
```

`totals()` returns a `Result` rather than panicking: an out-of-range
discount (`discount_bps > 10_000`) or a multiplication that would overflow
an `i64` cent count comes back as a `LineItemError` instead of silently
producing a wrong number.

## CLI usage

The CLI reads an invoice from a file argument or stdin, as either headered
CSV or JSON. A `.json` file extension selects the JSON parser; anything
else is read as CSV, unless the input (including stdin) starts with `[`.

CSV: the header names the columns present — `description`, `quantity`,
and `unit_price` are required; `discount_bps` and `tax_bps` are optional
and default to 0 when omitted, and columns can appear in any order. Blank
lines and `#`-comments are skipped anywhere in the file, including before
the header. A field can be double-quoted to contain a comma (`""` inside
a quoted field is a literal quote):

```
# invoice.csv
description,quantity,unit_price,discount_bps,tax_bps
"Consulting, on-site",2.5,150.00,1000,825
Widget,10,4.99,0,825
Returned widget,-2,4.99,0,825
```

```
$ cargo run -- invoice.csv
Consulting, on-site          qty      2.500  subtotal     375.00  discount      37.50  tax      27.84  total     365.34
Widget                       qty     10.000  subtotal      49.90  discount       0.00  tax       4.12  total      54.02
Returned widget               qty     -2.000  subtotal     -9.98  discount       0.00  tax      -0.82  total     -10.80
grand total                                                                                                    408.56
```

JSON: a top-level array of objects, one per line item, with the same
fields as the CSV columns. `quantity` and `unit_price` are given as JSON
strings rather than numbers — a JSON number is a float, and floats are
exactly what this library avoids. `discount_bps` and `tax_bps` are plain
integers and default to 0 when omitted:

```
# invoice.json
[
  {"description": "Consulting, on-site", "quantity": "2.5", "unit_price": "150.00", "discount_bps": 1000, "tax_bps": 825},
  {"description": "Widget", "quantity": "10", "unit_price": "4.99", "tax_bps": 825}
]
```

```
$ cargo run -- invoice.json
Consulting, on-site          qty      2.500  subtotal     375.00  discount      37.50  tax      27.84  total     365.34
Widget                       qty     10.000  subtotal      49.90  discount       0.00  tax       4.12  total      54.02
grand total                                                                                                    419.36
```

## Status

This is a first pass: the core pricing math, the CSV and JSON parsers, and
table-driven test suites covering the rounding and parsing edge cases live
in `src/lib.rs`. The JSON parser is hand-rolled rather than pulling in
serde — reading five fields off an array of objects doesn't justify a
dependency. The CLI is deliberately minimal. No third-party dependencies
— standard library only.

Not done yet: multiple tax rates per line, a half-even rounding mode, and
a JSON output mode for the CLI.

## License

MIT, see `LICENSE`.
