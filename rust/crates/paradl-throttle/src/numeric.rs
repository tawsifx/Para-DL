//! Numeric parity helpers for Python floating-point semantics.
//!
//! Python's `round()` uses correct decimal rounding with ties-to-even, while
//! `statistics.median`/`fmean`/`pstdev` use exact or compensated summation.
//! These helpers reproduce those semantics without extra dependencies.

/// Round `value` to `ndigits` decimal places using Python's banker's rounding.
///
/// Rust's fixed-precision formatter already rounds half-to-even on the exact
/// binary value, which is precisely what CPython's `float.__round__` does via
/// `_Py_dg_dtoa`. Formatting and parsing back avoids the classic
/// `(x * 100.0).round() / 100.0` drift.
///
/// ```
/// use paradl_throttle::numeric::py_round;
/// assert_eq!(py_round(2.5, 0), 2.0);
/// assert_eq!(py_round(3.5, 0), 4.0);
/// assert_eq!(py_round(2.675, 2), 2.67);
/// ```
pub fn py_round(value: f64, ndigits: i32) -> f64 {
    if !value.is_finite() {
        return value;
    }
    if ndigits < 0 {
        let factor = 10_f64.powi(-ndigits);
        return (value / factor).round_ties_even() * factor;
    }
    format!("{:.*}", ndigits as usize, value)
        .parse::<f64>()
        .unwrap_or(value)
}

/// Compensated (Neumaier) summation, equivalent to Python's `math.fsum`.
///
/// ```
/// use paradl_throttle::numeric::fsum;
/// assert_eq!(fsum(&[0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1]), 1.0);
/// ```
pub fn fsum(values: &[f64]) -> f64 {
    let mut total = 0.0_f64;
    let mut compensation = 0.0_f64;
    for &value in values {
        let t = total + value;
        if total.abs() >= value.abs() {
            compensation += (total - t) + value;
        } else {
            compensation += (value - t) + total;
        }
        total = t;
    }
    total + compensation
}

/// Arithmetic mean using compensated summation (`statistics.fmean`).
pub fn fmean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    fsum(values) / values.len() as f64
}

/// Median of a slice (`statistics.median`).
///
/// For an even number of values the arithmetic mean of the two middle values
/// is returned, exactly as CPython does.
pub fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    }
}

/// Population standard deviation (`statistics.pstdev`).
pub fn pstdev(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mean = fmean(values);
    let squared: Vec<f64> = values.iter().map(|x| (x - mean) * (x - mean)).collect();
    (fsum(&squared) / values.len() as f64).sqrt()
}

/// 1 / √π — the reciprocal of Γ(1/2).
const INV_SQRT_PI: f64 = 0.5641895835477563;

fn lower_gamma_series(a: f64, x: f64) -> f64 {
    let mut sum = 1.0 / a;
    let mut term = sum;
    let mut ap = a;
    for _ in 0..1000 {
        ap += 1.0;
        term *= x / ap;
        sum += term;
        if term.abs() <= sum.abs() * f64::EPSILON {
            break;
        }
    }
    // Γ(1/2) = √π, so dividing by Γ(a) is multiplying by 1/√π.
    sum * (-x + a * x.ln()).exp() * INV_SQRT_PI
}

fn upper_gamma_fraction(a: f64, x: f64) -> f64 {
    const EPS: f64 = 3.0e-15;
    const FPMIN: f64 = 1.0e-300;
    let mut b = x + 1.0 - a;
    let mut c = 1.0 / FPMIN;
    let mut d = 1.0 / b;
    let mut h = d;
    for i in 1..=1000 {
        let an = -(i as f64) * (i as f64 - a);
        b += 2.0;
        d = an * d + b;
        if d.abs() < FPMIN {
            d = FPMIN;
        }
        c = b + an / c;
        if c.abs() < FPMIN {
            c = FPMIN;
        }
        d = 1.0 / d;
        let delta = d * c;
        h *= delta;
        if (delta - 1.0).abs() <= EPS {
            break;
        }
    }
    (-x + a * x.ln()).exp() * h * INV_SQRT_PI
}

/// Complementary error function, matching `math.erfc` to machine precision.
///
/// Implemented as the regularised upper incomplete gamma function
/// `Q(1/2, x²)` using a series/continued-fraction split. This avoids the
/// unstable `float_erf` feature and keeps the crate dependency-free.
pub fn erfc(x: f64) -> f64 {
    if x < 0.0 {
        return 2.0 - erfc(-x);
    }
    if x == 0.0 {
        return 1.0;
    }
    let z = x * x;
    if z < 1.5 {
        1.0 - lower_gamma_series(0.5, z)
    } else {
        upper_gamma_fraction(0.5, z)
    }
}
