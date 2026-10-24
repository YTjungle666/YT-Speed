//! 带宽换算：与上游 JS 逐位对齐。
//!
//! 上游（`speedtest_worker.js`）：
//!
//! ```js
//! const speed = totLoaded / (t / 1000.0);                 // bytes / second
//! dlStatus = ((speed * 8 * settings.overheadCompensationFactor)
//!             / (settings.useMebibits ? 1048576 : 1000000)).toFixed(2);
//! ```
//!
//! 注意乘除顺序也照抄（`speed * 8 * factor / divisor`），因为 f64 不满足结合律，
//! 换个顺序最后一位就可能不同。

/// 换算参数。默认值即上游默认：`overheadCompensationFactor = 1.06`、十进兆。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Conv {
    /// 传输开销补偿系数，上游默认 1.06。
    pub overhead: f64,
    /// 除数：1 000 000（Mbit）或 1 048 576（Mibit）。
    pub divisor: f64,
}

impl Default for Conv {
    fn default() -> Self {
        Self {
            overhead: 1.06,
            divisor: 1_000_000.0,
        }
    }
}

impl Conv {
    /// 米比特单位（`useMebibits = true`）。
    pub fn mebibits() -> Self {
        Self {
            overhead: 1.06,
            divisor: 1_048_576.0,
        }
    }

    /// 原始速率，单位 Mbit/s（或 Mibit/s），已乘开销补偿。
    pub fn mbps(&self, bytes: f64, secs: f64) -> f64 {
        let speed = bytes / secs;
        speed * 8.0 * self.overhead / self.divisor
    }

    /// 上游 `dlStatus`/`ulStatus` 的字符串形态：两位小数。
    pub fn status(&self, bytes: f64, secs: f64) -> String {
        to_fixed2(self.mbps(bytes, secs))
    }
}

/// `Number.prototype.toFixed(2)` 的等价实现。
///
/// 与 Rust 的 `{:.2}` 的差别只在“恰好平局”时：JS 规范要求取较大的那个数
/// （half-up），而 Rust 的格式化是 half-to-even（`0.125` → JS `"0.13"`，Rust `"0.12"`）。
/// 测速数字虽极少恰好落在 .xx5，但边界是真实存在的，所以这里按十进制展开后
/// 手工 half-up。展开必须取 **17 位**小数：f64 的双精度有效位是 17 位十进制，
/// 取少了会把“略小于平局”的值四舍五入成平局——例如 `2.675` 的实际值是
/// 2.67499999999999982，JS 得 `"2.67"`；若只展开 10 位变成 2.6750000000，
/// 就会错判成平局进位成 `"2.68"`。这是最初实现踩过的坑，用例钉在 tests/parity.rs。
pub fn to_fixed2(x: f64) -> String {
    if x.is_nan() || x.is_infinite() {
        // JS：NaN.toFixed(2) === "NaN"，Infinity → "Infinity"
        return if x.is_nan() {
            "NaN".into()
        } else {
            "Infinity".into()
        };
    }
    if x == 0.0 {
        return "0.00".into(); // 含 -0.0
    }
    let neg = x < 0.0;
    let s = format!("{:.17}", x.abs());
    let (int_part, frac) = s.split_once('.').expect("定点格式必带小数点");

    let digits = frac.as_bytes();
    let round_up = digits[2] >= b'5';

    let mut cents = [digits[0] - b'0', digits[1] - b'0'];
    let mut int_v: Vec<u8> = int_part.as_bytes().to_vec();

    if round_up {
        let mut carry = true;
        for i in (0..2).rev() {
            if !carry {
                break;
            }
            if cents[i] == 9 {
                cents[i] = 0;
            } else {
                cents[i] += 1;
                carry = false;
            }
        }
        if carry {
            let mut c = true;
            for i in (0..int_v.len()).rev() {
                if !c {
                    break;
                }
                if int_v[i] == b'9' {
                    int_v[i] = b'0';
                } else {
                    int_v[i] += 1;
                    c = false;
                }
            }
            if c {
                int_v.insert(0, b'1');
            }
        }
    }

    let mut out = String::with_capacity(int_v.len() + 4);
    if neg {
        out.push('-');
    }
    out.push_str(std::str::from_utf8(&int_v).expect("ASCII"));
    out.push('.');
    out.push((b'0' + cents[0]) as char);
    out.push((b'0' + cents[1]) as char);
    out
}
