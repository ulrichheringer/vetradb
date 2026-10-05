//! Bounded, lossless SQL values. Persistence never routes exact numeric through float.
pub use bigdecimal::{BigDecimal, RoundingMode};
pub use chrono;
pub use serde;
use serde::{Deserialize, Serialize};
pub use serde_json;
use std::{cmp::Ordering, str::FromStr};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
    pub position: Option<usize>,
}
pub type Result<T> = std::result::Result<T, Error>;
impl Error {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            position: None,
        }
    }
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new("0A000", message)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Type {
    Bool,
    Int2,
    Int4,
    Int8,
    Float4,
    Float8,
    Text,
    Varchar(Option<u32>),
    Bytea,
    Uuid,
    Numeric(Option<(u16, u16)>),
    Date,
    Time,
    Timestamp,
    Timestamptz,
    Json,
    Jsonb,
}
impl Type {
    pub fn oid(&self) -> u32 {
        match self {
            Self::Bool => 16,
            Self::Bytea => 17,
            Self::Int8 => 20,
            Self::Int2 => 21,
            Self::Int4 => 23,
            Self::Text => 25,
            Self::Json => 114,
            Self::Float4 => 700,
            Self::Float8 => 701,
            Self::Varchar(_) => 1043,
            Self::Date => 1082,
            Self::Time => 1083,
            Self::Timestamp => 1114,
            Self::Timestamptz => 1184,
            Self::Numeric(_) => 1700,
            Self::Uuid => 2950,
            Self::Jsonb => 3802,
        }
    }
    pub fn from_oid(oid: u32) -> Result<Self> {
        Ok(match oid {
            16 => Self::Bool,
            17 => Self::Bytea,
            20 => Self::Int8,
            21 => Self::Int2,
            23 => Self::Int4,
            25 => Self::Text,
            114 => Self::Json,
            700 => Self::Float4,
            701 => Self::Float8,
            1043 => Self::Varchar(None),
            1082 => Self::Date,
            1083 => Self::Time,
            1114 => Self::Timestamp,
            1184 => Self::Timestamptz,
            1700 => Self::Numeric(None),
            2950 => Self::Uuid,
            3802 => Self::Jsonb,
            _ => return Err(Error::unsupported("type OID")),
        })
    }
}
// IEEE values are persisted as bits: NaN/infinity and negative zero survive restart.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scalar {
    Null,
    Bool(bool),
    Int(i64),
    Float(u64),
    Text(String),
    Bytes(Vec<u8>),
    Uuid([u8; 16]),
    Numeric(String),
    Date(i32),
    Time(i64),
    Timestamp(i64),
    Timestamptz(i64),
    Json(String),
}
pub const MAX_VALUE: usize = 1024 * 1024;
impl Scalar {
    pub fn decimal(text: &str) -> Result<Self> {
        if text.len() > 1000 {
            return Err(Error::new("54000", "numeric digit budget"));
        }
        let n = BigDecimal::from_str(text).map_err(|_| Error::new("22P02", "invalid numeric"))?;
        let (_, scale) = n.as_bigint_and_exponent();
        if !(-1000..=1000).contains(&scale) {
            return Err(Error::new("22003", "numeric scale budget"));
        }
        Ok(Self::Numeric(n.normalized().to_string()))
    }
    pub fn truth(&self) -> Result<Option<bool>> {
        match self {
            Self::Null => Ok(None),
            Self::Bool(b) => Ok(Some(*b)),
            _ => Err(Error::new("42804", "boolean expression required")),
        }
    }
    pub fn numeric(&self) -> Result<BigDecimal> {
        match self {
            Self::Int(n) => Ok(BigDecimal::from(*n)),
            Self::Numeric(s) => {
                BigDecimal::from_str(s).map_err(|_| Error::new("XX001", "numeric encoding"))
            }
            _ => Err(Error::new("42804", "numeric operand required")),
        }
    }
    pub fn compare(&self, other: &Self) -> Result<Option<Ordering>> {
        if matches!(self, Self::Null) || matches!(other, Self::Null) {
            return Ok(None);
        }
        let order = match (self, other) {
            (Self::Int(_) | Self::Numeric(_), Self::Int(_) | Self::Numeric(_)) => {
                self.numeric()?.cmp(&other.numeric()?)
            }
            (Self::Float(a), Self::Float(b)) => float_cmp(f64::from_bits(*a), f64::from_bits(*b)),
            (Self::Bool(a), Self::Bool(b)) => a.cmp(b),
            (Self::Text(a), Self::Text(b)) => a.as_bytes().cmp(b.as_bytes()),
            (Self::Bytes(a), Self::Bytes(b)) => a.cmp(b),
            (Self::Uuid(a), Self::Uuid(b)) => a.cmp(b),
            (Self::Date(a), Self::Date(b)) => a.cmp(b),
            (Self::Time(a), Self::Time(b))
            | (Self::Timestamp(a), Self::Timestamp(b))
            | (Self::Timestamptz(a), Self::Timestamptz(b)) => a.cmp(b),
            (Self::Json(_), Self::Json(_)) => {
                return Err(Error::unsupported("JSON comparison ordering"));
            }
            _ => return Err(Error::new("42883", "incompatible comparison types")),
        };
        Ok(Some(order))
    }
    pub fn text(&self) -> String {
        match self {
            Self::Null => String::new(),
            Self::Bool(v) => if *v { "t" } else { "f" }.into(),
            Self::Int(n) => n.to_string(),
            Self::Float(v) => f64::from_bits(*v).to_string(),
            Self::Text(s) | Self::Numeric(s) | Self::Json(s) => s.clone(),
            Self::Bytes(b) => format!("\\x{}", hex(b)),
            Self::Uuid(b) => {
                let s = hex(b);
                format!(
                    "{}-{}-{}-{}-{}",
                    &s[..8],
                    &s[8..12],
                    &s[12..16],
                    &s[16..20],
                    &s[20..]
                )
            }
            Self::Date(n) => date_epoch()
                .checked_add_signed(chrono::Duration::days(i64::from(*n)))
                .map_or_else(|| n.to_string(), |d| d.to_string()),
            Self::Time(n) => format!(
                "{:02}:{:02}:{:02}.{:06}",
                n / 3_600_000_000,
                n / 60_000_000 % 60,
                n / 1_000_000 % 60,
                n % 1_000_000
            ),
            Self::Timestamp(n) => chrono::DateTime::from_timestamp_micros(*n)
                .map_or_else(|| n.to_string(), |d| d.naive_utc().to_string()),
            Self::Timestamptz(n) => chrono::DateTime::from_timestamp_micros(*n)
                .map_or_else(|| n.to_string(), |d| d.to_rfc3339()),
        }
    }
    pub fn cast(&self, ty: &Type) -> Result<Self> {
        if matches!(self, Self::Null) {
            return Ok(Self::Null);
        }
        let text = self.text();
        if text.len() > MAX_VALUE {
            return Err(Error::new("54000", "value budget"));
        }
        Ok(match ty {
            Type::Bool => Self::Bool(match text.to_lowercase().as_str() {
                "true" | "t" | "1" | "yes" | "on" => true,
                "false" | "f" | "0" | "no" | "off" => false,
                _ => return Err(Error::new("22P02", "invalid boolean")),
            }),
            Type::Int2 | Type::Int4 | Type::Int8 => {
                let n = text
                    .parse::<i64>()
                    .map_err(|_| Error::new("22003", "integer out of range"))?;
                if matches!(ty, Type::Int2) && i16::try_from(n).is_err()
                    || matches!(ty, Type::Int4) && i32::try_from(n).is_err()
                {
                    return Err(Error::new("22003", "integer out of range"));
                }
                Self::Int(n)
            }
            Type::Float4 | Type::Float8 => {
                let n = text
                    .parse::<f64>()
                    .map_err(|_| Error::new("22P02", "invalid float"))?;
                let n = if matches!(ty, Type::Float4) {
                    let f = n as f32;
                    if f.is_infinite() && n.is_finite() {
                        return Err(Error::new("22003", "float4 overflow"));
                    }
                    f64::from(f)
                } else {
                    n
                };
                Self::Float(n.to_bits())
            }
            Type::Text => Self::Text(text),
            Type::Varchar(n) => {
                if n.is_some_and(|n| text.chars().count() > n as usize) {
                    return Err(Error::new("22001", "varchar length"));
                }
                Self::Text(text)
            }
            Type::Bytea => Self::Bytes(if let Self::Bytes(b) = self {
                b.clone()
            } else {
                unhex(
                    text.strip_prefix("\\x")
                        .ok_or_else(|| Error::new("22P02", "bytea needs hex format"))?,
                )?
            }),
            Type::Uuid => {
                let b = unhex(&text.replace('-', ""))?;
                Self::Uuid(
                    b.try_into()
                        .map_err(|_| Error::new("22P02", "UUID length"))?,
                )
            }
            Type::Numeric(spec) => {
                let Self::Numeric(s) = Self::decimal(&text)? else {
                    unreachable!()
                };
                let n = BigDecimal::from_str(&s).map_err(|_| Error::new("22P02", "numeric"))?;
                if let Some((precision, scale)) = spec {
                    if *precision == 0 || *precision > 1000 || scale > precision {
                        return Err(Error::new("22023", "numeric typmod"));
                    }
                    let n = n.with_scale_round(i64::from(*scale), RoundingMode::HalfUp);
                    let rendered = n.with_scale(i64::from(*scale)).to_plain_string();
                    let significant = rendered
                        .trim_start_matches('-')
                        .split('.')
                        .next()
                        .unwrap_or("")
                        .trim_start_matches('0')
                        .len();
                    if significant > usize::from(precision - scale) {
                        return Err(Error::new("22003", "numeric overflow"));
                    }
                    Self::Numeric(rendered)
                } else {
                    Self::Numeric(s)
                }
            }
            Type::Date => {
                let d = chrono::NaiveDate::parse_from_str(&text, "%Y-%m-%d")
                    .map_err(|_| Error::new("22007", "invalid date"))?;
                Self::Date(
                    i32::try_from((d - date_epoch()).num_days())
                        .map_err(|_| Error::new("22008", "date range"))?,
                )
            }
            Type::Time => {
                use chrono::Timelike;
                let t = chrono::NaiveTime::parse_from_str(&text, "%H:%M:%S%.f")
                    .map_err(|_| Error::new("22007", "invalid time"))?;
                Self::Time(
                    i64::from(t.num_seconds_from_midnight()) * 1_000_000
                        + i64::from(t.nanosecond() / 1000),
                )
            }
            Type::Timestamp => {
                let d = chrono::NaiveDateTime::parse_from_str(&text, "%Y-%m-%d %H:%M:%S%.f")
                    .or_else(|_| {
                        chrono::NaiveDateTime::parse_from_str(&text, "%Y-%m-%dT%H:%M:%S%.f")
                    })
                    .map_err(|_| Error::new("22007", "invalid timestamp"))?;
                Self::Timestamp(d.and_utc().timestamp_micros())
            }
            Type::Timestamptz => {
                let d = chrono::DateTime::parse_from_rfc3339(&text.replace(' ', "T")).map_err(
                    |_| Error::new("22007", "timestamp requires explicit RFC3339 offset"),
                )?;
                Self::Timestamptz(d.timestamp_micros())
            }
            Type::Json | Type::Jsonb => {
                let value: serde_json::Value =
                    serde_json::from_str(&text).map_err(|_| Error::new("22P02", "invalid JSON"))?;
                Self::Json(if matches!(ty, Type::Jsonb) {
                    value.to_string()
                } else {
                    text
                })
            }
        })
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut b = b"VSQLV001".to_vec();
        b.extend(serde_json::to_vec(self).map_err(|_| Error::new("XX001", "value encode"))?);
        if b.len() > MAX_VALUE {
            return Err(Error::new("54000", "value budget"));
        }
        Ok(b)
    }
    pub fn decode(b: &[u8]) -> Result<Self> {
        if b.len() > MAX_VALUE || !b.starts_with(b"VSQLV001") {
            return Err(Error::new("XX001", "value codec"));
        }
        serde_json::from_slice(&b[8..]).map_err(|_| Error::new("XX001", "value encoding"))
    }
    /// Ordered keys for fixed types; numeric uses an exponent/significand key.
    pub fn ordered(&self) -> Result<Vec<u8>> {
        let mut b = Vec::new();
        match self {
            Self::Null => b.push(0),
            Self::Bool(v) => b.extend([1, u8::from(*v)]),
            Self::Int(n) => {
                b.push(2);
                b.extend(((*n as u64) ^ (1 << 63)).to_be_bytes())
            }
            Self::Float(n) => {
                b.push(3);
                let f = f64::from_bits(*n);
                let n = if f.is_nan() {
                    u64::MAX
                } else if f == 0.0 {
                    1 << 63
                } else if n >> 63 == 1 {
                    !n
                } else {
                    n ^ (1 << 63)
                };
                b.extend(n.to_be_bytes())
            }
            Self::Numeric(s) => {
                b.push(4);
                let n = BigDecimal::from_str(s)
                    .map_err(|_| Error::new("XX001", "numeric"))?
                    .normalized();
                if n == BigDecimal::from(0) {
                    b.push(1)
                } else {
                    let (digits, scale) = n.as_bigint_and_exponent();
                    let s = digits.to_string();
                    let negative = s.starts_with('-');
                    let s = s.trim_start_matches('-');
                    let exponent =
                        i64::try_from(s.len()).map_err(|_| Error::new("54000", "numeric"))? - scale;
                    b.push(if negative { 0 } else { 2 });
                    let mut key = ((exponent as u64) ^ (1 << 63)).to_be_bytes().to_vec();
                    key.extend(s.bytes().map(|c| c - b'0' + 1));
                    key.push(0);
                    if negative {
                        key.iter_mut().for_each(|v| *v = !*v)
                    }
                    b.extend(key)
                }
            }
            Self::Json(_) => return Err(Error::unsupported("JSON comparison/index ordering")),
            Self::Text(s) => {
                b.push(5);
                escaped(s.as_bytes(), &mut b)
            }
            Self::Bytes(s) => {
                b.push(6);
                escaped(s, &mut b)
            }
            Self::Uuid(u) => {
                b.push(7);
                b.extend(u)
            }
            Self::Date(n) => {
                b.push(8);
                b.extend(((*n as u32) ^ (1 << 31)).to_be_bytes())
            }
            Self::Time(n) | Self::Timestamp(n) | Self::Timestamptz(n) => {
                b.push(9);
                b.extend(((*n as u64) ^ (1 << 63)).to_be_bytes())
            }
        }
        if b.len() > 2048 {
            return Err(Error::new("54000", "index key budget"));
        }
        Ok(b)
    }
}
fn escaped(s: &[u8], b: &mut Vec<u8>) {
    for c in s {
        if *c == 0 {
            b.extend([0, 255])
        } else {
            b.push(*c)
        }
    }
    b.extend([0, 0])
}
fn float_cmp(a: f64, b: f64) -> Ordering {
    if a.is_nan() {
        if b.is_nan() {
            Ordering::Equal
        } else {
            Ordering::Greater
        }
    } else if b.is_nan() {
        Ordering::Less
    } else {
        a.partial_cmp(&b).unwrap_or(Ordering::Equal)
    }
}
fn date_epoch() -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()
}
pub fn hex(b: &[u8]) -> String {
    {
        let mut s = String::with_capacity(b.len() * 2);
        for byte in b {
            s.push(char::from(b"0123456789abcdef"[(byte >> 4) as usize]));
            s.push(char::from(b"0123456789abcdef"[(byte & 15) as usize]));
        }
        s
    }
}
pub fn unhex(s: &str) -> Result<Vec<u8>> {
    if s.len() % 2 != 0 || s.len() > MAX_VALUE * 2 {
        return Err(Error::new("22P02", "hex encoding"));
    }
    s.as_bytes()
        .chunks_exact(2)
        .map(|b| {
            let s = std::str::from_utf8(b).map_err(|_| Error::new("22021", "UTF-8"))?;
            u8::from_str_radix(s, 16).map_err(|_| Error::new("22P02", "hex encoding"))
        })
        .collect()
}
/// PostgreSQL base-10000 quotient scale, then exact integer division with half-away rounding.
pub fn divide(a: &BigDecimal, b: &BigDecimal) -> Result<Scalar> {
    use bigdecimal::num_bigint::{BigInt, Sign};
    let (mut numerator, sa) = a.as_bigint_and_exponent();
    let (mut denominator, sb) = b.as_bigint_and_exponent();
    if denominator == BigInt::from(0) {
        return Err(Error::new("22012", "division by zero"));
    }
    fn lead(n: &BigDecimal) -> (i64, u32) {
        let (i, scale) = n.normalized().as_bigint_and_exponent();
        let digits = i.to_string();
        let digits = digits.trim_start_matches('-');
        let exponent = digits.len() as i64 - scale;
        let weight = (exponent - 1).div_euclid(4);
        let count = (exponent - weight * 4) as usize;
        let mut first = digits.chars().take(count).collect::<String>();
        while first.len() < count {
            first.push('0')
        }
        (weight, first.parse().unwrap_or(0))
    }
    let (wa, da) = lead(a);
    let (wb, db) = lead(b);
    let qweight = wa - wb - i64::from(da <= db);
    let scale = (16 - qweight * 4).max(sa.max(sb)).clamp(0, 1000);
    let exp = sb - sa + scale;
    if !(-3000..=3000).contains(&exp) {
        return Err(Error::new("54000", "numeric division budget"));
    }
    let power = BigInt::from(10).pow(exp.unsigned_abs() as u32);
    if exp >= 0 {
        numerator *= power
    } else {
        denominator *= power
    }
    let mut quotient = &numerator / &denominator;
    let remainder = &numerator % &denominator;
    let abs = |n: BigInt| if n.sign() == Sign::Minus { -n } else { n };
    if abs(remainder) * 2 >= abs(denominator.clone()) {
        quotient += if numerator.sign() == denominator.sign() {
            BigInt::from(1)
        } else {
            BigInt::from(-1)
        }
    }
    Ok(Scalar::Numeric(
        BigDecimal::new(quotient, scale).to_plain_string(),
    ))
}
