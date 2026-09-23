# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import math

# This implements the quantile function (the inverse of the CDF) for
# Student's t-distribution, which is used by perfcompare.py for calculating
# confidence intervals.
#
# This function is implemented by SciPy as scipy.stats.t.ppf(), but we
# don't want to depend on SciPy because we don't have a build of it which
# works with fuchsia-vendored-python, and in general it is difficult to
# build Python libraries that use C extension modules for use with
# fuchsia-vendored-python.  It is simpler to provide our own implementation
# of this one function instead!
#
# The implementation here was written by Gemini.


def t_pdf(x, df):
    """Probability Density Function of Student's t-distribution."""
    coeff = math.exp(math.lgamma((df + 1) / 2) - math.lgamma(df / 2))
    coeff /= math.sqrt(df * math.pi)
    return coeff * (1 + (x**2) / df) ** (-(df + 1) / 2)


def _beta_cf(a, b, x, max_iter=200, eps=1e-12):
    """Continued fraction helper for incomplete beta (modified for stability)."""
    m = 1
    qab, qap, qam = a + b, a + 1, a - 1
    c, d = 1.0, 1.0 - qab * x / qap
    if abs(d) < eps:
        d = eps
    d = 1.0 / d
    h = d

    for m in range(1, max_iter + 1):
        m2 = 2 * m
        # Even step
        aa = m * (b - m) * x / ((qam + m2) * (a + m2))
        d = 1.0 + aa * d
        if abs(d) < eps:
            d = eps
        c = 1.0 + aa / c
        if abs(c) < eps:
            c = eps
        d = 1.0 / d
        h *= d * c
        # Odd step
        aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2))
        d = 1.0 + aa * d
        if abs(d) < eps:
            d = eps
        c = 1.0 + aa / c
        if abs(c) < eps:
            c = eps
        d = 1.0 / d
        h *= d * c
        # Early exit on convergence
        if abs(d * c - 1.0) < eps:
            break
    return h


def beta_inc(a, b, x):
    """Regularized incomplete beta function with domain guards."""
    if x <= 0:
        return 0.0
    if x >= 1:
        return 1.0

    # Log-gamma for the coefficient
    lbeta = math.lgamma(a) + math.lgamma(b) - math.lgamma(a + b)
    bt = math.exp(a * math.log(x) + b * math.log(1 - x) - lbeta)

    if x < (a + 1) / (a + b + 2):
        return bt * _beta_cf(a, b, x) / a
    else:
        return 1.0 - bt * _beta_cf(b, a, 1 - x) / b


def t_cdf(x, df):
    """CDF of Student's t using the relationship with the Beta function."""
    if x == 0:
        return 0.5
    # The symmetry of the t-distribution
    xt = df / (df + x**2)
    prob = beta_inc(df / 2, 0.5, xt)
    return 1 - 0.5 * prob if x > 0 else 0.5 * prob


def t_ppf(p, df, tol=1e-10):
    """Inverse CDF (Quantile function) using Newton-Raphson."""
    if p <= 0 or p >= 1:
        raise ValueError("Probability p must be in (0, 1)")

    # Initial guess using a rough normal approximation
    # For df > 2, the variance is df/(df-2), but 0 is a safe start
    x = 0.0

    for _ in range(50):
        f_x = t_cdf(x, df) - p
        f_prime_x = t_pdf(x, df)

        # Avoid division by zero
        if f_prime_x == 0:
            break

        dx = f_x / f_prime_x
        x = x - dx

        if abs(dx) < tol:
            return x
    return x
