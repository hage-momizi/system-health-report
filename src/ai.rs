use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use tracing::info;

use crate::config::Config;

#[async_trait]
pub trait AiProvider: Send + Sync {
    async fn analyze(&self, prompt: &str) -> Result<String>;
}

pub struct OpenRouterProvider {
    client: Client,
    api_key: String,
    model: String,
}

pub struct OpenAiProvider {
    client: Client,
    api_key: String,
    model: String,
}

pub struct AnthropicProvider {
    client: Client,
    api_key: String,
    model: String,
}

#[async_trait]
impl AiProvider for OpenRouterProvider {
    async fn analyze(&self, prompt: &str) -> Result<String> {
        info!("Sending prompt to OpenRouter (model: {})", self.model);

        let body = json!({
            "model": self.model,
            "messages": [
                {
                    "role": "user",
                    "content": prompt
                }
            ]
        });

        let response = self
            .client
            .post("https://openrouter.ai/api/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .context("Failed to send request to OpenRouter")?;

        let status = response.status();
        let response_text = response.text().await.context("Failed to read response body")?;

        if !status.is_success() {
            return Err(anyhow!(
                "OpenRouter API error {}: {}",
                status,
                response_text
            ));
        }

        let json: Value = serde_json::from_str(&response_text)
            .context("Failed to parse OpenRouter response")?;

        extract_openai_content(&json)
    }
}

#[async_trait]
impl AiProvider for OpenAiProvider {
    async fn analyze(&self, prompt: &str) -> Result<String> {
        info!("Sending prompt to OpenAI (model: {})", self.model);

        let body = json!({
            "model": self.model,
            "messages": [
                {
                    "role": "user",
                    "content": prompt
                }
            ]
        });

        let response = self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .context("Failed to send request to OpenAI")?;

        let status = response.status();
        let response_text = response.text().await.context("Failed to read response body")?;

        if !status.is_success() {
            return Err(anyhow!("OpenAI API error {}: {}", status, response_text));
        }

        let json: Value =
            serde_json::from_str(&response_text).context("Failed to parse OpenAI response")?;

        extract_openai_content(&json)
    }
}

#[async_trait]
impl AiProvider for AnthropicProvider {
    async fn analyze(&self, prompt: &str) -> Result<String> {
        info!("Sending prompt to Anthropic (model: {})", self.model);

        let body = json!({
            "model": self.model,
            "max_tokens": 4096,
            "messages": [
                {
                    "role": "user",
                    "content": prompt
                }
            ]
        });

        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .context("Failed to send request to Anthropic")?;

        let status = response.status();
        let response_text = response.text().await.context("Failed to read response body")?;

        if !status.is_success() {
            return Err(anyhow!(
                "Anthropic API error {}: {}",
                status,
                response_text
            ));
        }

        let json: Value =
            serde_json::from_str(&response_text).context("Failed to parse Anthropic response")?;

        // Anthropic response format: { "content": [{ "type": "text", "text": "..." }] }
        json["content"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|item| item["text"].as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("Could not extract text from Anthropic response"))
    }
}

fn extract_openai_content(json: &Value) -> Result<String> {
    json["choices"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|choice| choice["message"]["content"].as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("Could not extract content from API response"))
}

pub fn create_provider(config: &Config) -> Result<Box<dyn AiProvider>> {
    let client = Client::new();
    let providers = &config.ai_assistant.providers;

    match config.ai_assistant.active_provider.as_str() {
        "openrouter" => {
            let provider_config = providers
                .openrouter
                .as_ref()
                .ok_or_else(|| anyhow!("OpenRouter config not found"))?;
            Ok(Box::new(OpenRouterProvider {
                client,
                api_key: provider_config.api_key.clone(),
                model: provider_config.model.clone(),
            }))
        }
        "openai" => {
            let provider_config = providers
                .openai
                .as_ref()
                .ok_or_else(|| anyhow!("OpenAI config not found"))?;
            Ok(Box::new(OpenAiProvider {
                client,
                api_key: provider_config.api_key.clone(),
                model: provider_config.model.clone(),
            }))
        }
        "anthropic" => {
            let provider_config = providers
                .anthropic
                .as_ref()
                .ok_or_else(|| anyhow!("Anthropic config not found"))?;
            Ok(Box::new(AnthropicProvider {
                client,
                api_key: provider_config.api_key.clone(),
                model: provider_config.model.clone(),
            }))
        }
        provider => Err(anyhow!("Unknown AI provider: {}", provider)),
    }
}
