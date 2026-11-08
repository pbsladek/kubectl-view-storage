//! Bounded Kubernetes quantities and exact arithmetic, without floating point.
use std::{cmp::Ordering, fmt, str::FromStr};

pub const MAX_SCALAR_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuantityError {
    Syntax,
    Negative,
    TooLong,
    OutOfRange,
    ZeroDenominator,
}

impl fmt::Display for QuantityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Syntax => "invalid quantity syntax",
            Self::Negative => "negative quantities are unsupported",
            Self::TooLong => "quantity exceeds 256 bytes",
            Self::OutOfRange => "quantity exceeds the supported integer range",
            Self::ZeroDenominator => "ratio denominator must be positive",
        })
    }
}
impl std::error::Error for QuantityError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Bytes(u64);
impl Bytes {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Inodes(u64);
impl Inodes {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl FromStr for Bytes {
    type Err = QuantityError;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if input.len() > MAX_SCALAR_BYTES {
            return Err(QuantityError::TooLong);
        }
        if input.starts_with('-') {
            return Err(QuantityError::Negative);
        }
        let input = input.strip_prefix('+').unwrap_or(input);
        let mut digits = Vec::with_capacity(input.len() + 20);
        let mut point = false;
        let mut fraction = 0i32;
        let mut end = 0;
        for (i, byte) in input.bytes().enumerate() {
            match byte {
                b'0'..=b'9' => {
                    digits.push(byte - b'0');
                    if point {
                        fraction += 1;
                    }
                }
                b'.' if !point => point = true,
                _ => break,
            }
            end = i + 1;
        }
        if digits.is_empty() {
            return Err(QuantityError::Syntax);
        }
        let suffix = &input[end..];
        let (decimal, binary) = match suffix {
            "n" => (-9, 0),
            "u" => (-6, 0),
            "m" => (-3, 0),
            "" => (0, 0),
            "k" | "K" => (3, 0),
            "M" => (6, 0),
            "G" => (9, 0),
            "T" => (12, 0),
            "P" => (15, 0),
            "E" => (18, 0),
            "Ki" => (0, 10),
            "Mi" => (0, 20),
            "Gi" => (0, 30),
            "Ti" => (0, 40),
            "Pi" => (0, 50),
            "Ei" => (0, 60),
            s if s.starts_with(['e', 'E']) => (bounded_exponent(&s[1..])?, 0),
            _ => return Err(QuantityError::Syntax),
        };
        if digits.iter().all(|d| *d == 0) {
            return Ok(Self(0));
        }
        // Decimal long multiplication is bounded by the input length plus 19.
        // Huge exponents never allocate huge zero/power-of-ten buffers.
        for _ in 0..binary {
            let mut carry = 0;
            for digit in digits.iter_mut().rev() {
                let value = *digit * 2 + carry;
                *digit = value % 10;
                carry = value / 10;
            }
            if carry != 0 {
                digits.insert(0, carry);
            }
        }
        let first = digits
            .iter()
            .position(|d| *d != 0)
            .ok_or(QuantityError::Syntax)?;
        let digits = &digits[first..];
        let scale = decimal - fraction;
        let integer_len = digits.len() as i32 + scale;
        if integer_len > 20 {
            return Err(QuantityError::OutOfRange);
        }
        if integer_len <= 0 {
            return Ok(Self(1));
        }
        let count = integer_len as usize;
        let mut value = 0u64;
        for i in 0..count {
            value = value
                .checked_mul(10)
                .and_then(|v| v.checked_add(u64::from(digits.get(i).copied().unwrap_or(0))))
                .ok_or(QuantityError::OutOfRange)?;
        }
        if digits
            .get(count..)
            .is_some_and(|rest| rest.iter().any(|d| *d != 0))
        {
            value = value.checked_add(1).ok_or(QuantityError::OutOfRange)?;
        }
        Ok(Self(value))
    }
}

fn bounded_exponent(input: &str) -> Result<i32, QuantityError> {
    let (negative, digits) = if let Some(s) = input.strip_prefix('-') {
        (true, s)
    } else {
        (false, input.strip_prefix('+').unwrap_or(input))
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(QuantityError::Syntax);
    }
    // Above this bound every nonzero 256-byte significand is already outside
    // u64 or rounds up to one. This bounds work without altering valid values.
    let mut magnitude = 0i32;
    for byte in digits.bytes() {
        magnitude = (magnitude * 10 + i32::from(byte - b'0')).min(1000);
    }
    Ok(if negative { -magnitude } else { magnitude })
}

pub fn checked_sum(values: impl IntoIterator<Item = u128>) -> Result<u128, QuantityError> {
    values.into_iter().try_fold(0u128, |total, value| {
        total.checked_add(value).ok_or(QuantityError::OutOfRange)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExactRatio {
    numerator: u128,
    denominator: u128,
}

impl ExactRatio {
    pub fn new(numerator: u128, denominator: u128) -> Result<Self, QuantityError> {
        if denominator == 0 {
            return Err(QuantityError::ZeroDenominator);
        }
        let common = gcd(numerator, denominator);
        Ok(Self {
            numerator: numerator / common,
            denominator: denominator / common,
        })
    }
    pub const fn numerator(self) -> u128 {
        self.numerator
    }
    pub const fn denominator(self) -> u128 {
        self.denominator
    }

    /// Display only: exact half-up rounding to six percentage decimal places.
    pub fn percent(self) -> String {
        let mut whole = (self.numerator / self.denominator).to_string();
        let mut remainder = self.numerator % self.denominator;
        for _ in 0..8 {
            let (digit, rest) = decimal_step(remainder, self.denominator);
            whole.push(char::from(b'0' + digit));
            remainder = rest;
        }
        let mut digits: Vec<u8> = whole.bytes().map(|b| b - b'0').collect();
        if remainder >= self.denominator / 2 + self.denominator % 2 {
            let mut carry = true;
            for digit in digits.iter_mut().rev() {
                if !carry {
                    break;
                }
                if *digit == 9 {
                    *digit = 0;
                } else {
                    *digit += 1;
                    carry = false;
                }
            }
            if carry {
                digits.insert(0, 1);
            }
        }
        let split = digits.len() - 6;
        let integer: String = digits[..split]
            .iter()
            .map(|d| char::from(b'0' + *d))
            .collect();
        let integer = integer.trim_start_matches('0');
        let integer = if integer.is_empty() { "0" } else { integer };
        let fraction: String = digits[split..]
            .iter()
            .map(|d| char::from(b'0' + *d))
            .collect();
        let fraction = fraction.trim_end_matches('0');
        if fraction.is_empty() {
            integer.to_owned()
        } else {
            format!("{integer}.{fraction}")
        }
    }
}

fn decimal_step(remainder: u128, denominator: u128) -> (u8, u128) {
    let mut acc = 0;
    let mut digit = 0;
    for _ in 0..10 {
        if acc >= denominator - remainder {
            acc -= denominator - remainder;
            digit += 1;
        } else {
            acc += remainder;
        }
    }
    (digit, acc)
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

impl Ord for ExactRatio {
    fn cmp(&self, other: &Self) -> Ordering {
        let (mut a, mut b, mut c, mut d) = (
            self.numerator,
            self.denominator,
            other.numerator,
            other.denominator,
        );
        let mut reverse = false;
        loop {
            let comparison = (a / b).cmp(&(c / d));
            if comparison != Ordering::Equal {
                return if reverse {
                    comparison.reverse()
                } else {
                    comparison
                };
            }
            let (ar, cr) = (a % b, c % d);
            let comparison = match (ar == 0, cr == 0) {
                (true, true) => return Ordering::Equal,
                (true, false) => Some(Ordering::Less),
                (false, true) => Some(Ordering::Greater),
                _ => None,
            };
            if let Some(order) = comparison {
                return if reverse { order.reverse() } else { order };
            }
            (a, b, c, d) = (b, ar, d, cr);
            reverse = !reverse;
        }
    }
}
impl PartialOrd for ExactRatio {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Threshold {
    coefficient: u64,
    scale: u32,
}

impl Threshold {
    pub fn ratio(self) -> ExactRatio {
        // Constructors enforce positive, bounded coefficient and scale <= 6.
        ExactRatio {
            numerator: self.coefficient as u128,
            denominator: 100 * 10u128.pow(self.scale),
        }
        .canonical()
    }
    pub fn reached(self, used: u128, capacity: u128) -> bool {
        ExactRatio::new(used, capacity).is_ok_and(|r| r >= self.ratio())
    }
}
impl ExactRatio {
    fn canonical(self) -> Self {
        let common = gcd(self.numerator, self.denominator);
        Self {
            numerator: self.numerator / common,
            denominator: self.denominator / common,
        }
    }
}
impl FromStr for Threshold {
    type Err = QuantityError;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if input.len() > MAX_SCALAR_BYTES {
            return Err(QuantityError::TooLong);
        }
        let (whole, fraction) = input.split_once('.').unwrap_or((input, ""));
        if whole.is_empty()
            || !whole.bytes().all(|b| b.is_ascii_digit())
            || !fraction.bytes().all(|b| b.is_ascii_digit())
            || fraction.len() > 6
            || (input.contains('.') && fraction.is_empty())
        {
            return Err(QuantityError::Syntax);
        }
        let fraction = fraction.trim_end_matches('0');
        let scale = fraction.len() as u32;
        let coefficient = format!("{whole}{fraction}")
            .parse::<u64>()
            .map_err(|_| QuantityError::OutOfRange)?;
        if coefficient == 0 || coefficient > 100 * 10u64.pow(scale) {
            return Err(QuantityError::OutOfRange);
        }
        Ok(Self { coefficient, scale })
    }
}
impl fmt::Display for Threshold {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let divisor = 10u64.pow(self.scale);
        if self.scale == 0 {
            write!(f, "{}", self.coefficient)
        } else {
            write!(
                f,
                "{}.{:0width$}",
                self.coefficient / divisor,
                self.coefficient % divisor,
                width = self.scale as usize
            )
        }
    }
}
impl Ord for Threshold {
    fn cmp(&self, other: &Self) -> Ordering {
        self.ratio().cmp(&other.ratio())
    }
}
impl PartialOrd for Threshold {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
#[path = "../tests/unit/qty.rs"]
mod tests;
