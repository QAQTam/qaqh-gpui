//! 前端本地幂等键。
//!
//! V3 只要求这些 id **非空且 ≤128 字节**（`qaqh_core_vocab::ids` 的结构校验），
//! 语义由使用方决定：`command_id` 是重放去重键、`input_id` 是本地回声去重键。
//! 形状沿用本仓历史口径 `前缀_` + 26 字符 Crockford base32（与 `ses_*` 同观感）。
//!
//! 载荷 = 48 bit 毫秒时间戳 + 16 bit 进程内计数 + 64 bit 纳秒抖动：同机同进程内
//! 严格不重复即可，不做 UUIDv7 版本位（这里不是持久事实 id，不需要跨机可解析）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// 生成本地 id（`cmd_*` / `inp_*` / `wb_*`）。
pub fn new_id(prefix: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let ms = now.as_millis() as u64;
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let jitter = (now.subsec_nanos() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let value: u128 =
        ((ms as u128) << 80) | (((counter & 0xffff) as u128) << 64) | u128::from(jitter);
    let mut out = String::with_capacity(prefix.len() + 27);
    out.push_str(prefix);
    out.push('_');
    for position in (0..26).rev() {
        out.push(ALPHABET[((value >> (position * 5)) & 0x1f) as usize] as char);
    }
    out
}
