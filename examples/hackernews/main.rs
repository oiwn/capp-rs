//! Typical real world example of another one Hackernews crawler!
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE, Engine as _};
use capp::prelude::{
    Computation, ComputationError, InMemoryTaskQueue, Task, TaskQueue, WorkerId,
    WorkerOptionsBuilder, WorkersManager, WorkersManagerOptionsBuilder,
};
use capp::{config::Configurable, http, reqwest};
use capp::{tracing, tracing_subscriber};
use rand::{seq::SliceRandom, thread_rng};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::LazyLock;
use std::{
    collections::HashSet,
    path,
    sync::{Arc, Mutex},
};
use url::{ParseError, Url};

const SEED_URLS: [&str; 1] = ["https://news.ycombinator.com"];

static URL_SET: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| {
    let mut set = HashSet::new();
    // Add some urls we do not want to add into queue
    set.insert("https://news.ycombinator.com/submit".into());
    set.insert("https://news.ycombinator.com/jobs".into());
    set.insert("https://news.ycombinator.com/show".into());
    set.insert("https://news.ycombinator.com/ask".into());
    set.insert("https://news.ycombinator.com/newcomments".into());
    set.insert("https://news.ycombinator.com/front".into());
    set.insert("https://news.ycombinator.com/newest".into());
    Mutex::new(set)
});

#[derive(Debug)]
pub struct LinkExtractionResult {
    pub links: Vec<Url>,
    pub errors: u32,
}

pub struct CategorizedLinks {
    general_links: Vec<Url>,
}

/// Used to store crawling links
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SiteLink {
    pub url: String,
}

#[derive(Debug)]
struct HNCrawler {}

struct Context {
    config: toml::Value,
    pub user_agents: Vec<String>,
}

impl Configurable for Context {
    fn config(&self) -> &toml::Value {
        &self.config
    }
}

impl SiteLink {
    fn new(url: &str) -> Self {
        Self { url: url.into() }
    }
}

impl Context {
    async fn from_config(config_file_path: impl AsRef<path::Path>) -> Self {
        let config = Self::load_config(config_file_path)
            .expect("Unable to read config file");
        let uas_file_path = config["app"]["user_agents_file"].as_str().unwrap();
        let user_agents = Self::load_text_file_lines(uas_file_path)
            .expect("Unable to read user agents file");
        Self {
            config,
            user_agents,
        }
    }

    // Get random user agent
    pub fn get_random_ua(&self) -> String {
        self.user_agents
            .choose(&mut thread_rng())
            .unwrap()
            .to_string()
    }
}

#[async_trait]
impl Computation<SiteLink, Context> for HNCrawler {
    /// Processor will fail tasks which value can be divided to 3
    async fn call(
        &self,
        worker_id: WorkerId,
        ctx: Arc<Context>,
        storage: Arc<dyn TaskQueue<SiteLink> + Send + Sync + 'static>,
        task: &mut Task<SiteLink>,
    ) -> Result<(), ComputationError> {
        tracing::info!("[worker-{}] Processing task: {:?}", worker_id, task);

        let url = task.payload.url.clone();
        tracing::info!("Fetching url: {:?}", &url);

        let http_client = Self::get_http_client(&ctx.clone());
        let base_url = Self::extract_base_url(&url).unwrap();

        match Self::fetch_html(http_client, &url).await {
            Ok((reqwest::StatusCode::OK, text)) => {
                tracing::info!("[{}] Profile data crawled.", &url);

                let links = Self::extract_links(&text, &base_url);
                tracing::info!(
                    "Links: {:?} Errors: {:?}",
                    links.links.len(),
                    links.errors
                );
                let links = Self::filter_links(links.links);
                tracing::info!("General: {}", links.general_links.len());

                let links_stored =
                    Self::store_links_website(links.general_links, storage.clone())
                        .await
                        .unwrap();
                tracing::info!("Links stored: {}", links_stored);

                // If we're on news page store it as file
                if url.contains("item?id=") {
                    if let Err(e) =
                        Self::save_page_to_file(&url, &text, "target/tmp")
                    {
                        tracing::error!("Failed to save page to file: {:?}", e);
                    } else {
                        tracing::info!("Page saved successfully.");
                    };
                }
            }
            Ok((code, _)) => {
                tracing::error!("Wrong response code: {}", code);
                return Err(ComputationError::Function(
                    "Wrong response code".into(),
                ));
            }
            Err(err) => {
                tracing::error!("Content fetching error: {}", err);
                return Err(ComputationError::Function(
                    "Content fetching error".into(),
                ));
            }
        };
        Ok(())
    }
}

impl HNCrawler {
    /// Fetch json content from response received by client
    pub async fn fetch_html(
        client: reqwest::Client,
        url: &str,
    ) -> reqwest::Result<(reqwest::StatusCode, String)> {
        let backoff = backoff::ExponentialBackoffBuilder::new()
            .with_randomization_factor(0.5)
            .with_max_interval(std::time::Duration::from_secs(10))
            .with_max_elapsed_time(Some(std::time::Duration::from_secs(30)))
            .build();

        let fetch_content = || async {
            let response = client
                .get(url)
                .header("Accept", "text/html,*/*;q=0.8")
                .header("Accept-Language", "en-US,en;q=0.5")
                .header("Accept-Encoding", "gzip, deflate")
                .send()
                .await?;
            let status = response.status();
            let text = response.text().await?;
            Ok((status, text))
        };

        tracing::info!("[{}] retrieving url...", url);
        backoff::future::retry(backoff, fetch_content).await
    }

    // Store links to website for further crawling
    async fn store_links_website(
        links: Vec<Url>,
        storage: Arc<dyn TaskQueue<SiteLink> + Send + Sync>,
    ) -> Result<usize, anyhow::Error> {
        let mut links_stored = 0;
        tracing::info!("Adding {} links to the queue...", links.len());

        for link in links.iter() {
            let link_str = link.as_str().to_owned();

            let should_store = {
                // Scoped lock acquisition
                let mut url_set_guard = URL_SET.lock().unwrap();
                url_set_guard.insert(link_str.clone())
            };

            if should_store {
                let link_data = SiteLink { url: link_str };
                storage.push(&Task::new(link_data)).await?;
                links_stored += 1;
            }
        }

        Ok(links_stored)
    }

    // Extract links using: DOM Tree -> CSS Selector -> links
    pub fn extract_links(content: &str, base_url: &Url) -> LinkExtractionResult {
        let document = Html::parse_document(content);
        let selector = Selector::parse("a[href]").unwrap();

        let mut links = Vec::new();
        let mut errors = 0;

        for element in document.select(&selector) {
            if let Some(href) = element.value().attr("href") {
                let url = base_url.join(href);
                match url {
                    Ok(absolute_url) => links.push(absolute_url),
                    Err(ParseError::RelativeUrlWithoutBase) => {
                        // Attempt to parse it as an absolute URL if it
                        // fails due to being a relative URL without a base.
                        match Url::parse(href) {
                            Ok(absolute_url) => links.push(absolute_url),
                            Err(_) => errors += 1,
                        }
                    }
                    Err(_) => errors += 1,
                };
            }
        }

        LinkExtractionResult { links, errors }
    }

    /// Filter links, left general links inside website and links to profiles
    pub fn filter_links(links: Vec<Url>) -> CategorizedLinks {
        let mut general_links = Vec::new();

        for url in links {
            match url.domain() {
                Some(domain) if domain == "news.ycombinator.com" => {
                    tracing::debug!("Url path: {}", url.path());
                    if url.path().contains("/user")
                        || url.path().contains("/vote")
                        || url.path().contains("/hide")
                    {
                        continue;
                    }

                    general_links.push(url);
                }
                Some(domain) => {
                    tracing::debug!("Skipping URL with domain: {}", domain);
                    continue;
                }
                None => {
                    tracing::debug!("URL has no domain: {}", url);
                    continue;
                }
            };
        }

        CategorizedLinks { general_links }
    }

    pub fn get_http_client(ctx: &Context) -> reqwest::Client {
        let proxy_provider = ctx.config()["app"]["proxy_provider"]
            .as_str()
            .expect("Can't find app.proxy_provider settings");
        let client: reqwest::Client =
            http::build_http_client(http::HttpClientParams::from_config(
                &ctx.config()[proxy_provider],
                &ctx.get_random_ua(),
            ))
            .unwrap();
        client
    }

    // save page to file
    fn save_page_to_file(
        url: &str,
        text: &str,
        base_dir: &str,
    ) -> Result<(), anyhow::Error> {
        // Generate the MD5 hash of the URL
        let md5_hash = md5::compute(url);
        let hash_prefix = format!("{:x}", &md5_hash)[..4].to_string();

        // Encode the URL to a valid filename using base64
        let encoded_string = URL_SAFE.encode(url.as_bytes());

        // Create the directory if it doesn't exist
        let dir_path = std::path::Path::new(base_dir).join(&hash_prefix);
        std::fs::create_dir_all(&dir_path)?;

        // Construct the file path
        // let encoded_filename = std::str::from_utf8(encoded_bytes)?.to_string();
        let file_path = dir_path.join(format!("{}.html", encoded_string));

        // Open the file in write mode
        let mut file = std::fs::File::create(file_path)?;

        // Write the content to the file
        file.write_all(text.as_bytes())?;

        Ok(())
    }

    /// Try to extract base_url from url
    fn extract_base_url(input_url: &str) -> Option<Url> {
        let parsed_url = Url::parse(input_url).ok()?;

        let base_url = format!(
            "{}://{}{}",
            parsed_url.scheme(),
            parsed_url.host_str()?, // Returns None if no host present
            match parsed_url.port() {
                Some(port) => format!(":{}", port), // Include the port if present
                None => String::new(),
            }
        );

        Url::parse(&base_url).ok()
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let ctx =
        Arc::new(Context::from_config("examples/hackernews/hn_config.toml").await);

    tracing::info!("Starting HN crawler...");

    let manager_options = WorkersManagerOptionsBuilder::default()
        .worker_options(
            WorkerOptionsBuilder::default()
                .max_retries(10_u32)
                .build()
                .unwrap(),
        )
        .concurrency_limit(4_usize)
        .build()
        .unwrap();

    let storage: InMemoryTaskQueue<SiteLink> = InMemoryTaskQueue::new();
    let tasks_queue_len = storage.list.lock().unwrap().len();

    tracing::info!("Website links tasks in queue: {}", tasks_queue_len);
    // Add seed urls
    if tasks_queue_len == 0 {
        tracing::warn!("Queue is empty! Seeding urls... {}", SEED_URLS.join(" "));
        for url in SEED_URLS.iter() {
            let initial_task = Task::new(SiteLink::new(url));
            let _ = storage.push(&initial_task).await;
        }
    }

    let computation = Arc::new(HNCrawler {});
    let mut manager = WorkersManager::new_from_arcs(
        ctx.clone(),
        computation,
        Arc::new(storage),
        manager_options,
    );
    manager.run_workers().await;
}
