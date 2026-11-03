//! Distribution functions for statistical tests: the regularized incomplete
//! beta and gamma functions, and the tail probabilities built on them.
//!
//! The engines compute the summary statistics (counts, means, spreads,
//! cross-tab counts); only these functions turn them into p-values. They
//! are checked against SciPy in `tests/stat_tests.rs`.

/// ln Γ(x) for x > 0 (Lanczos, g = 7, n = 9; about 15 significant digits).
pub fn ln_gamma(x: f64) -> f64 {
    const G: f64 = 7.0;
    const C: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        // Reflection: Γ(x)Γ(1−x) = π / sin(πx).
        let pi = std::f64::consts::PI;
        return (pi / (pi * x).sin()).ln() - ln_gamma(1.0 - x);
    }
    let x = x - 1.0;
    let mut a = C[0];
    let t = x + G + 0.5;
    for (i, c) in C.iter().enumerate().skip(1) {
        a += c / (x + i as f64);
    }
    0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + a.ln()
}

/// Continued fraction for the incomplete beta function (modified Lentz).
fn beta_cf(a: f64, b: f64, x: f64) -> f64 {
    const TINY: f64 = 1e-300;
    let (qab, qap, qam) = (a + b, a + 1.0, a - 1.0);
    let mut c = 1.0;
    let mut d = 1.0 - qab * x / qap;
    if d.abs() < TINY {
        d = TINY;
    }
    d = 1.0 / d;
    let mut h = d;
    for m in 1..=10_000 {
        let m = m as f64;
        let m2 = 2.0 * m;
        let aa = m * (b - m) * x / ((qam + m2) * (a + m2));
        d = 1.0 + aa * d;
        if d.abs() < TINY {
            d = TINY;
        }
        c = 1.0 + aa / c;
        if c.abs() < TINY {
            c = TINY;
        }
        d = 1.0 / d;
        h *= d * c;
        let aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2));
        d = 1.0 + aa * d;
        if d.abs() < TINY {
            d = TINY;
        }
        c = 1.0 + aa / c;
        if c.abs() < TINY {
            c = TINY;
        }
        d = 1.0 / d;
        let del = d * c;
        h *= del;
        if (del - 1.0).abs() < 1e-16 {
            break;
        }
    }
    h
}

/// The regularized incomplete beta function I_x(a, b).
pub fn beta_inc(a: f64, b: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let ln_front = ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * x.ln() + b * (1.0 - x).ln();
    // The continued fraction converges fast for x < (a+1)/(a+b+2).
    if x < (a + 1.0) / (a + b + 2.0) {
        ln_front.exp() * beta_cf(a, b, x) / a
    } else {
        1.0 - ln_front.exp() * beta_cf(b, a, 1.0 - x) / b
    }
}

/// The regularized upper incomplete gamma function Q(a, x) = Γ(a, x) / Γ(a).
pub fn gamma_q(a: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 1.0;
    }
    let ln_front = -x + a * x.ln() - ln_gamma(a);
    if x < a + 1.0 {
        // Series for P(a, x).
        let (mut sum, mut del, mut ap) = (1.0 / a, 1.0 / a, a);
        for _ in 0..100_000 {
            ap += 1.0;
            del *= x / ap;
            sum += del;
            if del.abs() < sum.abs() * 1e-16 {
                break;
            }
        }
        1.0 - sum * ln_front.exp()
    } else {
        // Continued fraction for Q(a, x) (modified Lentz).
        const TINY: f64 = 1e-300;
        let mut b = x + 1.0 - a;
        let mut c = 1.0 / TINY;
        let mut d = 1.0 / b;
        let mut h = d;
        for i in 1..100_000 {
            let an = -(i as f64) * (i as f64 - a);
            b += 2.0;
            d = an * d + b;
            if d.abs() < TINY {
                d = TINY;
            }
            c = b + an / c;
            if c.abs() < TINY {
                c = TINY;
            }
            d = 1.0 / d;
            let del = d * c;
            h *= del;
            if (del - 1.0).abs() < 1e-16 {
                break;
            }
        }
        ln_front.exp() * h
    }
}

/// Two-sided p-value of Student's t with `df` degrees of freedom.
pub fn t_two_sided(t: f64, df: f64) -> f64 {
    if !t.is_finite() {
        return 0.0;
    }
    beta_inc(df / 2.0, 0.5, df / (df + t * t))
}

/// Upper tail of the F distribution with (d1, d2) degrees of freedom.
pub fn f_upper(f: f64, d1: f64, d2: f64) -> f64 {
    if f <= 0.0 {
        return 1.0;
    }
    beta_inc(d2 / 2.0, d1 / 2.0, d2 / (d2 + d1 * f))
}

/// Upper tail of the chi-square distribution with `df` degrees of freedom.
pub fn chi2_upper(x: f64, df: f64) -> f64 {
    gamma_q(df / 2.0, x / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, rel: f64) -> bool {
        (a - b).abs() <= rel * b.abs().max(1e-300)
    }

    #[test]
    fn known_values() {
        assert!(close(ln_gamma(5.0), (24f64).ln(), 1e-14));
        assert!(close(ln_gamma(0.5), std::f64::consts::PI.sqrt().ln(), 1e-14));
        // t = 2.0 with 10 df: two-sided p = 0.07338803...
        assert!(close(t_two_sided(2.0, 10.0), 0.073_388_034_770_740_4, 1e-10));
        // chi-square 3.84 with 1 df is about 0.05.
        assert!(close(chi2_upper(3.841_458_820_694_124, 1.0), 0.05, 1e-10));
        // F(3, 20) = 3.098 is about 0.05.
        assert!(close(f_upper(3.098_391_212_140_781, 3.0, 20.0), 0.05, 1e-8));
        assert_eq!(t_two_sided(0.0, 5.0), 1.0);
        assert_eq!(chi2_upper(0.0, 3.0), 1.0);
    }
}
