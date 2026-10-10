//! 统一的模型刊例价表（美元 / 百万 token）。
//!
//! 所有 adapter 共用：用量事件按「模型」计价，而不是按 harness 套一个固定价。
//! 价格来源：Anthropic / OpenAI 官方刊例价（与 OpenRouter 公开价表核对，2026-10），
//! DeepSeek 官方美元价，其余国产模型取官方/聚合平台公开价的近似档。
//! 未知模型返回 None —— 不计费、在界面上标注「未定价」，绝不静默套用默认价。

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    /// 未命中缓存的输入
    pub input: f64,
    pub output: f64,
    /// 缓存读取（命中）
    pub cache_read: f64,
    /// 缓存写入（5 分钟 TTL / 通用）
    pub cache_write: f64,
    /// 缓存写入（1 小时 TTL，Claude 专有，2× 输入价）
    pub cache_write_1h: f64,
}

/// 单个事件的 token 分桶（互不重叠：input 不含缓存部分）
#[derive(Debug, Clone, Copy, Default)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    /// 5 分钟 / 通用缓存写入
    pub cache_write: u64,
    /// 1 小时缓存写入（计入 cache_write 总量，但按 1h 价计费）
    pub cache_write_1h: u64,
}

const fn claude(input: f64, output: f64, cache_read: f64) -> Price {
    Price {
        input,
        output,
        cache_read,
        cache_write: input * 1.25,
        cache_write_1h: input * 2.0,
    }
}

/// 缓存写入按输入价的 1.25 倍（OpenAI GPT-6 / 5.6 系）
const fn oai_cw(input: f64, output: f64, cache_read: f64) -> Price {
    Price {
        input,
        output,
        cache_read,
        cache_write: input * 1.25,
        cache_write_1h: input * 1.25,
    }
}

/// 无缓存写入概念的模型：写入按输入价计
const fn flat(input: f64, output: f64, cache_read: f64) -> Price {
    Price {
        input,
        output,
        cache_read,
        cache_write: input,
        cache_write_1h: input,
    }
}

/// 规则按顺序匹配，先命中者生效：每条规则的所有片段都必须出现在归一化后的模型名中。
/// 越具体的规则越靠前（opus-5-5 在 opus-5 之前，gpt-5-4-mini 在 gpt-5-4 之前）。
const RULES: &[(&[&str], Price)] = &[
    // ---- Anthropic Claude ----
    (&["fable-5-1"], claude(10.0, 50.0, 0.25)),
    (&["mythos-5-1"], claude(10.0, 50.0, 0.25)),
    (&["fable"], claude(10.0, 50.0, 1.0)),
    (&["mythos"], claude(10.0, 50.0, 1.0)),
    (&["opus-5-5"], claude(4.0, 20.0, 0.2)),
    (&["opus-5"], claude(5.0, 25.0, 0.5)),
    (&["opus-4-8"], claude(5.0, 25.0, 0.5)),
    (&["opus-4-7"], claude(5.0, 25.0, 0.5)),
    (&["opus-4-6"], claude(5.0, 25.0, 0.5)),
    (&["opus-4-5"], claude(5.0, 25.0, 0.5)),
    (&["opus-4"], claude(15.0, 75.0, 1.5)),
    (&["3-opus"], claude(15.0, 75.0, 1.5)),
    (&["opus"], claude(5.0, 25.0, 0.5)),
    (&["sonnet-5-5"], claude(2.0, 10.0, 0.1)),
    (&["sonnet-5"], claude(2.0, 10.0, 0.2)),
    (&["sonnet"], claude(3.0, 15.0, 0.3)),
    (&["haiku-5-5"], claude(0.1, 0.5, 0.01)),
    (&["haiku-4"], claude(1.0, 5.0, 0.1)),
    (&["3-5-haiku"], claude(0.8, 4.0, 0.08)),
    (&["haiku-3-5"], claude(0.8, 4.0, 0.08)),
    (&["haiku"], claude(0.25, 1.25, 0.03)),
    // ---- OpenAI ----
    (&["gpt-6-1-sol"], oai_cw(2.0, 10.0, 0.1)),
    (&["gpt-6-sol"], oai_cw(2.0, 10.0, 0.2)),
    (&["gpt-6-astra"], oai_cw(10.0, 50.0, 1.0)),
    (&["gpt-6-luna"], oai_cw(0.1, 0.5, 0.01)),
    (&["gpt-5-6-sol"], oai_cw(2.0, 10.0, 0.2)),
    (&["gpt-5-6-terra"], oai_cw(2.0, 12.0, 0.2)),
    (&["gpt-5-6-luna"], oai_cw(0.2, 1.2, 0.02)),
    (&["gpt-5-5-pro"], flat(30.0, 180.0, 30.0)),
    (&["gpt-5-5"], flat(5.0, 30.0, 0.5)),
    (&["gpt-5-4-pro"], flat(30.0, 180.0, 30.0)),
    (&["gpt-5-4-mini"], flat(0.75, 4.5, 0.075)),
    (&["gpt-5-4-nano"], flat(0.2, 1.25, 0.02)),
    (&["gpt-5-4"], flat(2.5, 15.0, 0.25)),
    (&["gpt-5-3"], flat(1.75, 14.0, 0.175)),
    (&["gpt-5-2-pro"], flat(21.0, 168.0, 21.0)),
    (&["gpt-5-2"], flat(1.75, 14.0, 0.175)),
    (&["gpt-5-1-codex-mini"], flat(0.25, 2.0, 0.025)),
    (&["gpt-5-codex-mini"], flat(0.25, 2.0, 0.025)),
    (&["gpt-5-1"], flat(1.25, 10.0, 0.125)),
    (&["gpt-5-pro"], flat(15.0, 120.0, 15.0)),
    (&["gpt-5-mini"], flat(0.25, 2.0, 0.025)),
    (&["gpt-5-nano"], flat(0.05, 0.4, 0.005)),
    (&["gpt-5"], flat(1.25, 10.0, 0.125)),
    (&["codex-mini"], flat(1.5, 6.0, 0.375)),
    (&["gpt-4-1-mini"], flat(0.4, 1.6, 0.1)),
    (&["gpt-4-1"], flat(2.0, 8.0, 0.5)),
    (&["gpt-4o-mini"], flat(0.15, 0.6, 0.075)),
    (&["gpt-4o"], flat(2.5, 10.0, 1.25)),
    (&["o4-mini"], flat(1.1, 4.4, 0.275)),
    (&["o3"], flat(2.0, 8.0, 0.5)),
    // ---- DeepSeek（官方美元价）----
    (&["deepseek", "flash"], flat(0.14, 0.28, 0.0028)),
    (&["deepseek", "pro"], flat(1.74, 3.48, 0.0145)),
    (&["deepseek", "reasoner"], flat(0.55, 2.19, 0.14)),
    (&["deepseek", "r1"], flat(0.55, 2.19, 0.14)),
    (&["deepseek"], flat(0.27, 1.10, 0.07)),
    // ---- 智谱 GLM ----
    (&["glm", "flash"], flat(0.15, 0.5, 0.03)),
    (&["glm", "air"], flat(0.13, 0.85, 0.025)),
    (&["glm", "turbo"], flat(1.2, 4.0, 0.24)),
    (&["glm-5"], flat(1.0, 3.2, 0.2)),
    (&["glm"], flat(0.6, 2.2, 0.11)),
    // ---- Moonshot Kimi ----
    (&["kimi-k3"], flat(0.5, 13.5, 0.28)),
    (&["kimi"], flat(0.6, 2.5, 0.15)),
    // ---- 其他 ----
    (&["qwen", "coder"], flat(0.3, 1.0, 0.1)),
    (&["qwen"], flat(0.3, 1.2, 0.06)),
    (&["minimax"], flat(0.3, 1.2, 0.03)),
    (&["grok", "code"], flat(0.2, 1.5, 0.02)),
    (&["grok-4-5"], flat(2.0, 6.0, 0.3)),
    (&["grok-4-6"], flat(2.0, 6.0, 0.5)),
    (&["grok-4-7"], flat(2.0, 6.0, 0.5)),
    (&["grok"], flat(3.0, 15.0, 0.75)),
    (&["gemini", "flash-lite"], flat(0.3, 2.5, 0.03)),
    (&["gemini-3-5-flash"], flat(1.5, 9.0, 0.15)),
    (&["gemini", "flash"], flat(0.75, 3.75, 0.075)),
    (&["gemini-2-5-pro"], flat(1.25, 10.0, 0.125)),
    (&["gemini", "pro"], flat(2.0, 12.0, 0.2)),
];

/// 归一化模型名：小写、去 provider 前缀、`.`/`_` 统一为 `-`，
/// 使 `claude-opus-4.5`、`anthropic/claude-opus-4-5`、`GPT-5.4` 都能命中同一规则
pub fn normalize(model: &str) -> String {
    let m = model.trim().to_lowercase();
    let m = m.rsplit('/').next().unwrap_or(&m).to_string();
    m.replace(['.', '_', ' '], "-")
}

/// 片段必须落在词边界上：数字结尾的片段后面不能紧跟数字，字母结尾的片段后面不能紧跟字母
/// （开头同理）。否则 `gpt-5-2025-08-07`（GPT-5 带日期）会被误认成 `gpt-5-2`。
fn contains_token(m: &str, part: &str) -> bool {
    let same_class = |a: char, b: char| {
        (a.is_ascii_digit() && b.is_ascii_digit()) || (a.is_alphabetic() && b.is_alphabetic())
    };
    let (Some(first), Some(last)) = (part.chars().next(), part.chars().last()) else {
        return false;
    };
    m.match_indices(part).any(|(i, _)| {
        let before_ok = m[..i].chars().last().is_none_or(|c| !same_class(c, first));
        let after_ok = m[i + part.len()..]
            .chars()
            .next()
            .is_none_or(|c| !same_class(c, last));
        before_ok && after_ok
    })
}

pub fn lookup(model: &str) -> Option<Price> {
    let m = normalize(model);
    if m.is_empty() || m.starts_with('<') {
        // `<synthetic>` 等占位模型：不是真实请求
        return None;
    }
    RULES
        .iter()
        .find(|(parts, _)| parts.iter().all(|p| contains_token(&m, p)))
        .map(|(_, p)| *p)
}

/// 按模型计价；未知模型返回 None
pub fn cost(model: &str, t: &Tokens) -> Option<f64> {
    let p = lookup(model)?;
    let cw_5m = t.cache_write.saturating_sub(t.cache_write_1h);
    Some(
        (t.input as f64 * p.input
            + t.output as f64 * p.output
            + t.cache_read as f64 * p.cache_read
            + cw_5m as f64 * p.cache_write
            + t.cache_write_1h as f64 * p.cache_write_1h)
            / 1e6,
    )
}

/// Claude fast 模式的价格倍数（Opus 5 / 5.5 为 2×）
pub fn fast_multiplier(model: &str) -> f64 {
    let m = normalize(model);
    if m.contains("opus-5") {
        2.0
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(model: &str) -> (f64, f64, f64) {
        let p = lookup(model).unwrap_or_else(|| panic!("未定价：{model}"));
        (p.input, p.output, p.cache_read)
    }

    #[test]
    fn claude_generations_are_not_conflated() {
        assert_eq!(p("claude-opus-5-5"), (4.0, 20.0, 0.2));
        assert_eq!(p("claude-opus-5"), (5.0, 25.0, 0.5));
        assert_eq!(p("claude-opus-4-5-20251101"), (5.0, 25.0, 0.5));
        assert_eq!(p("claude-opus-4-1-20250805"), (15.0, 75.0, 1.5));
        assert_eq!(p("claude-opus-4-20250514"), (15.0, 75.0, 1.5));
        assert_eq!(p("claude-sonnet-4-5-20250929"), (3.0, 15.0, 0.3));
        assert_eq!(p("claude-sonnet-5"), (2.0, 10.0, 0.2));
        assert_eq!(p("claude-3-5-sonnet-20241022"), (3.0, 15.0, 0.3));
        assert_eq!(p("claude-haiku-4-5"), (1.0, 5.0, 0.1));
        assert_eq!(p("claude-3-5-haiku-20241022"), (0.8, 4.0, 0.08));
        assert_eq!(p("claude-fable-5-1"), (10.0, 50.0, 0.25));
        assert_eq!(p("anthropic/claude-opus-4.6"), (5.0, 25.0, 0.5));
    }

    #[test]
    fn openai_variants_pick_most_specific_rule() {
        assert_eq!(p("gpt-6.1-sol"), (2.0, 10.0, 0.1));
        assert_eq!(p("gpt-6-astra"), (10.0, 50.0, 1.0));
        assert_eq!(p("gpt-5.6-luna"), (0.2, 1.2, 0.02));
        assert_eq!(p("gpt-5.4-xhigh"), (2.5, 15.0, 0.25));
        assert_eq!(p("gpt-5.4-mini"), (0.75, 4.5, 0.075));
        assert_eq!(p("gpt-5.3-codex"), (1.75, 14.0, 0.175));
        assert_eq!(p("gpt-5-codex"), (1.25, 10.0, 0.125));
        assert_eq!(p("gpt-5-mini"), (0.25, 2.0, 0.025));
        // 带日期的 GPT-5 不能被当成 GPT-5.2
        assert_eq!(p("gpt-5-2025-08-07"), (1.25, 10.0, 0.125));
    }

    #[test]
    fn third_party_families() {
        assert_eq!(p("deepseek-flash"), (0.14, 0.28, 0.0028));
        assert_eq!(p("deepseek-v4-pro[1m]"), (1.74, 3.48, 0.0145));
        assert_eq!(p("GLM-5.3-Flash"), (0.15, 0.5, 0.03));
        assert_eq!(p("qwen3-coder-plus"), (0.3, 1.0, 0.1));
        assert!(lookup("deepseek-prover").is_some_and(|p| p.input == 0.27));
    }

    #[test]
    fn unknown_and_synthetic_models_are_unpriced() {
        assert!(lookup("<synthetic>").is_none());
        assert!(lookup("").is_none());
        assert!(lookup("codex-auto-review").is_none());
        assert!(lookup("some-local-llama").is_none());
    }

    #[test]
    fn cache_write_ttl_is_priced_separately() {
        // opus-5-5：1h 写入 2×$4，5m 写入 1.25×$4
        let t = Tokens {
            cache_write: 3_000_000,
            cache_write_1h: 1_000_000,
            ..Default::default()
        };
        let c = cost("claude-opus-5-5", &t).unwrap();
        assert!((c - (2.0 * 5.0 + 1.0 * 8.0)).abs() < 1e-9, "cost = {c}");
    }
}
