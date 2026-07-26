/// Structured errors for the `web_fetch` tool.

#[derive(Debug, thiserror::Error)]
pub enum WebFetchError {
    #[error("URL exceeds maximum length of {max} characters")]
    UrlTooLong { max: usize },

    #[error("unsupported URL scheme: {scheme} (only http/https allowed)")]
    UnsupportedScheme { scheme: String },

    #[error("invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),

    #[error("failed to build HTTP client: {0}")]
    ClientBuildError(reqwest::Error),

    #[error("HTTP request failed: {0}")]
    HttpRequest(#[from] reqwest::Error),

    #[error("invalid redirect URL: {0}")]
    InvalidRedirect(String),

    #[error("too many redirects (max {max})")]
    TooManyRedirects { max: usize },

    #[error("response body exceeds maximum size of {max} bytes")]
    ResponseTooLarge { max: usize },

    #[error("invalid proxy configuration: {0}")]
    ProxyConfigError(String),

    #[error("failed to save downloaded file: {0}")]
    IoError(#[from] std::io::Error),

    #[error("unsupported content type {content_type} from {url}")]
    UnsupportedContentType { content_type: String, url: String },

    #[error("content body does not match claimed content type {content_type} from {url}")]
    ContentTypeMismatch { content_type: String, url: String },
}
