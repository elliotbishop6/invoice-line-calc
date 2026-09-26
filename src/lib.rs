// Money and quantities are fixed-point integers (cents, thousandths of a
// unit) rather than f64. An invoice that silently drifts by half a cent
// because of float rounding is a bug someone in accounting will find before
// we do.

use std::fmt;
use std::str::FromStr;

/// An amount of money, stored as integer cents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Money(i64);

impl Money {
    pub fn from_cents(cents: i64) -> Self {
        Money(cents)
    }

    pub fn cents(self) -> i64 {
        self.0
    }

    pub fn checked_add(self, other: Money) -> Option<Money> {
        self.0.checked_add(other.0).map(Money)
    }

    pub fn checked_sub(self, other: Money) -> Option<Money> {
        self.0.checked_sub(other.0).map(Money)
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs = self.0.unsigned_abs();
        write!(f, "{sign}{}.{:02}", abs / 100, abs % 100)
    }
}

impl FromStr for Money {
    type Err = ParseMoneyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_fixed_point(s, 2)
            .map(Money)
            .map_err(|()| ParseMoneyError(s.to_string()))
    }
}

/// A quantity of units, stored as thousandths (three decimal places), which
/// is enough for things like 2.5 hours or 0.125 kg without pulling in a
/// decimal library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Quantity(i64);

impl Quantity {
    pub fn from_thousandths(thousandths: i64) -> Self {
        Quantity(thousandths)
    }

    pub fn as_thousandths(self) -> i64 {
        self.0
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs = self.0.unsigned_abs();
        write!(f, "{sign}{}.{:03}", abs / 1000, abs % 1000)
    }
}

impl FromStr for Quantity {
    type Err = ParseQuantityError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_fixed_point(s, 3)
            .map(Quantity)
            .map_err(|()| ParseQuantityError(s.to_string()))
    }
}

/// One priced line on an invoice: some quantity of a thing, a unit price,
/// a discount, and zero or more tax rates. `discount_bps` and each entry
/// in `tax_bps` are basis points (1/100 of a percent), so 825 means 8.25%.
/// A line can carry more than one tax rate (state plus local sales tax,
/// say) because those are usually set independently and each rounds to
/// the nearest cent on its own rather than being pre-added into one rate;
/// two 0.5% rates on a $1.00 line each round 0.5 cents up to a full cent,
/// for two cents total, where a single pre-added 1% rate would round to
/// one cent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineItem {
    pub description: String,
    pub quantity: Quantity,
    pub unit_price: Money,
    pub discount_bps: u32,
    pub tax_bps: Vec<u32>,
}

/// The result of pricing a `LineItem`: subtotal, the discount taken off it,
/// tax charged on what's left, and the final total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineTotals {
    pub subtotal: Money,
    pub discount: Money,
    pub tax: Money,
    pub total: Money,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineItemError {
    Overflow,
    InvalidDiscount(u32),
}

impl fmt::Display for LineItemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LineItemError::Overflow => write!(f, "arithmetic overflow while computing line totals"),
            LineItemError::InvalidDiscount(bps) => {
                write!(f, "discount_bps {bps} exceeds 10000 (100%)")
            }
        }
    }
}

impl std::error::Error for LineItemError {}

impl LineItem {
    /// Computes subtotal, discount, tax, and total.
    ///
    /// Discount is taken off the subtotal first, then each tax rate is
    /// applied to what's left and summed; every step (the discount and
    /// each individual tax rate) rounds to the nearest cent independently,
    /// rounding exact halves away from zero. That means a return (negative
    /// quantity) rounds its subtotal down in magnitude the same way a sale
    /// rounds up, rather than always rounding toward positive infinity.
    pub fn totals(&self) -> Result<LineTotals, LineItemError> {
        if self.discount_bps > 10_000 {
            return Err(LineItemError::InvalidDiscount(self.discount_bps));
        }

        let subtotal = proportional(self.unit_price.cents(), self.quantity.as_thousandths(), 1000)?;
        let discount = proportional(subtotal.cents(), self.discount_bps as i64, 10_000)?;
        let after_discount = subtotal.checked_sub(discount).ok_or(LineItemError::Overflow)?;

        let mut tax = Money::from_cents(0);
        for &rate in &self.tax_bps {
            let rate_tax = proportional(after_discount.cents(), rate as i64, 10_000)?;
            tax = tax.checked_add(rate_tax).ok_or(LineItemError::Overflow)?;
        }

        let total = after_discount.checked_add(tax).ok_or(LineItemError::Overflow)?;

        Ok(LineTotals { subtotal, discount, tax, total })
    }
}

/// Computes `round_half_away_from_zero(amount_cents * multiplier / denominator)`
/// as a `Money`, widening to i128 so the intermediate product can't overflow
/// before rounding, then narrowing back and reporting overflow if the
/// result doesn't fit in a cent count we can actually represent.
fn proportional(amount_cents: i64, multiplier: i64, denominator: i64) -> Result<Money, LineItemError> {
    let product = (amount_cents as i128) * (multiplier as i128);
    let rounded = round_half_away_from_zero(product, denominator as i128);
    let cents = i64::try_from(rounded).map_err(|_| LineItemError::Overflow)?;
    Ok(Money::from_cents(cents))
}

fn round_half_away_from_zero(numerator: i128, denominator: i128) -> i128 {
    let half = denominator / 2;
    if numerator >= 0 {
        (numerator + half) / denominator
    } else {
        -((-numerator + half) / denominator)
    }
}

/// Parses a decimal string like "12.34" or "-0.5" into an integer scaled by
/// `10^decimals`. Rejects more fractional digits than `decimals` instead of
/// truncating, so a quantity like "1.2345" fails loudly rather than quietly
/// losing precision.
fn parse_fixed_point(s: &str, decimals: usize) -> Result<i64, ()> {
    let s = s.trim();
    if s.is_empty() {
        return Err(());
    }

    let (negative, rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };

    let mut parts = rest.splitn(2, '.');
    let whole = parts.next().unwrap();
    let frac = parts.next().unwrap_or("");

    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return Err(());
    }
    if frac.len() > decimals || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return Err(());
    }

    let whole_val: i64 = whole.parse().map_err(|_| ())?;
    let mut frac_val: i64 = if frac.is_empty() { 0 } else { frac.parse().map_err(|_| ())? };
    for _ in frac.len()..decimals {
        frac_val = frac_val.checked_mul(10).ok_or(())?;
    }

    let scale = 10i64.checked_pow(decimals as u32).ok_or(())?;
    let magnitude = whole_val
        .checked_mul(scale)
        .and_then(|v| v.checked_add(frac_val))
        .ok_or(())?;

    Ok(if negative { -magnitude } else { magnitude })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseMoneyError(String);

impl fmt::Display for ParseMoneyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid money value {:?} (expected e.g. \"12.34\" or \"-5\")", self.0)
    }
}

impl std::error::Error for ParseMoneyError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseQuantityError(String);

impl fmt::Display for ParseQuantityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid quantity {:?} (expected e.g. \"2.5\", up to 3 decimal places)",
            self.0
        )
    }
}

impl std::error::Error for ParseQuantityError {}

/// Parses a headered CSV invoice into `LineItem`s. Column order in the
/// header doesn't matter; `discount_bps` and `tax_bps` are optional and
/// default to no discount and no tax when the column is absent, since most
/// lines on a real invoice have neither. `tax_bps` holds one or more
/// semicolon-separated rates (`"500;300"` for a 5% and a 3% rate charged
/// separately), since a single column still has to fit one CSV field.
pub mod csv {
    use super::{LineItem, Money, Quantity};
    use std::fmt;

    const REQUIRED: [&str; 3] = ["description", "quantity", "unit_price"];
    const KNOWN: [&str; 5] = ["description", "quantity", "unit_price", "discount_bps", "tax_bps"];

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum CsvError {
        MissingHeader,
        MissingColumn(&'static str),
        UnknownColumn(String),
        WrongFieldCount { line: usize, expected: usize, got: usize },
        InvalidField { line: usize, column: &'static str, message: String },
    }

    impl fmt::Display for CsvError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                CsvError::MissingHeader => write!(f, "file has no header row"),
                CsvError::MissingColumn(name) => write!(f, "header is missing required column {name:?}"),
                CsvError::UnknownColumn(name) => write!(f, "header has unrecognized column {name:?}"),
                CsvError::WrongFieldCount { line, expected, got } => {
                    write!(f, "line {line}: expected {expected} fields (matching the header), got {got}")
                }
                CsvError::InvalidField { line, column, message } => {
                    write!(f, "line {line}: column {column:?}: {message}")
                }
            }
        }
    }

    impl std::error::Error for CsvError {}

    struct Columns {
        description: usize,
        quantity: usize,
        unit_price: usize,
        discount_bps: Option<usize>,
        tax_bps: Option<usize>,
    }

    impl Columns {
        fn from_header(header: &[String]) -> Result<Self, CsvError> {
            for name in header {
                if !KNOWN.contains(&name.as_str()) {
                    return Err(CsvError::UnknownColumn(name.clone()));
                }
            }

            let find = |name: &'static str| {
                header.iter().position(|h| h == name).ok_or(CsvError::MissingColumn(name))
            };

            Ok(Columns {
                description: find("description")?,
                quantity: find("quantity")?,
                unit_price: find("unit_price")?,
                discount_bps: header.iter().position(|h| h == "discount_bps"),
                tax_bps: header.iter().position(|h| h == "tax_bps"),
            })
        }
    }

    /// Parses a full CSV document: a header row followed by one line item
    /// per row. Blank lines and lines starting with `#` are skipped
    /// wherever they appear, including before the header.
    pub fn parse(input: &str) -> Result<Vec<LineItem>, CsvError> {
        let mut rows = input.lines().enumerate().filter(|(_, line)| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        });

        let (_, header_line) = rows.next().ok_or(CsvError::MissingHeader)?;
        let header: Vec<String> =
            split_line(header_line).into_iter().map(|f| f.to_ascii_lowercase()).collect();
        let columns = Columns::from_header(&header)?;

        let mut items = Vec::new();
        for (index, line) in rows {
            let line_no = index + 1;
            let fields = split_line(line);
            if fields.len() != header.len() {
                return Err(CsvError::WrongFieldCount { line: line_no, expected: header.len(), got: fields.len() });
            }

            let description = fields[columns.description].clone();

            let quantity: Quantity = fields[columns.quantity].parse().map_err(|e: super::ParseQuantityError| {
                CsvError::InvalidField { line: line_no, column: "quantity", message: e.to_string() }
            })?;

            let unit_price: Money = fields[columns.unit_price].parse().map_err(|e: super::ParseMoneyError| {
                CsvError::InvalidField { line: line_no, column: "unit_price", message: e.to_string() }
            })?;

            let discount_bps = match columns.discount_bps {
                Some(i) => fields[i].parse::<u32>().map_err(|_| CsvError::InvalidField {
                    line: line_no,
                    column: "discount_bps",
                    message: format!("invalid integer {:?}", fields[i]),
                })?,
                None => 0,
            };

            let tax_bps = match columns.tax_bps {
                Some(i) => parse_tax_rates(&fields[i]).map_err(|message| CsvError::InvalidField {
                    line: line_no,
                    column: "tax_bps",
                    message,
                })?,
                None => Vec::new(),
            };

            items.push(LineItem { description, quantity, unit_price, discount_bps, tax_bps });
        }

        Ok(items)
    }

    /// Parses a `tax_bps` field into zero or more basis-point rates. An
    /// empty field means no tax; multiple rates are joined with `;`.
    fn parse_tax_rates(field: &str) -> Result<Vec<u32>, String> {
        if field.is_empty() {
            return Ok(Vec::new());
        }
        field
            .split(';')
            .map(|part| part.trim().parse::<u32>().map_err(|_| format!("invalid integer {part:?}")))
            .collect()
    }

    /// Splits one CSV line into fields, honoring double-quoted fields (so a
    /// description can contain a comma) with `""` as an escaped quote.
    /// Unquoted fields are trimmed; quoted fields are taken verbatim so
    /// deliberate leading/trailing space survives.
    fn split_line(line: &str) -> Vec<String> {
        let mut fields = Vec::new();
        let mut field = String::new();
        let mut in_quotes = false;
        let mut quoted = false;
        let mut chars = line.chars().peekable();

        while let Some(c) = chars.next() {
            if in_quotes {
                if c == '"' {
                    if chars.peek() == Some(&'"') {
                        field.push('"');
                        chars.next();
                    } else {
                        in_quotes = false;
                    }
                } else {
                    field.push(c);
                }
            } else if c == '"' && field.is_empty() {
                in_quotes = true;
                quoted = true;
            } else if c == ',' {
                fields.push(if quoted { std::mem::take(&mut field) } else { field.trim().to_string() });
                field.clear();
                quoted = false;
            } else {
                field.push(c);
            }
        }
        fields.push(if quoted { field } else { field.trim().to_string() });

        fields
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_header_in_any_column_order() {
            let input = "unit_price,description,quantity\n10.00,Widget,2\n";
            let items = parse(input).unwrap();
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].description, "Widget");
            assert_eq!(items[0].quantity.to_string(), "2.000");
            assert_eq!(items[0].unit_price.to_string(), "10.00");
            assert_eq!(items[0].discount_bps, 0);
            assert_eq!(items[0].tax_bps, Vec::<u32>::new());
        }

        #[test]
        fn quoted_field_can_contain_a_comma() {
            let input = "description,quantity,unit_price\n\"Consulting, on-site\",1,50.00\n";
            let items = parse(input).unwrap();
            assert_eq!(items[0].description, "Consulting, on-site");
        }

        #[test]
        fn skips_blank_and_comment_lines_before_and_after_header() {
            let input = "# invoice\n\ndescription,quantity,unit_price\n\n# a widget\nWidget,1,5.00\n";
            let items = parse(input).unwrap();
            assert_eq!(items.len(), 1);
        }

        #[test]
        fn optional_columns_default_to_zero() {
            let input = "description,quantity,unit_price,tax_bps\nWidget,1,5.00,825\n";
            let items = parse(input).unwrap();
            assert_eq!(items[0].discount_bps, 0);
            assert_eq!(items[0].tax_bps, vec![825]);
        }

        #[test]
        fn multiple_tax_rates_are_semicolon_separated() {
            let input = "description,quantity,unit_price,tax_bps\nWidget,1,5.00,500;300\n";
            let items = parse(input).unwrap();
            assert_eq!(items[0].tax_bps, vec![500, 300]);
        }

        #[test]
        fn missing_required_column_is_an_error() {
            let input = "quantity,unit_price\n1,5.00\n";
            assert_eq!(parse(input), Err(CsvError::MissingColumn("description")));
        }

        #[test]
        fn unknown_column_is_an_error() {
            let input = "description,quantity,unit_price,sku\nWidget,1,5.00,ABC\n";
            assert_eq!(parse(input), Err(CsvError::UnknownColumn("sku".to_string())));
        }

        #[test]
        fn wrong_field_count_is_an_error() {
            let input = "description,quantity,unit_price\nWidget,1\n";
            assert_eq!(parse(input), Err(CsvError::WrongFieldCount { line: 2, expected: 3, got: 2 }));
        }

        #[test]
        fn empty_input_is_an_error() {
            assert_eq!(parse(""), Err(CsvError::MissingHeader));
        }
    }
}

/// Parses a JSON invoice into `LineItem`s: a top-level array of objects,
/// one per line. `quantity` and `unit_price` must be given as JSON
/// strings rather than numbers, for the same reason the CSV parser and the
/// rest of this crate avoid `f64`: a JSON number is a float, and floats
/// are exactly the representation this library exists to avoid.
/// `discount_bps` is a plain JSON integer and defaults to 0 when omitted.
/// `tax_bps` is either a single integer or an array of integers (for more
/// than one tax rate on the line) and defaults to an empty array when
/// omitted. There's no serde here — pulling one in for five fields
/// isn't worth a dependency, so this hand-rolls just enough of the JSON
/// grammar to read that shape.
pub mod json {
    use super::{LineItem, Money, Quantity};
    use std::fmt;

    const KNOWN: [&str; 5] = ["description", "quantity", "unit_price", "discount_bps", "tax_bps"];

    #[derive(Debug, Clone, PartialEq)]
    enum Value {
        Null,
        Bool(bool),
        Number(f64),
        String(String),
        Array(Vec<Value>),
        Object(Vec<(String, Value)>),
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum JsonError {
        Syntax { pos: usize, message: String },
        NotAnArray,
        NotAnObject { index: usize },
        MissingField { index: usize, field: &'static str },
        WrongType { index: usize, field: &'static str, expected: &'static str },
        InvalidField { index: usize, field: &'static str, message: String },
        UnknownField { index: usize, field: String },
    }

    impl fmt::Display for JsonError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                JsonError::Syntax { pos, message } => write!(f, "syntax error at character {pos}: {message}"),
                JsonError::NotAnArray => write!(f, "top-level JSON value must be an array of line items"),
                JsonError::NotAnObject { index } => write!(f, "item {}: expected an object", index + 1),
                JsonError::MissingField { index, field } => {
                    write!(f, "item {}: missing required field {field:?}", index + 1)
                }
                JsonError::WrongType { index, field, expected } => {
                    write!(f, "item {}: field {field:?} must be a {expected}", index + 1)
                }
                JsonError::InvalidField { index, field, message } => {
                    write!(f, "item {}: field {field:?}: {message}", index + 1)
                }
                JsonError::UnknownField { index, field } => {
                    write!(f, "item {}: unrecognized field {field:?}", index + 1)
                }
            }
        }
    }

    impl std::error::Error for JsonError {}

    /// Parses a full JSON document into line items.
    pub fn parse(input: &str) -> Result<Vec<LineItem>, JsonError> {
        let mut parser = Parser::new(input);
        parser.skip_ws();
        let value = parser.parse_value()?;
        parser.skip_ws();
        if parser.pos != parser.chars.len() {
            return Err(parser.error("unexpected trailing data after JSON value"));
        }

        let items = match value {
            Value::Array(items) => items,
            _ => return Err(JsonError::NotAnArray),
        };

        items.iter().enumerate().map(|(index, item)| line_item_from_value(index, item)).collect()
    }

    fn line_item_from_value(index: usize, value: &Value) -> Result<LineItem, JsonError> {
        let fields = match value {
            Value::Object(fields) => fields,
            _ => return Err(JsonError::NotAnObject { index }),
        };

        for (name, _) in fields {
            if !KNOWN.contains(&name.as_str()) {
                return Err(JsonError::UnknownField { index, field: name.clone() });
            }
        }

        let find = |name: &str| fields.iter().find(|(n, _)| n == name).map(|(_, v)| v);

        let description = match find("description") {
            Some(Value::String(s)) => s.clone(),
            Some(_) => return Err(JsonError::WrongType { index, field: "description", expected: "string" }),
            None => return Err(JsonError::MissingField { index, field: "description" }),
        };

        let quantity = match find("quantity") {
            Some(Value::String(s)) => s.parse::<Quantity>().map_err(|e| JsonError::InvalidField {
                index,
                field: "quantity",
                message: e.to_string(),
            })?,
            Some(_) => return Err(JsonError::WrongType { index, field: "quantity", expected: "string" }),
            None => return Err(JsonError::MissingField { index, field: "quantity" }),
        };

        let unit_price = match find("unit_price") {
            Some(Value::String(s)) => s.parse::<Money>().map_err(|e| JsonError::InvalidField {
                index,
                field: "unit_price",
                message: e.to_string(),
            })?,
            Some(_) => return Err(JsonError::WrongType { index, field: "unit_price", expected: "string" }),
            None => return Err(JsonError::MissingField { index, field: "unit_price" }),
        };

        let discount_bps = match find("discount_bps") {
            Some(Value::Number(n)) => number_to_bps(*n).ok_or_else(|| JsonError::InvalidField {
                index,
                field: "discount_bps",
                message: format!("expected a non-negative integer, got {n}"),
            })?,
            Some(_) => return Err(JsonError::WrongType { index, field: "discount_bps", expected: "integer" }),
            None => 0,
        };

        let tax_bps = match find("tax_bps") {
            Some(Value::Number(n)) => vec![number_to_bps(*n).ok_or_else(|| JsonError::InvalidField {
                index,
                field: "tax_bps",
                message: format!("expected a non-negative integer, got {n}"),
            })?],
            Some(Value::Array(rates)) => rates
                .iter()
                .map(|v| match v {
                    Value::Number(n) => number_to_bps(*n).ok_or_else(|| JsonError::InvalidField {
                        index,
                        field: "tax_bps",
                        message: format!("expected a non-negative integer, got {n}"),
                    }),
                    _ => Err(JsonError::WrongType { index, field: "tax_bps", expected: "integer or array of integers" }),
                })
                .collect::<Result<Vec<u32>, JsonError>>()?,
            Some(_) => {
                return Err(JsonError::WrongType { index, field: "tax_bps", expected: "integer or array of integers" })
            }
            None => Vec::new(),
        };

        Ok(LineItem { description, quantity, unit_price, discount_bps, tax_bps })
    }

    fn number_to_bps(n: f64) -> Option<u32> {
        if !n.is_finite() || n.fract() != 0.0 || n < 0.0 || n > u32::MAX as f64 {
            None
        } else {
            Some(n as u32)
        }
    }

    struct Parser {
        chars: Vec<char>,
        pos: usize,
    }

    impl Parser {
        fn new(input: &str) -> Self {
            Parser { chars: input.chars().collect(), pos: 0 }
        }

        fn peek(&self) -> Option<char> {
            self.chars.get(self.pos).copied()
        }

        fn advance(&mut self) -> Option<char> {
            let c = self.peek();
            if c.is_some() {
                self.pos += 1;
            }
            c
        }

        fn skip_ws(&mut self) {
            while matches!(self.peek(), Some(' ') | Some('\t') | Some('\n') | Some('\r')) {
                self.pos += 1;
            }
        }

        fn error(&self, message: &str) -> JsonError {
            JsonError::Syntax { pos: self.pos, message: message.to_string() }
        }

        fn parse_value(&mut self) -> Result<Value, JsonError> {
            self.skip_ws();
            match self.peek() {
                Some('{') => self.parse_object(),
                Some('[') => self.parse_array(),
                Some('"') => self.parse_string().map(Value::String),
                Some('t') => self.parse_literal("true", Value::Bool(true)),
                Some('f') => self.parse_literal("false", Value::Bool(false)),
                Some('n') => self.parse_literal("null", Value::Null),
                Some(c) if c == '-' || c.is_ascii_digit() => self.parse_number(),
                Some(_) => Err(self.error("unexpected character")),
                None => Err(self.error("unexpected end of input")),
            }
        }

        fn parse_object(&mut self) -> Result<Value, JsonError> {
            self.pos += 1; // consume '{'
            let mut fields = Vec::new();
            self.skip_ws();
            if self.peek() == Some('}') {
                self.pos += 1;
                return Ok(Value::Object(fields));
            }
            loop {
                self.skip_ws();
                if self.peek() != Some('"') {
                    return Err(self.error("expected a string key"));
                }
                let key = self.parse_string()?;
                self.skip_ws();
                if self.advance() != Some(':') {
                    return Err(self.error("expected ':' after object key"));
                }
                self.skip_ws();
                let value = self.parse_value()?;
                fields.push((key, value));
                self.skip_ws();
                match self.advance() {
                    Some(',') => {}
                    Some('}') => break,
                    _ => return Err(self.error("expected ',' or '}' in object")),
                }
            }
            Ok(Value::Object(fields))
        }

        fn parse_array(&mut self) -> Result<Value, JsonError> {
            self.pos += 1; // consume '['
            let mut items = Vec::new();
            self.skip_ws();
            if self.peek() == Some(']') {
                self.pos += 1;
                return Ok(Value::Array(items));
            }
            loop {
                self.skip_ws();
                let value = self.parse_value()?;
                items.push(value);
                self.skip_ws();
                match self.advance() {
                    Some(',') => {}
                    Some(']') => break,
                    _ => return Err(self.error("expected ',' or ']' in array")),
                }
            }
            Ok(Value::Array(items))
        }

        fn parse_string(&mut self) -> Result<String, JsonError> {
            self.pos += 1; // consume opening quote
            let mut s = String::new();
            loop {
                match self.advance() {
                    None => return Err(self.error("unterminated string")),
                    Some('"') => break,
                    Some('\\') => match self.advance() {
                        Some('"') => s.push('"'),
                        Some('\\') => s.push('\\'),
                        Some('/') => s.push('/'),
                        Some('b') => s.push('\u{8}'),
                        Some('f') => s.push('\u{c}'),
                        Some('n') => s.push('\n'),
                        Some('r') => s.push('\r'),
                        Some('t') => s.push('\t'),
                        Some('u') => {
                            let cp = self.parse_hex4()?;
                            let resolved = if (0xD800..=0xDBFF).contains(&cp) {
                                if self.advance() == Some('\\') && self.advance() == Some('u') {
                                    let low = self.parse_hex4()?;
                                    if (0xDC00..=0xDFFF).contains(&low) {
                                        0x10000 + ((cp - 0xD800) << 10) + (low - 0xDC00)
                                    } else {
                                        return Err(self.error("invalid low surrogate in unicode escape"));
                                    }
                                } else {
                                    return Err(self.error("unpaired high surrogate in unicode escape"));
                                }
                            } else {
                                cp
                            };
                            s.push(char::from_u32(resolved).ok_or_else(|| self.error("invalid unicode escape"))?);
                        }
                        _ => return Err(self.error("invalid escape sequence")),
                    },
                    Some(c) => s.push(c),
                }
            }
            Ok(s)
        }

        fn parse_hex4(&mut self) -> Result<u32, JsonError> {
            let mut value = 0u32;
            for _ in 0..4 {
                let c = self.advance().ok_or_else(|| self.error("unterminated unicode escape"))?;
                let digit = c.to_digit(16).ok_or_else(|| self.error("invalid hex digit in unicode escape"))?;
                value = value * 16 + digit;
            }
            Ok(value)
        }

        fn parse_literal(&mut self, word: &str, value: Value) -> Result<Value, JsonError> {
            for expected in word.chars() {
                if self.advance() != Some(expected) {
                    return Err(self.error(&format!("expected literal {word:?}")));
                }
            }
            Ok(value)
        }

        fn parse_number(&mut self) -> Result<Value, JsonError> {
            let start = self.pos;
            if self.peek() == Some('-') {
                self.pos += 1;
            }
            match self.peek() {
                Some('0') => self.pos += 1,
                Some(c) if c.is_ascii_digit() => {
                    while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                        self.pos += 1;
                    }
                }
                _ => return Err(self.error("invalid number")),
            }
            if self.peek() == Some('.') {
                self.pos += 1;
                if !matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                    return Err(self.error("invalid number: expected a digit after '.'"));
                }
                while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
            if matches!(self.peek(), Some('e') | Some('E')) {
                self.pos += 1;
                if matches!(self.peek(), Some('+') | Some('-')) {
                    self.pos += 1;
                }
                if !matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                    return Err(self.error("invalid number: expected a digit in exponent"));
                }
                while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
            let text: String = self.chars[start..self.pos].iter().collect();
            text.parse::<f64>().map(Value::Number).map_err(|_| self.error("invalid number"))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_required_fields_with_defaults() {
            let input = r#"[{"description":"Widget","quantity":"2","unit_price":"5.00"}]"#;
            let items = parse(input).unwrap();
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].description, "Widget");
            assert_eq!(items[0].quantity.to_string(), "2.000");
            assert_eq!(items[0].unit_price.to_string(), "5.00");
            assert_eq!(items[0].discount_bps, 0);
            assert_eq!(items[0].tax_bps, Vec::<u32>::new());
        }

        #[test]
        fn parses_optional_fields_across_whitespace_and_newlines() {
            let input = "  [\n  {\n    \"description\": \"Consulting, on-site\",\n    \"quantity\": \"2.5\",\n    \"unit_price\": \"150.00\",\n    \"discount_bps\": 1000,\n    \"tax_bps\": 825\n  }\n]\n";
            let items = parse(input).unwrap();
            assert_eq!(items[0].discount_bps, 1000);
            assert_eq!(items[0].tax_bps, vec![825]);
        }

        #[test]
        fn multiple_tax_rates_are_a_json_array() {
            let input = r#"[{"description":"Widget","quantity":"1","unit_price":"5.00","tax_bps":[500,300]}]"#;
            let items = parse(input).unwrap();
            assert_eq!(items[0].tax_bps, vec![500, 300]);
        }

        #[test]
        fn empty_array_is_valid() {
            assert_eq!(parse("[]").unwrap().len(), 0);
        }

        #[test]
        fn escaped_quote_in_description() {
            let input = r#"[{"description":"6\" pipe","quantity":"1","unit_price":"1.00"}]"#;
            let items = parse(input).unwrap();
            assert_eq!(items[0].description, "6\" pipe");
        }

        #[test]
        fn top_level_must_be_an_array() {
            let input = r#"{"description":"Widget"}"#;
            assert_eq!(parse(input), Err(JsonError::NotAnArray));
        }

        #[test]
        fn missing_required_field_is_an_error() {
            let input = r#"[{"quantity":"1","unit_price":"1.00"}]"#;
            assert_eq!(parse(input), Err(JsonError::MissingField { index: 0, field: "description" }));
        }

        #[test]
        fn unknown_field_is_an_error() {
            let input = r#"[{"description":"Widget","quantity":"1","unit_price":"1.00","sku":"ABC"}]"#;
            assert_eq!(parse(input), Err(JsonError::UnknownField { index: 0, field: "sku".to_string() }));
        }

        #[test]
        fn quantity_must_be_a_string_not_a_number() {
            let input = r#"[{"description":"Widget","quantity":2,"unit_price":"1.00"}]"#;
            assert_eq!(
                parse(input),
                Err(JsonError::WrongType { index: 0, field: "quantity", expected: "string" })
            );
        }

        #[test]
        fn malformed_json_is_a_syntax_error() {
            let input = r#"[{"description":"Widget",}]"#;
            assert!(matches!(parse(input), Err(JsonError::Syntax { .. })));
        }

        #[test]
        fn invalid_quantity_string_is_reported() {
            let input = r#"[{"description":"Widget","quantity":"1.2345","unit_price":"1.00"}]"#;
            assert!(matches!(parse(input), Err(JsonError::InvalidField { field: "quantity", .. })));
        }

        #[test]
        fn second_item_index_is_reported_in_errors() {
            let input = r#"[{"description":"Widget","quantity":"1","unit_price":"1.00"},{"quantity":"1","unit_price":"1.00"}]"#;
            assert_eq!(parse(input), Err(JsonError::MissingField { index: 1, field: "description" }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row per awkward case we've hit or expect to hit: fractional
    /// quantities, credits (negative quantities), a discount that zeroes
    /// tax, rounding exactly on a half cent in both directions, multiple
    /// tax rates that round independently, and the two error paths
    /// (overflow, out-of-range discount).
    struct Case {
        name: &'static str,
        quantity: i64,
        unit_price: i64,
        discount_bps: u32,
        tax_bps: Vec<u32>,
        expected: Result<(i64, i64, i64, i64), LineItemError>,
    }

    fn cases() -> Vec<Case> {
        vec![
            Case {
                name: "whole units, no discount or tax",
                quantity: 2000,
                unit_price: 500,
                discount_bps: 0,
                tax_bps: vec![],
                expected: Ok((1000, 0, 0, 1000)),
            },
            Case {
                name: "fractional quantity rounds an exact half cent up",
                quantity: 500,
                unit_price: 3,
                discount_bps: 0,
                tax_bps: vec![],
                expected: Ok((2, 0, 0, 2)),
            },
            Case {
                name: "negative quantity (a credit) rounds an exact half away from zero",
                quantity: -500,
                unit_price: 3,
                discount_bps: 0,
                tax_bps: vec![],
                expected: Ok((-2, 0, 0, -2)),
            },
            Case {
                name: "zero quantity zeroes everything even with discount and tax set",
                quantity: 0,
                unit_price: 999,
                discount_bps: 500,
                tax_bps: vec![800],
                expected: Ok((0, 0, 0, 0)),
            },
            Case {
                name: "discount rounds an exact half away from zero",
                quantity: 1000,
                unit_price: 100,
                discount_bps: 150,
                tax_bps: vec![],
                expected: Ok((100, 2, 0, 98)),
            },
            Case {
                name: "a 100% discount leaves nothing for tax to apply to",
                quantity: 1000,
                unit_price: 500,
                discount_bps: 10_000,
                tax_bps: vec![2000],
                expected: Ok((500, 500, 0, 0)),
            },
            Case {
                name: "discount then tax each round independently",
                quantity: 1000,
                unit_price: 999,
                discount_bps: 1000,
                tax_bps: vec![825],
                expected: Ok((999, 100, 74, 973)),
            },
            Case {
                name: "two tax rates round independently and can sum to more than one \
                       combined rate would",
                quantity: 1000,
                unit_price: 100,
                discount_bps: 0,
                tax_bps: vec![50, 50],
                expected: Ok((100, 0, 2, 102)),
            },
            Case {
                name: "quantity times price overflows an i64 cent count",
                quantity: 2000,
                unit_price: i64::MAX,
                discount_bps: 0,
                tax_bps: vec![],
                expected: Err(LineItemError::Overflow),
            },
            Case {
                name: "a discount above 100% is rejected outright",
                quantity: 1000,
                unit_price: 100,
                discount_bps: 10_001,
                tax_bps: vec![],
                expected: Err(LineItemError::InvalidDiscount(10_001)),
            },
        ]
    }

    #[test]
    fn totals_table() {
        for case in cases() {
            let item = LineItem {
                description: case.name.to_string(),
                quantity: Quantity(case.quantity),
                unit_price: Money(case.unit_price),
                discount_bps: case.discount_bps,
                tax_bps: case.tax_bps.clone(),
            };

            let got = item
                .totals()
                .map(|t| (t.subtotal.cents(), t.discount.cents(), t.tax.cents(), t.total.cents()));

            assert_eq!(got, case.expected, "case: {}", case.name);
        }
    }

    #[test]
    fn money_parses_dollars_and_cents() {
        assert_eq!("12.34".parse::<Money>().unwrap(), Money(1234));
        assert_eq!("-5".parse::<Money>().unwrap(), Money(-500));
        assert_eq!("0.05".parse::<Money>().unwrap(), Money(5));
        assert!("1.234".parse::<Money>().is_err());
        assert!("abc".parse::<Money>().is_err());
    }

    #[test]
    fn quantity_parses_up_to_three_decimal_places() {
        assert_eq!("2.5".parse::<Quantity>().unwrap(), Quantity(2500));
        assert_eq!("-0.5".parse::<Quantity>().unwrap(), Quantity(-500));
        assert!("1.2345".parse::<Quantity>().is_err());
    }

    #[test]
    fn money_display_pads_single_digit_cents() {
        assert_eq!(Money(105).to_string(), "1.05");
        assert_eq!(Money(-105).to_string(), "-1.05");
    }
}
