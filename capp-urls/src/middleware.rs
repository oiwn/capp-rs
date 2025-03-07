use regex::Regex;
use std::collections::HashSet;
use url::Url;

/// Trait that defines middleware for processing lists of URLs
pub trait Middleware {
    /// Process a list of URLs and return a modified list
    fn process(&self, urls: Vec<Url>) -> Vec<Url>;
}

/// Middleware that removes duplicate URLs from a list
#[derive(Debug)]
pub struct DeduplicateMiddleware;

impl Middleware for DeduplicateMiddleware {
    fn process(&self, urls: Vec<Url>) -> Vec<Url> {
        let mut seen = HashSet::new();
        urls.into_iter()
            .filter(|url| seen.insert(url.as_str().to_string()))
            .collect()
    }
}

// StringPatternMiddleware
pub struct StringPatternMiddleware {
    patterns: Vec<String>,
    include: bool,
}

impl StringPatternMiddleware {
    pub fn new(patterns: Vec<String>, include: bool) -> Self {
        Self { patterns, include }
    }
}

impl Middleware for StringPatternMiddleware {
    fn process(&self, urls: Vec<Url>) -> Vec<Url> {
        urls.into_iter()
            .filter(|url| {
                let url_str = url.as_str();
                let contains_pattern = self
                    .patterns
                    .iter()
                    .any(|pattern| url_str.contains(pattern));
                if self.include {
                    contains_pattern
                } else {
                    !contains_pattern
                }
            })
            .collect()
    }
}

/// Middleware that filters URLs based on a regex pattern
#[derive(Debug)]
pub struct RegexFilterMiddleware {
    patterns: Vec<Regex>,
    include: bool,
}

impl RegexFilterMiddleware {
    /// Create a new pattern filter middleware
    ///
    /// # Arguments
    /// * `patterns` - A list of regex patterns to match against URLs
    /// * `include` - If true, include matching URLs; if false, exclude matching URLs
    pub fn new(patterns: Vec<&str>, include: bool) -> Result<Self, regex::Error> {
        let compiled_patterns = patterns
            .into_iter()
            .map(Regex::new)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            patterns: compiled_patterns,
            include,
        })
    }
}

impl Middleware for RegexFilterMiddleware {
    fn process(&self, urls: Vec<Url>) -> Vec<Url> {
        urls.into_iter()
            .filter(|url| {
                let url_str = url.as_str();
                let matches = self
                    .patterns
                    .iter()
                    .any(|pattern| pattern.is_match(url_str));
                if self.include { matches } else { !matches }
            })
            .collect()
    }
}

/// Middleware that filters URLs based on whether they belong to the same site as a base URL
#[derive(Debug)]
pub struct SiteFilterMiddleware {
    base_url: Url,
    same_site: bool,
}

impl SiteFilterMiddleware {
    /// Create a new site filter middleware
    ///
    /// # Arguments
    /// * `base_url` - The base URL to compare against
    /// * `same_site` - If true, include URLs on the same site; if false, exclude them
    pub fn new(base_url: Url, same_site: bool) -> Self {
        Self {
            base_url,
            same_site,
        }
    }

    /// Create a middleware that keeps only URLs on the same site as the base URL
    pub fn on_site(base_url: Url) -> Self {
        Self::new(base_url, true)
    }

    /// Create a middleware that keeps only URLs on different sites from the base URL
    pub fn off_site(base_url: Url) -> Self {
        Self::new(base_url, false)
    }
}

impl Middleware for SiteFilterMiddleware {
    fn process(&self, urls: Vec<Url>) -> Vec<Url> {
        let base_host = self.base_url.host_str();

        urls.into_iter()
            .filter(|url| {
                let url_host = url.host_str();
                let same_site = base_host == url_host;

                if self.same_site {
                    same_site
                } else {
                    !same_site
                }
            })
            .collect()
    }
}
