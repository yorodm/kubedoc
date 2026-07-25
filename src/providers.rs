use rig_core::client::{CompletionClient, Nothing};

use crate::config::KubedocConfig;

pub fn openai_completion(
    config: &KubedocConfig,
) -> Result<rig_core::providers::openai::completion::CompletionModel, Box<dyn std::error::Error>> {
    let api_key_env = config
        .llm
        .api_key_env
        .as_deref()
        .unwrap_or("OPENAI_API_KEY");
    let api_key = std::env::var(api_key_env).map_err(|_| format!("{api_key_env} not set"))?;

    let mut builder = rig_core::providers::openai::CompletionsClient::builder().api_key(&api_key);

    if let Some(ref base_url) = config.llm.base_url
        && !base_url.is_empty() {
            builder = builder.base_url(base_url);
        }

    let client = builder.build()?;
    Ok(client.completion_model(&config.llm.model))
}

pub fn anthropic_completion(
    config: &KubedocConfig,
) -> Result<rig_core::providers::anthropic::completion::CompletionModel, Box<dyn std::error::Error>>
{
    let api_key_env = config
        .llm
        .api_key_env
        .as_deref()
        .unwrap_or("ANTHROPIC_API_KEY");
    let api_key = std::env::var(api_key_env).map_err(|_| format!("{api_key_env} not set"))?;

    let mut builder = rig_core::providers::anthropic::Client::builder().api_key(api_key);

    if let Some(ref base_url) = config.llm.base_url
        && !base_url.is_empty() {
            builder = builder.base_url(base_url);
        }

    let client = builder.build()?;
    Ok(client.completion_model(&config.llm.model))
}

pub fn groq_completion(
    config: &KubedocConfig,
) -> Result<rig_core::providers::groq::CompletionModel, Box<dyn std::error::Error>> {
    let api_key_env = config.llm.api_key_env.as_deref().unwrap_or("GROQ_API_KEY");
    let api_key = std::env::var(api_key_env).map_err(|_| format!("{api_key_env} not set"))?;

    let mut builder = rig_core::providers::groq::Client::builder().api_key(&api_key);

    if let Some(ref base_url) = config.llm.base_url
        && !base_url.is_empty() {
            builder = builder.base_url(base_url);
        }

    let client = builder.build()?;
    Ok(client.completion_model(&config.llm.model))
}

pub fn ollama_completion(
    config: &KubedocConfig,
) -> Result<rig_core::providers::ollama::CompletionModel, Box<dyn std::error::Error>> {
    let mut builder = rig_core::providers::ollama::Client::builder().api_key(Nothing);

    if let Some(ref base_url) = config.llm.base_url
        && !base_url.is_empty() {
            builder = builder.base_url(base_url);
        }

    let client = builder.build()?;
    Ok(client.completion_model(&config.llm.model))
}
