use capp_urls::{
    DeduplicateMiddleware, PatternFilterMiddleware, SiteFilterMiddleware, UrlList,
};
use url::Url;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a list of URLs
    let urls = vec![
        Url::parse("https://example.com/page1")?,
        Url::parse("https://example.com/page2")?,
        Url::parse("https://example.com/page1")?, // Duplicate
        Url::parse("https://example.org/page1")?,
    ];

    // Create a URL list and apply middleware
    let processed_urls = UrlList::new(urls)
        .apply(&DeduplicateMiddleware)
        .apply(&PatternFilterMiddleware::new("example\\.com", true)?)
        .urls()
        .to_vec();

    println!("Processed URLs: {:?}", processed_urls);

    Ok(())
}
