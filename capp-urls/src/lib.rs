//! URL list processing middleware and utilities.
//!
//! This crate provides tools for working with collections of URLs,
//! allowing them to be filtered, deduplicated, and processed through
//! a middleware system.

pub mod middleware;
pub mod urls;

pub use middleware::{
    DeduplicateMiddleware, Middleware, RegexFilterMiddleware, SiteFilterMiddleware,
    StringPatternMiddleware,
};
pub use urls::UrlList;

// Re-export
pub use url;

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    fn make_test_urls() -> Vec<Url> {
        vec![
            Url::parse("https://example.com/page1").unwrap(),
            Url::parse("https://example.com/page2").unwrap(),
            Url::parse("https://example.com/page1").unwrap(), // Duplicate
            Url::parse("https://example.org/page1").unwrap(),
            Url::parse("https://subdomain.example.com/page1").unwrap(),
        ]
    }

    #[test]
    fn test_url_list_creation() {
        let urls = make_test_urls();
        let url_list = UrlList::new(urls.clone());

        assert_eq!(url_list.len(), 5);
        assert_eq!(url_list.urls(), urls.as_slice());
    }

    #[test]
    fn test_deduplicate_middleware() {
        let urls = make_test_urls();
        let url_list = UrlList::new(urls);

        let deduplicated = url_list.apply(&DeduplicateMiddleware);

        assert_eq!(deduplicated.len(), 4); // One duplicate removed
    }

    #[test]
    fn test_pattern_filter_middleware() {
        let urls = make_test_urls();
        let url_list = UrlList::new(urls);

        // Include only URLs with "example.com"
        let pattern_filter =
            RegexFilterMiddleware::new(vec!["example\\.com"], true).unwrap();
        let filtered = url_list.clone().apply(&pattern_filter);

        assert_eq!(filtered.len(), 4); // The 4 example.com URLs

        // Exclude URLs with "example.com"
        let pattern_filter =
            RegexFilterMiddleware::new(vec!["example\\.com"], false).unwrap();
        let filtered = url_list.clone().apply(&pattern_filter);

        assert_eq!(filtered.len(), 1); // The 1 non-example.com URLs
    }

    #[test]
    fn test_string_pattern_middleware() {
        let urls = vec![
            Url::parse("https://example.com/page1").unwrap(),
            Url::parse("https://example.org/page2").unwrap(),
            Url::parse("https://subdomain.example.com/page3").unwrap(),
            Url::parse("https://example.net/test").unwrap(),
        ];
        let url_list = UrlList::new(urls);

        // Test including URLs with specific patterns
        let patterns = vec!["example.com".to_string(), "example.net".to_string()];
        let include_filter = StringPatternMiddleware::new(patterns, true);
        let filtered = url_list.clone().apply(&include_filter);

        assert_eq!(filtered.len(), 3); // Should keep example.com, subdomain.example.com, and example.net URLs
        assert!(
            filtered
                .urls()
                .iter()
                .all(|url| url.as_str().contains("example.com")
                    || url.as_str().contains("example.net"))
        );

        // Test excluding URLs with specific patterns
        let patterns = vec!["example.com".to_string()];
        let exclude_filter = StringPatternMiddleware::new(patterns, false);
        let filtered = url_list.clone().apply(&exclude_filter);

        assert_eq!(filtered.len(), 2); // Should only keep example.org URL
        assert!(
            filtered
                .urls()
                .iter()
                .all(|url| !url.as_str().contains("example.com"))
        );
    }

    #[test]
    fn test_site_filter_middleware() {
        let urls = make_test_urls();
        let url_list = UrlList::new(urls);
        let base = Url::parse("https://example.com/").unwrap();

        // Keep only URLs on the same site
        let on_site = SiteFilterMiddleware::on_site(base.clone());
        let filtered = url_list.clone().apply(&on_site);

        assert_eq!(filtered.len(), 3); // The 3 example.com URLs

        // Keep only URLs on different sites
        let off_site = SiteFilterMiddleware::off_site(base);
        let filtered = url_list.clone().apply(&off_site);

        assert_eq!(filtered.len(), 2); // The 2 non-example.com URLs
    }

    #[test]
    fn test_chained_middleware() {
        let urls = make_test_urls();
        let url_list = UrlList::new(urls);

        let deduplicate = DeduplicateMiddleware;
        let pattern_filter =
            RegexFilterMiddleware::new(vec!["example\\.com"], true).unwrap();

        let filtered = url_list.apply(&deduplicate).apply(&pattern_filter);

        assert_eq!(filtered.len(), 3); // The 2 unique example.com URLs
    }
}
