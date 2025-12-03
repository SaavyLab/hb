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
