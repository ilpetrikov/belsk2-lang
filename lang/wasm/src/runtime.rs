//! Parts of the runtime written in Belsk2 itself and compiled into every
//! WASM build (only what a program uses ends up in the module).

/// Formats `double` and `float` values as the interpreter prints them:
/// whole numbers without a point, otherwise the shortest text in plain
/// notation that reads back as the same value.
pub const RUNTIME: &str = r#"
fn rtPow10(k: int): double {
    var p = 1.0;
    var i = 0;
    while i < k {
        p = p * 10.0;
        i += 1;
    }
    return p;
}

fn rtSame(x: double, a: double, single: bool): bool {
    if single {
        return float(x) == float(a);
    }
    return x == a;
}

fn rtZeros(n: int): string {
    var s = "";
    var i = 0;
    while i < n {
        s += "0";
        i += 1;
    }
    return s;
}

fn rtPoint(s: long, k: int): string {
    var digits = str(s);
    if k == 0 {
        return digits;
    }
    while len(digits) <= k {
        digits = "0" + digits;
    }
    var cut = len(digits) - k;
    return substr(digits, 0, cut) + "." + substr(digits, cut, k);
}

fn rtFormat(v: double, single: bool): string {
    if v != v {
        return "NaN";
    }
    if v > 1.7976931348623157e308 {
        return "Infinity";
    }
    if v < -1.7976931348623157e308 {
        return "-Infinity";
    }
    var sign = "";
    var a = v;
    if v < 0.0 {
        sign = "-";
        a = -v;
    }
    if a < 1e15 && a == double(long(a)) {
        return str(long(v));
    }
    // e: the position of the first significant digit.
    var e = 0;
    var t = a;
    while t >= 10.0 {
        t = t / 10.0;
        e += 1;
    }
    while t < 1.0 {
        t = t * 10.0;
        e -= 1;
    }
    // The fewest significant digits that read back as the same value
    // (exact while the powers of ten are; else 17 digits at most).
    var p = 1;
    while p <= 17 {
        var k = p - 1 - e;
        if k >= 0 {
            var x = a;
            var kk = k;
            while kk > 300 {
                x = x * 1e10;
                kk -= 10;
            }
            var scale = rtPow10(kk);
            var scaled = x * scale + 0.5;
            if scaled < 9.2e18 {
                var d = long(scaled);
                if p == 17 || rtSame(double(d) / scale, x, single) {
                    return sign + rtPoint(d, k);
                }
            }
        } else {
            var scale = rtPow10(-k);
            var d = long(a / scale + 0.5);
            if p == 17 || rtSame(double(d) * scale, a, single) {
                return sign + str(d) + rtZeros(-k);
            }
        }
        p += 1;
    }
    return sign + str(long(a));
}
"#;
