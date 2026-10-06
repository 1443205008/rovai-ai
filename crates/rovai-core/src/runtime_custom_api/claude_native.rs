//! Native identity observations for the existing status display, never execution admission.
use serde_json::Value;

/// Native identity fields, shared by auth status and the existing initialize
/// response. Human-facing diagnostic sections are never authentication evidence.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Identity {
    Official,
    Api(String),
    SignedOut,
    ThirdParty,
    #[default]
    Unknown,
}
impl Identity {
    pub fn from_auth_status(value: &Value) -> Self {
        if value["apiProvider"]
            .as_str()
            .is_some_and(|v| v != "firstParty")
        {
            return Self::ThirdParty;
        }
        match (value["loggedIn"].as_bool(), value["authMethod"].as_str()) {
            (Some(false), Some("none")) => Self::SignedOut,
            (Some(true), Some("claude.ai" | "oauth_token")) => Self::Official,
            (Some(true), Some("api_key" | "api_key_helper")) => value["apiKeySource"]
                .as_str()
                .map(|source| Self::Api(source.into()))
                .unwrap_or(Self::Unknown),
            (Some(true), Some("third_party")) => Self::ThirdParty,
            _ => Self::Unknown,
        }
    }
    pub fn from_initialize(value: &Value) -> Self {
        let account = &value["account"];
        if account["apiProvider"]
            .as_str()
            .is_some_and(|v| v != "firstParty")
        {
            return Self::ThirdParty;
        }
        match account["tokenSource"].as_str() {
            Some(
                "claude.ai"
                | "CLAUDE_CODE_OAUTH_TOKEN"
                | "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR"
                | "CCR_OAUTH_TOKEN_FILE",
            ) => Self::Official,
            Some("ANTHROPIC_AUTH_TOKEN" | "apiKeyHelper") => {
                Self::Api(account["tokenSource"].as_str().unwrap().into())
            }
            // Native subscription auth omits tokenSource and supplies subscriptionType.
            // Its value can be null; email alone is never login evidence.
            None if account.get("subscriptionType").is_some() => Self::Official,
            Some("none") | None => match account["apiKeySource"].as_str() {
                Some(source) if source != "none" => Self::Api(source.into()),
                _ if account["tokenSource"] == "none" => Self::SignedOut,
                _ => Self::Unknown,
            },
            Some(source) => Self::Api(source.into()),
        }
    }
    pub fn official_login_status(&self) -> Option<&'static str> {
        match self {
            Self::Official => Some("signed_in"),
            Self::SignedOut => Some("signed_out"),
            _ => None,
        }
    }
}
