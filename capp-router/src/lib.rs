//! URL classification and routing module.
//!
//! This module provides functionality for classifying URLs based on predefined rules
//! and grouping them into categories. It is designed to be flexible and extensible,
//! allowing for various classification strategies.
//!
//! The main components of this module are:
//!
//! - `Router`: The primary struct for URL classification and routing.
//! - `URLClassifier`: An internal struct used by `Router` for URL classification.
//! - `ClassificationRule`: A trait for implementing custom classification rules.
//! - `RegexRule`: A concrete implementation of `ClassificationRule` using regular expressions.
//!
//! # Examples
//!
//! ```
//! use capp_router::Router;
//! use url::Url;
//!
//! let mut router = Router::new();
//! router.add_regex_rule("example", vec!["example\\.com"], vec!["subdomain\\.example\\.com"]).unwrap();
//!
//! let url = Url::parse("https://example.com/page").unwrap();
//! assert_eq!(router.classify_url(&url), Some("example".to_string()));
//! ```
//!
//! This module is designed to be used optionally within a larger application context,
//! providing URL classification capabilities when needed without being a mandatory component.
#![warn(clippy::unwrap_used)]
use indexmap::IndexMap;
pub use regex::Regex;
pub use url;
use url::Url;

// ClassifiedURLs as a type alias using IndexMap
pub type ClassifiedURLs = IndexMap<String, Vec<Url>>;

/// A URL classifier and router.
///
/// This struct provides functionality to classify URLs based on predefined rules
/// and group them into categories.
#[derive(Debug)]
pub struct Router {
    classifier: URLClassifier,
}

// Define a trait for classification rules
trait ClassificationRule: Send + Sync {
    fn classify(&self, url: &Url) -> Option<String>;
}

// Implement the regex-based rule
struct RegexRule {
    name: String,
    allow: Vec<Regex>,
    except: Vec<Regex>,
}

impl Router {
    /// Creates a new Router instance.
    ///
    /// # Returns
    ///
    /// A new `Router` with an empty set of classification rules.
    pub fn new() -> Self {
        Router {
            classifier: URLClassifier::new(),
        }
    }

    /// Adds a new regex-based rule to the router.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the category this rule defines.
    /// * `allow` - A vector of regex patterns that URLs must match to be included.
    /// * `except` - A vector of regex patterns that, if matched, will exclude a URL.
    ///
    /// # Returns
    ///
    /// A `Result` which is `Ok(())` if the rule was added successfully, or an
    /// `Err` containing a `regex::Error` if there was an issue compiling the regexes.
    pub fn add_regex_rule(
        &mut self,
        name: &str,
        allow: Vec<&str>,
        except: Vec<&str>,
    ) -> Result<(), regex::Error> {
        let rule = RegexRule {
            name: name.to_string(),
            allow: allow
                .into_iter()
                .map(Regex::new)
                .collect::<Result<_, _>>()?,
            except: except
                .into_iter()
                .map(Regex::new)
                .collect::<Result<_, _>>()?,
        };
        self.classifier.add_rule(Box::new(rule));
        Ok(())
    }

    /// Classifies a list of URLs into categories.
    ///
    /// # Arguments
    ///
    /// * `urls` - A vector of `Url`s to classify.
    ///
    /// # Returns
    ///
    /// An `IndexMap` where keys are category names and values are vectors of URLs
    /// that belong to that category.
    pub fn classify_urls(&self, urls: Vec<Url>) -> ClassifiedURLs {
        let mut classified = ClassifiedURLs::new();
        for url in urls {
            if let Some(category) = self.classifier.classify(&url) {
                classified.entry(category).or_default().push(url);
            }
        }
        classified
    }

    /// Classifies a single URL.
    ///
    /// # Arguments
    ///
    /// * `url` - The `Url` to classify.
    ///
    /// # Returns
    ///
    /// An `Option<String>` containing the category name if the URL was classified,
    /// or `None` if no matching rule was found.
    pub fn classify_url(&self, url: &Url) -> Option<String> {
        self.classifier.classify(url)
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

impl ClassificationRule for RegexRule {
    fn classify(&self, url: &Url) -> Option<String> {
        let domain = url.domain()?;
        let path = url.path();
        let test_str = format!("{}{}", domain, path);

        #[allow(clippy::collapsible_if)]
        if self.allow.iter().any(|r| r.is_match(&test_str)) {
            if self.except.iter().all(|r| !r.is_match(&test_str)) {
                return Some(self.name.clone());
            }
        }
        None
    }
}

// URLClassifier struct
pub struct URLClassifier {
    rules: Vec<Box<dyn ClassificationRule>>,
}

impl std::fmt::Debug for URLClassifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("URLClassifier")
            .field("rules_count", &self.rules.len())
            .finish()
    }
}

impl URLClassifier {
    fn new() -> Self {
        URLClassifier { rules: Vec::new() }
    }

    fn add_rule(&mut self, rule: Box<dyn ClassificationRule>) {
        self.rules.push(rule);
    }

    fn classify(&self, url: &Url) -> Option<String> {
        for rule in &self.rules {
            if let Some(classification) = rule.classify(url) {
                return Some(classification);
            }
        }
        None
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_add_regex_rule() {
        let mut router = Router::new();
        let result = router.add_regex_rule(
            "example",
            vec!["example\\.com"],
            vec!["subdomain\\.example\\.com"],
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_classify_url() {
        let mut router = Router::new();
        router
            .add_regex_rule(
                "example",
                vec!["example\\.com"],
                vec!["subdomain\\.example\\.com"],
            )
            .unwrap();

        let url = Url::parse("https://example.com/page").unwrap();
        assert_eq!(router.classify_url(&url), Some("example".to_string()));

        let url = Url::parse("https://subdomain.example.com/page").unwrap();
        assert_eq!(router.classify_url(&url), None);
    }

    #[test]
    fn test_classify_urls() {
        let mut router = Router::new();
        router
            .add_regex_rule(
                "example",
                vec!["example\\.com"],
                vec!["subdomain\\.example\\.com"],
            )
            .unwrap();

        let urls = vec![
            Url::parse("https://example.com/page1").unwrap(),
            Url::parse("https://subdomain.example.com/page").unwrap(),
            Url::parse("https://example.com/page2").unwrap(),
        ];

        let classified = router.classify_urls(urls);
        assert_eq!(classified.len(), 1);
        assert_eq!(classified["example"].len(), 2);
    }
}
