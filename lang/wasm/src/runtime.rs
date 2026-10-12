//! Parts of the runtime written in Belsk2 itself and compiled into every
//! WASM build (only what a program uses ends up in the module):
//!
//! - `rtFormat`: `str()` of `double` and `float`;
//! - `rtAny*`: operators on `any` values, with the binary numeric
//!   promotion of the interpreter;
//! - `rtParse`: a number from a string (`int(x)` of an `any` holding one).

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

fn rtFail(): any {
    int[] none = [];
    return none[0];
}

// The kind of a number: 1 sbyte, 2 byte, 3 short, 4 ushort (and char),
// 5 int, 6 uint, 7 long, 8 ulong, 9 float, 10 double; 0 if not a number.
fn rtNumKind(a: any): int {
    if a is int { return 5; }
    if a is double { return 10; }
    if a is long { return 7; }
    if a is char { return 4; }
    if a is float { return 9; }
    if a is byte { return 2; }
    if a is sbyte { return 1; }
    if a is short { return 3; }
    if a is ushort { return 4; }
    if a is uint { return 6; }
    if a is ulong { return 8; }
    return 0;
}

fn rtSigned(k: int): bool {
    return k == 1 || k == 3 || k == 5 || k == 7;
}

// The kind both operands are converted to (0: none).
fn rtPromote(a: int, b: int): int {
    if a == 0 || b == 0 {
        return 0;
    }
    if a >= 9 || b >= 9 {
        if a > b {
            return a;
        }
        return b;
    }
    if a == 8 || b == 8 {
        if (a == 8 && rtSigned(b)) || (b == 8 && rtSigned(a)) {
            return 0;
        }
        return 8;
    }
    if a == 7 || b == 7 {
        return 7;
    }
    if a == 6 || b == 6 {
        if (a == 6 && rtSigned(b)) || (b == 6 && rtSigned(a)) {
            return 7;
        }
        return 6;
    }
    return 5;
}

// Operators: 0 +, 1 -, 2 *, 3 /, 4 %, 5 &, 6 |, 7 ^, 8 <<, 9 >>.
fn rtOpInt(x: int, y: int, op: int): any {
    if op == 0 { return x + y; }
    if op == 1 { return x - y; }
    if op == 2 { return x * y; }
    if op == 3 { return x / y; }
    if op == 4 { return x % y; }
    if op == 5 { return x & y; }
    if op == 6 { return x | y; }
    if op == 7 { return x ^ y; }
    if op == 8 { return x << y; }
    return x >> y;
}

fn rtOpUint(x: uint, y: int, op: int): any {
    if op == 8 { return x << y; }
    if op == 9 { return x >> y; }
    uint z = uint(y);
    if op == 0 { return x + z; }
    if op == 1 { return x - z; }
    if op == 2 { return x * z; }
    if op == 3 { return x / z; }
    if op == 4 { return x % z; }
    if op == 5 { return x & z; }
    if op == 6 { return x | z; }
    return x ^ z;
}

fn rtOpLong(x: long, y: long, op: int): any {
    if op == 0 { return x + y; }
    if op == 1 { return x - y; }
    if op == 2 { return x * y; }
    if op == 3 { return x / y; }
    if op == 4 { return x % y; }
    if op == 5 { return x & y; }
    if op == 6 { return x | y; }
    if op == 7 { return x ^ y; }
    if op == 8 { return x << int(y); }
    return x >> int(y);
}

fn rtOpUlong(x: ulong, y: ulong, op: int): any {
    if op == 0 { return x + y; }
    if op == 1 { return x - y; }
    if op == 2 { return x * y; }
    if op == 3 { return x / y; }
    if op == 4 { return x % y; }
    if op == 5 { return x & y; }
    if op == 6 { return x | y; }
    if op == 7 { return x ^ y; }
    if op == 8 { return x << int(y); }
    return x >> int(y);
}

fn rtOpFloat(x: float, y: float, op: int): any {
    if op == 0 { return x + y; }
    if op == 1 { return x - y; }
    if op == 2 { return x * y; }
    if op == 3 { return x / y; }
    if op == 4 { return x % y; }
    return rtFail();
}

fn rtOpDouble(x: double, y: double, op: int): any {
    if op == 0 { return x + y; }
    if op == 1 { return x - y; }
    if op == 2 { return x * y; }
    if op == 3 { return x / y; }
    if op == 4 { return x % y; }
    return rtFail();
}

fn rtAnyOp(a: any, b: any, op: int): any {
    if op == 0 && (a is string || b is string || (a is char && b is char)) {
        return str(a) + str(b);
    }
    if op >= 5 && op <= 7 && a is bool && b is bool {
        bool x = a;
        bool y = b;
        if op == 5 { return x & y; }
        if op == 6 { return x | y; }
        return x ^ y;
    }
    var ka = rtNumKind(a);
    var kb = rtNumKind(b);
    var k = rtPromote(ka, kb);
    if op >= 8 {
        // A shift keeps the (promoted) type of its left operand.
        if kb == 0 || kb >= 9 || ka >= 9 {
            return rtFail();
        }
        k = ka;
        if k > 0 && k < 5 {
            k = 5;
        }
    }
    if k == 0 {
        return rtFail();
    }
    if k == 10 { return rtOpDouble(double(a), double(b), op); }
    if k == 9 { return rtOpFloat(float(a), float(b), op); }
    if k == 8 { return rtOpUlong(ulong(a), ulong(b), op); }
    if k == 7 { return rtOpLong(long(a), long(b), op); }
    if k == 6 { return rtOpUint(uint(a), int(b), op); }
    return rtOpInt(int(a), int(b), op);
}

// -1, 0 or 1; 2 if the numbers are unordered (NaN).
fn rtAnyOrder(a: any, b: any): int {
    if a is string && b is string {
        string x = a;
        string y = b;
        if x < y { return -1; }
        if x > y { return 1; }
        return 0;
    }
    var ka = rtNumKind(a);
    var kb = rtNumKind(b);
    if ka == 0 || kb == 0 {
        rtFail();
    }
    if ka >= 9 || kb >= 9 {
        double x = double(a);
        double y = double(b);
        if x < y { return -1; }
        if x > y { return 1; }
        if x == y { return 0; }
        return 2;
    }
    if ka == 8 || kb == 8 {
        if rtSigned(ka) && long(a) < 0L { return -1; }
        if rtSigned(kb) && long(b) < 0L { return 1; }
        ulong x = ulong(a);
        ulong y = ulong(b);
        if x < y { return -1; }
        if x > y { return 1; }
        return 0;
    }
    long x = long(a);
    long y = long(b);
    if x < y { return -1; }
    if x > y { return 1; }
    return 0;
}

// Comparisons: 0 <, 1 >, 2 <=, 3 >=.
fn rtAnyCmp(a: any, b: any, op: int): bool {
    var c = rtAnyOrder(a, b);
    if c == 2 { return false; }
    if op == 0 { return c < 0; }
    if op == 1 { return c > 0; }
    if op == 2 { return c <= 0; }
    return c >= 0;
}

fn rtAnyNeg(a: any): any {
    var k = rtNumKind(a);
    if k == 10 {
        double x = a;
        return -x;
    }
    if k == 9 {
        float x = a;
        return -x;
    }
    if k == 0 || k == 8 {
        return rtFail();
    }
    if k >= 6 {
        long x = long(a);
        return -x;
    }
    int x = int(a);
    return -x;
}

fn rtAnyBitNot(a: any): any {
    var k = rtNumKind(a);
    if k == 0 || k >= 9 {
        return rtFail();
    }
    if k == 8 {
        ulong x = a;
        return ~x;
    }
    if k == 7 {
        long x = a;
        return ~x;
    }
    if k == 6 {
        uint x = a;
        return ~x;
    }
    int x = int(a);
    return ~x;
}

fn rtSpace(c: char): bool {
    var k = int(c);
    return k == 32 || k == 9 || k == 10 || k == 13;
}

fn rtDigit(c: char): int {
    var d = int(c) - 48;
    if d >= 0 && d <= 9 {
        return d;
    }
    return -1;
}

// A number from a string, as a long if it is a whole number, else a
// double; stops the program if it is not one.
fn rtParse(s: string): any {
    var i = 0;
    var n = len(s);
    while i < n && rtSpace(s[i]) { i += 1; }
    while n > i && rtSpace(s[n - 1]) { n -= 1; }
    var negative = false;
    if i < n && (int(s[i]) == 45 || int(s[i]) == 43) {
        negative = int(s[i]) == 45;
        i += 1;
    }
    var digits = 0;
    var m = 0L;
    var scale = 0;
    while i < n && rtDigit(s[i]) >= 0 {
        if digits < 18 {
            m = m * 10L + long(rtDigit(s[i]));
        } else {
            scale += 1;
        }
        digits += 1;
        i += 1;
    }
    var whole = true;
    if i < n && int(s[i]) == 46 {
        whole = false;
        i += 1;
        while i < n && rtDigit(s[i]) >= 0 {
            if digits < 18 {
                m = m * 10L + long(rtDigit(s[i]));
                scale -= 1;
            }
            digits += 1;
            i += 1;
        }
    }
    if digits == 0 {
        return rtFail();
    }
    if i < n && (int(s[i]) == 101 || int(s[i]) == 69) {
        whole = false;
        i += 1;
        var minus = false;
        if i < n && (int(s[i]) == 45 || int(s[i]) == 43) {
            minus = int(s[i]) == 45;
            i += 1;
        }
        var e = 0;
        var seen = false;
        while i < n && rtDigit(s[i]) >= 0 {
            if e < 100000 {
                e = e * 10 + rtDigit(s[i]);
            }
            seen = true;
            i += 1;
        }
        if !seen {
            return rtFail();
        }
        if minus {
            scale -= e;
        } else {
            scale += e;
        }
    }
    if i != n {
        return rtFail();
    }
    if whole && scale == 0 {
        if negative {
            return -m;
        }
        return m;
    }
    var v = double(m);
    while scale > 22 {
        v = v * 1e22;
        scale -= 22;
    }
    while scale < -22 {
        v = v / 1e22;
        scale += 22;
    }
    if scale >= 0 {
        v = v * rtPow10(scale);
    } else {
        v = v / rtPow10(-scale);
    }
    if negative {
        return -v;
    }
    return v;
}
"#;
