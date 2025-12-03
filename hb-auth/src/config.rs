use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct AuthConfig {
    pub team_domain: Arc<String>,
    pub audience: Arc<String>,
}

impl AuthConfig {
    pub fn new<S: Into<String>>(team_domain: S, audience: S) -> Self {
        Self {
            team_domain: Arc::new(team_domain.into()),
            audience: Arc::new(audience.into()),
        }
    }

    pub(crate) fn issuer(&self) -> String {
        self.team_domain.to_string()
    }

    pub fn team_name(&self) -> String {
        let domain = self
            .team_domain
            .strip_prefix("https://")
            .or_else(|| self.team_domain.strip_prefix("http://"))
            .unwrap_or(&self.team_domain);

        domain.split('.').next().unwrap_or(domain).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_config_new() {
        let config = AuthConfig::new("https://myteam.cloudflareaccess.com", "aud123");
        assert_eq!(&*config.team_domain, "https://myteam.cloudflareaccess.com");
        assert_eq!(&*config.audience, "aud123");
    }

    #[test]
    fn test_auth_config_issuer() {
        let config = AuthConfig::new("https://myteam.cloudflareaccess.com", "aud123");
        assert_eq!(config.issuer(), "https://myteam.cloudflareaccess.com");
    }

    #[test]
    fn test_team_name_with_https() {
        let config = AuthConfig::new("https://myteam.cloudflareaccess.com", "aud");
        assert_eq!(config.team_name(), "myteam");
    }

    #[test]
    fn test_team_name_with_http() {
        let config = AuthConfig::new("http://myteam.cloudflareaccess.com", "aud");
        assert_eq!(config.team_name(), "myteam");
    }

    #[test]
    fn test_team_name_without_protocol() {
        let config = AuthConfig::new("myteam.cloudflareaccess.com", "aud");
        assert_eq!(config.team_name(), "myteam");
    }
}
