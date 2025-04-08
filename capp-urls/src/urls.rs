use crate::middleware::Middleware;
use url::Url;

/// A collection of URLs with operations for manipulating them through middleware
#[derive(Debug, Clone)]
pub struct UrlList {
    urls: Vec<Url>,
}

impl UrlList {
    /// Create a new UrlList from a vector of URLs
    pub fn new(urls: Vec<Url>) -> Self {
        Self { urls }
    }

    /// Apply a middleware to process the URLs
    pub fn apply<M: Middleware>(mut self, middleware: &M) -> Self {
        self.urls = middleware.process(self.urls);
        self
    }

    /// Get a reference to the underlying URLs
    pub fn urls(&self) -> &[Url] {
        &self.urls
    }

    /// Get the number of URLs in the list
    pub fn len(&self) -> usize {
        self.urls.len()
    }

    /// Check if the list is empty
    pub fn is_empty(&self) -> bool {
        self.urls.is_empty()
    }
}
