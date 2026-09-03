use serde::Serialize;

use super::model::TokenUsageBreakdown;
use super::model::UsageAttribution;

const NANOS_PER_UNIT: u128 = 1_000_000_000;
const TOKENS_PER_MILLION: u128 = 1_000_000;

pub const CODEX_CREDIT_RATE_CARD_ID: &str = "openai-codex-credits-2026-08-13";
pub const API_EQUIVALENT_RATE_CARD_ID: &str = "openai-api-standard-2026-08-13";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UsageRateCard {
    pub id: String,
    pub unit: String,
    pub description: String,
    pub source_urls: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DerivedUsageEstimate {
    pub rate_card_id: String,
    /// Decimal string so JSON consumers do not lose precision to binary floats.
    pub amount: String,
    pub priced_samples: u64,
    pub unpriced_samples: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct SampleEstimates {
    pub codex_credit_nanos: Option<u128>,
    pub api_equivalent_usd_nanos: Option<u128>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct EstimateAccumulator {
    amount_nanos: u128,
    priced_samples: u64,
    unpriced_samples: u64,
}

impl EstimateAccumulator {
    pub fn add(&mut self, amount_nanos: Option<u128>) -> anyhow::Result<()> {
        match amount_nanos {
            Some(amount_nanos) => {
                self.amount_nanos = self
                    .amount_nanos
                    .checked_add(amount_nanos)
                    .ok_or_else(|| anyhow::anyhow!("derived usage estimate overflow"))?;
                self.priced_samples = self
                    .priced_samples
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("priced usage sample count overflow"))?;
            }
            None => {
                self.unpriced_samples = self
                    .unpriced_samples
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("unpriced usage sample count overflow"))?;
            }
        }
        Ok(())
    }

    pub fn finish(&self, rate_card_id: &str) -> DerivedUsageEstimate {
        DerivedUsageEstimate {
            rate_card_id: rate_card_id.to_string(),
            amount: format_nanos(self.amount_nanos),
            priced_samples: self.priced_samples,
            unpriced_samples: self.unpriced_samples,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TokenRates {
    uncached_input: u64,
    cached_input: u64,
    cache_write_input: u64,
    output: u64,
}

#[derive(Debug, Clone, Copy)]
struct Multiplier {
    numerator: u64,
    denominator: u64,
}

impl Multiplier {
    const STANDARD: Self = Self {
        numerator: 1,
        denominator: 1,
    };
}

pub(super) fn rate_cards() -> Vec<UsageRateCard> {
    vec![
        UsageRateCard {
            id: CODEX_CREDIT_RATE_CARD_ID.to_string(),
            unit: "credit".to_string(),
            description: "Published Codex token credits; cache writes are excluded and Fast multipliers are applied when identified.".to_string(),
            source_urls: vec![
                "https://help.openai.com/en/articles/20001106-codex-rate-card".to_string(),
                "https://learn.chatgpt.com/docs/agent-configuration/speed".to_string(),
            ],
        },
        UsageRateCard {
            id: API_EQUIVALENT_RATE_CARD_ID.to_string(),
            unit: "USD".to_string(),
            description: "GPT-5.6 API-equivalent estimate; unknown models, tiers, and long-context samples remain unpriced.".to_string(),
            source_urls: vec![
                "https://openai.com/api/pricing/".to_string(),
                "https://learn.chatgpt.com/docs/agent-configuration/speed".to_string(),
            ],
        },
    ]
}

pub(super) fn derive_sample_estimates(
    attribution: &UsageAttribution,
    tokens: &TokenUsageBreakdown,
) -> SampleEstimates {
    if !is_openai_provider(attribution.provider.as_deref()) {
        return SampleEstimates::default();
    }
    let Some(model) = attribution.model.as_deref().map(normalize_model) else {
        return SampleEstimates::default();
    };
    SampleEstimates {
        codex_credit_nanos: codex_credit_rates(&model)
            .and_then(|rates| {
                codex_multiplier(&model, attribution.service_tier.as_deref())
                    .map(|multiplier| (rates, multiplier))
            })
            .and_then(|(rates, multiplier)| derive_amount_nanos(tokens, rates, multiplier)),
        api_equivalent_usd_nanos: api_rates(&model)
            .and_then(|rates| {
                api_multiplier(attribution.service_tier.as_deref())
                    .map(|multiplier| (rates, multiplier))
            })
            .and_then(|(rates, multiplier)| {
                (tokens.input_tokens <= 272_000)
                    .then(|| derive_amount_nanos(tokens, rates, multiplier))
                    .flatten()
            }),
    }
}

fn is_openai_provider(provider: Option<&str>) -> bool {
    provider.is_none_or(|provider| {
        let provider = provider.trim().to_ascii_lowercase();
        provider == "chatgpt" || provider == "codex" || provider.starts_with("openai")
    })
}

fn normalize_model(model: &str) -> String {
    model.trim().to_ascii_lowercase()
}

fn codex_credit_rates(model: &str) -> Option<TokenRates> {
    let rates = match model {
        "gpt-5.6-sol" => TokenRates::new(125_000_000_000, 12_500_000_000, 750_000_000_000),
        "gpt-5.6-terra" => TokenRates::new(50_000_000_000, 5_000_000_000, 300_000_000_000),
        "gpt-5.6-luna" => TokenRates::new(5_000_000_000, 500_000_000, 30_000_000_000),
        "gpt-5.5" => TokenRates::new(125_000_000_000, 12_500_000_000, 750_000_000_000),
        "gpt-5.5-cyber" | "gpt-5.5-codex-cyber" => {
            TokenRates::new(312_500_000_000, 31_250_000_000, 1_875_000_000_000)
        }
        "gpt-5.4" => TokenRates::new(62_500_000_000, 6_250_000_000, 375_000_000_000),
        "gpt-5.4-mini" => TokenRates::new(18_750_000_000, 1_875_000_000, 113_000_000_000),
        "gpt-5.3-codex" => TokenRates::new(43_750_000_000, 4_375_000_000, 350_000_000_000),
        "gpt-5.2" => TokenRates::new(43_750_000_000, 4_375_000_000, 350_000_000_000),
        _ => return None,
    };
    Some(rates)
}

fn api_rates(model: &str) -> Option<TokenRates> {
    let rates = match model {
        "gpt-5.6-sol" => {
            TokenRates::with_cache_write(5_000_000_000, 500_000_000, 6_250_000_000, 30_000_000_000)
        }
        "gpt-5.6-terra" => {
            TokenRates::with_cache_write(2_500_000_000, 250_000_000, 3_125_000_000, 15_000_000_000)
        }
        "gpt-5.6-luna" => {
            TokenRates::with_cache_write(1_000_000_000, 100_000_000, 1_250_000_000, 6_000_000_000)
        }
        _ => return None,
    };
    Some(rates)
}

impl TokenRates {
    const fn new(uncached_input: u64, cached_input: u64, output: u64) -> Self {
        Self::with_cache_write(uncached_input, cached_input, 0, output)
    }

    const fn with_cache_write(
        uncached_input: u64,
        cached_input: u64,
        cache_write_input: u64,
        output: u64,
    ) -> Self {
        Self {
            uncached_input,
            cached_input,
            cache_write_input,
            output,
        }
    }
}

fn codex_multiplier(model: &str, service_tier: Option<&str>) -> Option<Multiplier> {
    match normalize_tier(service_tier).as_deref() {
        None | Some("default") => Some(Multiplier::STANDARD),
        Some("fast") => match model {
            "gpt-5.6-sol"
            | "gpt-5.6-terra"
            | "gpt-5.6-luna"
            | "gpt-5.5"
            | "gpt-5.5-cyber"
            | "gpt-5.5-codex-cyber" => Some(Multiplier {
                numerator: 5,
                denominator: 2,
            }),
            "gpt-5.4" => Some(Multiplier {
                numerator: 2,
                denominator: 1,
            }),
            _ => None,
        },
        Some(_) => None,
    }
}

fn api_multiplier(service_tier: Option<&str>) -> Option<Multiplier> {
    match normalize_tier(service_tier).as_deref() {
        None | Some("default") => Some(Multiplier::STANDARD),
        Some("fast" | "priority") => Some(Multiplier {
            numerator: 2,
            denominator: 1,
        }),
        Some(_) => None,
    }
}

fn normalize_tier(service_tier: Option<&str>) -> Option<String> {
    service_tier
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn derive_amount_nanos(
    tokens: &TokenUsageBreakdown,
    rates: TokenRates,
    multiplier: Multiplier,
) -> Option<u128> {
    let accounted_input = tokens
        .cached_input_tokens
        .checked_add(tokens.cache_write_input_tokens)?;
    let uncached_input = tokens.input_tokens.checked_sub(accounted_input)?;
    let weighted = u128::from(uncached_input)
        .checked_mul(u128::from(rates.uncached_input))?
        .checked_add(
            u128::from(tokens.cached_input_tokens).checked_mul(u128::from(rates.cached_input))?,
        )?
        .checked_add(
            u128::from(tokens.cache_write_input_tokens)
                .checked_mul(u128::from(rates.cache_write_input))?,
        )?
        .checked_add(u128::from(tokens.output_tokens).checked_mul(u128::from(rates.output))?)?
        .checked_mul(u128::from(multiplier.numerator))?;
    weighted.checked_div(TOKENS_PER_MILLION.checked_mul(u128::from(multiplier.denominator))?)
}

fn format_nanos(value: u128) -> String {
    let whole = value / NANOS_PER_UNIT;
    let remainder = value % NANOS_PER_UNIT;
    if remainder == 0 {
        return whole.to_string();
    }
    let mut fraction = format!("{remainder:09}");
    while fraction.ends_with('0') {
        fraction.pop();
    }
    format!("{whole}.{fraction}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terra(service_tier: Option<&str>) -> UsageAttribution {
        UsageAttribution {
            provider: Some("openai".to_string()),
            model: Some("gpt-5.6-terra".to_string()),
            reasoning_effort: Some("xhigh".to_string()),
            service_tier: service_tier.map(str::to_string),
        }
    }

    fn mixed_tokens() -> TokenUsageBreakdown {
        TokenUsageBreakdown {
            total_tokens: 1_100_000,
            input_tokens: 1_000_000,
            cached_input_tokens: 200_000,
            cache_write_input_tokens: 100_000,
            output_tokens: 100_000,
            reasoning_output_tokens: 40_000,
        }
    }

    #[test]
    fn standard_terra_rates_keep_reasoning_as_an_output_subset() {
        let estimates = derive_sample_estimates(&terra(None), &mixed_tokens());
        assert_eq!(
            estimates.codex_credit_nanos.map(format_nanos),
            Some("66".to_string())
        );
        assert_eq!(
            estimates.api_equivalent_usd_nanos.map(format_nanos),
            None,
            "input above 272K remains unpriced until long-context tiers are modeled"
        );

        let mut tokens = mixed_tokens();
        tokens.input_tokens = 200_000;
        tokens.cached_input_tokens = 40_000;
        tokens.cache_write_input_tokens = 20_000;
        tokens.output_tokens = 20_000;
        tokens.reasoning_output_tokens = 8_000;
        tokens.total_tokens = 220_000;
        let estimates = derive_sample_estimates(&terra(None), &tokens);
        assert_eq!(
            estimates.codex_credit_nanos.map(format_nanos),
            Some("13.2".to_string())
        );
        assert_eq!(
            estimates.api_equivalent_usd_nanos.map(format_nanos),
            Some("0.7225".to_string())
        );
    }

    #[test]
    fn fast_tier_applies_published_credit_and_priority_multipliers() {
        let mut tokens = mixed_tokens();
        tokens.input_tokens = 200_000;
        tokens.cached_input_tokens = 40_000;
        tokens.cache_write_input_tokens = 20_000;
        tokens.output_tokens = 20_000;
        tokens.total_tokens = 220_000;
        let estimates = derive_sample_estimates(&terra(Some("fast")), &tokens);
        assert_eq!(
            estimates.codex_credit_nanos.map(format_nanos),
            Some("33".to_string())
        );
        assert_eq!(
            estimates.api_equivalent_usd_nanos.map(format_nanos),
            Some("1.445".to_string())
        );
    }

    #[test]
    fn unknown_provider_model_or_invalid_token_partition_is_unpriced() {
        let mut attribution = terra(None);
        attribution.provider = Some("deepseek".to_string());
        assert_eq!(
            derive_sample_estimates(&attribution, &mixed_tokens()),
            SampleEstimates::default()
        );

        attribution.provider = Some("openai".to_string());
        attribution.model = Some("future-model".to_string());
        assert_eq!(
            derive_sample_estimates(&attribution, &mixed_tokens()),
            SampleEstimates::default()
        );

        attribution.model = Some("gpt-5.6-terra".to_string());
        let mut invalid = mixed_tokens();
        invalid.cached_input_tokens = invalid.input_tokens;
        invalid.cache_write_input_tokens = 1;
        assert_eq!(
            derive_sample_estimates(&attribution, &invalid),
            SampleEstimates::default()
        );
    }
}
