use std::{fmt, str::FromStr};

use anyhow::{Context, anyhow};
use url::Url;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProxyUrl {
    inner: Url,
}

impl ProxyUrl {
    pub fn scheme(&self) -> &str {
        self.inner.scheme()
    }

    pub fn host(&self) -> Option<&str> {
        self.inner.host_str()
    }

    pub fn port(&self) -> Option<u16> {
        self.inner.port_or_known_default()
    }
}

impl FromStr for ProxyUrl {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let url = Url::parse(value).with_context(|| format!("invalid proxy URL: {value:?}"))?;
        match url.scheme() {
            "socks5" | "http" => {}
            scheme => return Err(anyhow!("unsupported proxy scheme: {scheme}")),
        }
        if url.host_str().is_none() {
            return Err(anyhow!("proxy URL must include a host"));
        }
        if url.port_or_known_default().is_none() {
            return Err(anyhow!("proxy URL must include a port"));
        }
        Ok(Self { inner: url })
    }
}

impl fmt::Display for ProxyUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_supported_proxy_urls() {
        assert!("socks5://127.0.0.1:1080".parse::<ProxyUrl>().is_ok());
        assert!(
            "http://user:pass@localhost:8080"
                .parse::<ProxyUrl>()
                .is_ok()
        );
    }

    #[test]
    fn rejects_unsupported_proxy_urls() {
        assert!("https://127.0.0.1:443".parse::<ProxyUrl>().is_err());
        assert!("socks5:///missing-host".parse::<ProxyUrl>().is_err());
    }
}
