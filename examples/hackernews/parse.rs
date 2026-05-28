use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoryMeta {
    pub id: u64,
    pub title: String,
    pub url: Option<String>,
    pub score: u32,
    pub by: Option<String>,
    pub time: Option<i64>,
    pub comments_count: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Comment {
    pub id: u64,
    pub story_id: u64,
    pub parent_id: Option<u64>,
    pub indent: u32,
    pub by: Option<String>,
    pub time: Option<i64>,
    pub text_html: String,
}

pub struct ItemPage {
    pub story: StoryMeta,
    pub comments: Vec<Comment>,
}

pub struct ListingPage {
    pub stories: Vec<u64>,
    pub next_page: Option<u32>,
}

pub fn parse_listing(html: &str) -> ListingPage {
    let doc = Html::parse_document(html);
    let row = Selector::parse("tr.athing.submission").unwrap();
    let more = Selector::parse("a.morelink").unwrap();

    let stories = doc
        .select(&row)
        .filter_map(|tr| tr.value().attr("id").and_then(|s| s.parse::<u64>().ok()))
        .collect();

    // Footer "More" link is `<a class="morelink" href="news?p=N" rel="next">More</a>`.
    let next_page = doc.select(&more).next().and_then(|a| {
        a.value()
            .attr("href")
            .and_then(|h| h.split("p=").nth(1).and_then(|n| n.parse::<u32>().ok()))
    });

    ListingPage { stories, next_page }
}

pub fn parse_item(id: u64, html: &str) -> Option<ItemPage> {
    let doc = Html::parse_document(html);
    let submission = Selector::parse("tr.athing.submission").unwrap();
    let subtext = Selector::parse("td.subtext").unwrap();

    let row = doc.select(&submission).next()?;
    let title_a = row
        .select(&Selector::parse("span.titleline > a").unwrap())
        .next()?;
    let title = title_a.text().collect::<String>();
    let url = title_a.value().attr("href").map(|s| s.to_string());

    let sub = doc.select(&subtext).next();
    let (score, by, time, comments_count) = sub
        .map(|s| extract_subtext(s))
        .unwrap_or((0, None, None, 0));

    let story = StoryMeta {
        id,
        title,
        url,
        score,
        by,
        time,
        comments_count,
    };

    let comments = parse_comments(id, &doc);
    Some(ItemPage { story, comments })
}

fn extract_subtext(el: ElementRef<'_>) -> (u32, Option<String>, Option<i64>, u32) {
    let score = el
        .select(&Selector::parse("span.score").unwrap())
        .next()
        .and_then(|s| s.text().next())
        .and_then(parse_leading_u32)
        .unwrap_or(0);

    let by = el
        .select(&Selector::parse("a.hnuser").unwrap())
        .next()
        .map(|a| a.text().collect::<String>());

    let time = el
        .select(&Selector::parse("span.age").unwrap())
        .next()
        .and_then(|s| s.value().attr("title"))
        .and_then(|t| {
            t.split_whitespace()
                .nth(1)
                .and_then(|n| n.parse::<i64>().ok())
        });

    // Last <a> in the subline: either "N comments" or "discuss".
    let comments_count = el
        .select(&Selector::parse("a").unwrap())
        .last()
        .and_then(|a| {
            let text = a.text().collect::<String>();
            parse_leading_u32(&text.replace('\u{a0}', " "))
        })
        .unwrap_or(0);

    (score, by, time, comments_count)
}

/// Parse leading ASCII digits from a string (handles "37 points", "37\u{a0}comments").
fn parse_leading_u32(s: &str) -> Option<u32> {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn parse_comments(story_id: u64, doc: &Html) -> Vec<Comment> {
    let row_sel = Selector::parse("tr.athing.comtr").unwrap();
    let ind_sel = Selector::parse("td.ind").unwrap();
    let user_sel = Selector::parse("a.hnuser").unwrap();
    let age_sel = Selector::parse("span.age").unwrap();
    let body_sel = Selector::parse("div.commtext").unwrap();

    let mut out = Vec::new();
    // Stack of (indent, comment_id) so we can resolve the parent of any row
    // as the most recent ancestor with smaller indent.
    let mut stack: Vec<(u32, u64)> = Vec::new();

    for row in doc.select(&row_sel) {
        let Some(id) = row.value().attr("id").and_then(|s| s.parse::<u64>().ok())
        else {
            continue;
        };

        let indent = row
            .select(&ind_sel)
            .next()
            .and_then(|td| td.value().attr("indent"))
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);

        while stack.last().is_some_and(|(d, _)| *d >= indent) {
            stack.pop();
        }
        let parent_id = stack.last().map(|(_, pid)| *pid);

        let by = row
            .select(&user_sel)
            .next()
            .map(|a| a.text().collect::<String>());

        let time = row
            .select(&age_sel)
            .next()
            .and_then(|s| s.value().attr("title"))
            .and_then(|t| {
                t.split_whitespace()
                    .nth(1)
                    .and_then(|n| n.parse::<i64>().ok())
            });

        let text_html = row
            .select(&body_sel)
            .next()
            .map(|d| d.inner_html())
            .unwrap_or_default();

        out.push(Comment {
            id,
            story_id,
            parent_id,
            indent,
            by,
            time,
            text_html,
        });
        stack.push((indent, id));
    }

    out
}
