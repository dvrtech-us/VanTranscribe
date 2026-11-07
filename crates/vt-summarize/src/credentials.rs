//! The only credential persistence boundary. Never serialize secrets into configuration.
use crate::{Provider, Result, SummaryConfig, SummaryError, providers};
fn account(config: &SummaryConfig) -> Result<String> {
    if !matches!(
        config.provider,
        Provider::Local | Provider::OpenAi | Provider::Anthropic | Provider::Compatible
    ) {
        return Err(SummaryError::Config(
            "this provider does not use a credential",
        ));
    }
    let url = providers::endpoint(config)?;
    Ok(format!(
        "{}|{}",
        config.provider.as_str(),
        url.origin().ascii_serialization()
    ))
}
#[cfg(target_os = "macos")]
const SERVICE: &str = "com.vantranscribe.summary-provider";
pub fn secret_set(config: &SummaryConfig, secret: &str) -> Result<()> {
    let name = account(config)?;
    if secret.trim().is_empty() || secret.len() > 8192 || secret.contains(['\r', '\n']) {
        return Err(SummaryError::Config("invalid credential"));
    }
    #[cfg(target_os = "macos")]
    {
        security_framework::passwords::set_generic_password(SERVICE, &name, secret.as_bytes())
            .map_err(|_| SummaryError::Credential)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (name, secret);
        Err(SummaryError::Credential)
    }
}
pub fn secret_get(config: &SummaryConfig) -> Result<Option<String>> {
    let name = account(config)?;
    #[cfg(target_os = "macos")]
    {
        match security_framework::passwords::get_generic_password(SERVICE, &name) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| SummaryError::Credential),
            Err(error) if error.code() == -25300 => Ok(None),
            Err(_) => Err(SummaryError::Credential),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = name;
        Err(SummaryError::Credential)
    }
}
pub fn secret_present(config: &SummaryConfig) -> Result<bool> {
    Ok(secret_get(config)?.is_some())
}
pub fn secret_delete(config: &SummaryConfig) -> Result<()> {
    let name = account(config)?;
    #[cfg(target_os = "macos")]
    {
        match security_framework::passwords::delete_generic_password(SERVICE, &name) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == -25300 => Ok(()),
            Err(_) => Err(SummaryError::Credential),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = name;
        Err(SummaryError::Credential)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scope_includes_provider_and_exact_origin() {
        let mut config = SummaryConfig {
            provider: Provider::Compatible,
            endpoint: Some("https://example.invalid/v1".into()),
            ..Default::default()
        };
        let first = account(&config).unwrap();
        config.endpoint = Some("https://example.invalid:444/v1".into());
        assert_ne!(first, account(&config).unwrap());
        config.endpoint = Some("https://other.invalid/v1".into());
        assert_ne!(first, account(&config).unwrap());
        config.endpoint = Some("https://example.invalid/v1".into());
        config.provider = Provider::OpenAi;
        assert_ne!(first, account(&config).unwrap());
    }
}
